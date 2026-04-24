//! Map [`crate::error::Error`] into MCP-friendly tool results.
//!
//! Tool handlers return `Result<CallToolResult, ...>` to tower-mcp.
//! Per the MCP spec, tool failures aren't protocol errors -- they
//! arrive as a successful response with `is_error: true`. We use that
//! shape so the LLM/client always sees a typed error description.
//!
//! Each variant is rendered as JSON with a stable `code` and `kind`
//! plus variant-specific fields, so callers can branch on `code`
//! without parsing strings.

use serde_json::json;
use tower_mcp::CallToolResult;

use crate::error::Error;

/// Convert a `claude_wrapper` error into a `CallToolResult` carrying
/// structured failure data. Always returns `Ok(CallToolResult)` --
/// the result itself reports the failure with `is_error: true`.
pub(crate) fn error_to_result(err: Error) -> CallToolResult {
    let payload = match &err {
        Error::NotFound => json!({
            "code": "not_found",
            "kind": "NotFound",
            "message": err.to_string(),
        }),
        Error::CommandFailed {
            command,
            exit_code,
            stdout,
            stderr,
            working_dir,
        } => json!({
            "code": "command_failed",
            "kind": "CommandFailed",
            "command": command,
            "exit_code": exit_code,
            "stdout": stdout,
            "stderr": stderr,
            "working_dir": working_dir.as_ref().map(|p| p.display().to_string()),
        }),
        Error::Io {
            message,
            working_dir,
            ..
        } => json!({
            "code": "io",
            "kind": "Io",
            "message": message,
            "working_dir": working_dir.as_ref().map(|p| p.display().to_string()),
        }),
        Error::Timeout { timeout_seconds } => json!({
            "code": "timeout",
            "kind": "Timeout",
            "timeout_seconds": timeout_seconds,
        }),
        #[cfg(feature = "json")]
        Error::Json { message, .. } => json!({
            "code": "json",
            "kind": "Json",
            "message": message,
        }),
        Error::VersionMismatch { found, minimum } => json!({
            "code": "version_mismatch",
            "kind": "VersionMismatch",
            "found": found.to_string(),
            "minimum": minimum.to_string(),
        }),
        Error::DangerousNotAllowed { env_var } => json!({
            "code": "dangerous_not_allowed",
            "kind": "DangerousNotAllowed",
            "env_var": env_var,
        }),
        Error::BudgetExceeded { total_usd, max_usd } => json!({
            "code": "budget_exceeded",
            "kind": "BudgetExceeded",
            "total_usd": total_usd,
            "max_usd": max_usd,
        }),
    };

    CallToolResult::error(payload.to_string())
}
