//! CLI output parsing helpers — pure functions, no pool state.
//!
//! These helpers parse CLI output, detect permission prompts, and extract
//! structured failure information from errors. All functions are stateless
//! and depend only on their arguments.

use crate::error::Error;

/// Generate a short unique ID based on the current timestamp.
pub(crate) fn new_id() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    format!("{nanos:x}")
}

// ── Permission prompt detection ─────────────────────────────────────

/// Patterns in stderr that indicate the CLI is waiting for permission/tool approval.
pub(crate) const PERMISSION_PATTERNS: &[&str] = &[
    "Allow",
    "allow this action",
    "approve",
    "permission",
    "Do you want to",
    "tool requires approval",
    "wants to use",
    "Press Enter",
    "y/n",
    "Y/n",
    "(yes/no)",
];

/// Structured failure details extracted from an error.
pub(crate) struct FailureDetails {
    pub(crate) failed_command: Option<String>,
    pub(crate) exit_code: Option<i32>,
    pub(crate) stderr: Option<String>,
}

/// Extract structured failure details from a pool error.
///
/// When the error wraps a `CommandFailed`, we capture the command, exit code,
/// and stderr so callers get actionable diagnostics.
pub(crate) fn extract_failure_details(err: &Error) -> FailureDetails {
    match err {
        Error::Wrapper(claude_wrapper::Error::CommandFailed {
            command,
            exit_code,
            stderr,
            ..
        }) => FailureDetails {
            failed_command: Some(command.clone()),
            exit_code: Some(*exit_code),
            stderr: if stderr.is_empty() {
                None
            } else {
                Some(stderr.clone())
            },
        },
        _ => FailureDetails {
            failed_command: None,
            exit_code: None,
            stderr: None,
        },
    }
}

/// Inspect a claude-wrapper error for signs of a permission prompt.
///
/// Returns `Some(Error::PermissionPromptDetected)` if the stderr in a
/// `CommandFailed` error contains permission prompt patterns.
pub(crate) fn detect_permission_prompt(
    err: &claude_wrapper::Error,
    slot_id: &str,
) -> Option<Error> {
    let stderr = match err {
        claude_wrapper::Error::CommandFailed { stderr, .. } => stderr,
        _ => return None,
    };

    if stderr.is_empty() {
        return None;
    }

    for pattern in PERMISSION_PATTERNS {
        if stderr.contains(pattern) {
            let tool_name = extract_tool_name(stderr);
            tracing::warn!(
                slot_id,
                tool = %tool_name,
                "permission prompt detected in slot stderr"
            );
            return Some(Error::PermissionPromptDetected {
                tool_name,
                stderr: stderr.clone(),
                slot_id: slot_id.to_string(),
            });
        }
    }

    None
}

/// Best-effort extraction of the tool name from stderr text.
pub(crate) fn extract_tool_name(stderr: &str) -> String {
    for line in stderr.lines() {
        let trimmed = line.trim();
        if let Some(rest) = trimmed.strip_prefix("Allow ")
            && let Some(tool) = rest.split_whitespace().next()
        {
            return tool.trim_end_matches('?').to_string();
        }
        if let Some(idx) = trimmed.find("wants to use ") {
            let after = &trimmed[idx + "wants to use ".len()..];
            if let Some(tool) = after.split_whitespace().next() {
                return tool.trim_end_matches(['.', '?', ',']).to_string();
            }
        }
    }
    "unknown".to_string()
}
