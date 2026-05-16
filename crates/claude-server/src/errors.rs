//! Shared error mapping for tool handlers.
//!
//! Most handlers do `wrapper.call().await.map_err(...)?` to flatten
//! a [`claude_wrapper::error::Error`] (or some other display-able
//! failure) into the MCP error type. This module hosts the two
//! shared mappers so every handler benefits from the same
//! special-casing without duplicating the same five-line helper in
//! every file.
//!
//! - [`from_wrapper`] -- typed-aware; recognizes
//!   [`claude_wrapper::error::Error::Auth`] (and inspectable
//!   [`Error::auth_kind`]-positive `CommandFailed`s) and emits a
//!   friendlier one-line message with a remediation hint.
//! - [`internal`] -- generic display fallback for failures that
//!   aren't wrapper errors (e.g. local parse errors).
//!
//! Convention: prefer [`from_wrapper`] anywhere you're handling a
//! [`claude_wrapper::error::Error`] -- it's strictly more useful.
//! Reach for [`internal`] only when you genuinely have something
//! else (a `&str`, a third-party error).

use std::fmt::Display;

use claude_wrapper::auth::AuthErrorKind;
use claude_wrapper::error::Error;

/// Map a [`claude_wrapper::error::Error`] into the MCP error type.
///
/// For [`Error::Auth`] (and any `CommandFailed` that
/// [`Error::auth_kind`] positively classifies), append a
/// remediation hint so an agent or coordinator gets actionable
/// guidance instead of raw CLI stderr. Other variants stringify
/// as before.
#[allow(dead_code)] // unused under `--no-default-features` (no surface features on)
pub(crate) fn from_wrapper(e: Error) -> tower_mcp::Error {
    match e.auth_kind() {
        Some(kind) => {
            let hint = remediation(kind);
            tower_mcp::Error::internal(format!("{e}. hint: {hint}"))
        }
        None => tower_mcp::Error::internal(e.to_string()),
    }
}

/// Generic stringifying mapper for non-wrapper failures
/// (parse errors, local validation, third-party libraries).
#[allow(dead_code)] // unused under `--no-default-features` (no surface features on)
pub(crate) fn internal(e: impl Display) -> tower_mcp::Error {
    tower_mcp::Error::internal(e.to_string())
}

#[allow(dead_code)] // unused under `--no-default-features` (no surface features on)
fn remediation(kind: AuthErrorKind) -> &'static str {
    match kind {
        AuthErrorKind::NotAuthenticated => {
            "no credentials configured. Run `claude login` or set ANTHROPIC_API_KEY"
        }
        AuthErrorKind::Expired => "stored credentials are expired. Re-run `claude login`",
        AuthErrorKind::InvalidCredentials => {
            "credentials were rejected. Check ANTHROPIC_API_KEY or re-run `claude login`"
        }
        AuthErrorKind::RateLimit => "rate limit or quota exceeded. Wait, top up, or switch keys",
        AuthErrorKind::ProviderError => {
            "cloud provider auth failed (Bedrock/Vertex). Check AWS/GCP credentials"
        }
        AuthErrorKind::Other => "auth-shaped failure; call `claude_auth_status` for live state",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn from_wrapper_appends_hint_on_auth_variant() {
        let e = Error::Auth {
            kind: AuthErrorKind::NotAuthenticated,
            command: "claude --print".to_string(),
            exit_code: 1,
            message: "Not authenticated. Run `claude login`.".to_string(),
        };
        let mcp = from_wrapper(e);
        let msg = format!("{mcp}");
        assert!(msg.contains("auth error"), "msg: {msg}");
        assert!(msg.contains("not_authenticated") || msg.contains("NotAuthenticated"));
        assert!(msg.contains("hint:"), "missing hint: {msg}");
        assert!(msg.contains("claude login"), "missing remediation: {msg}");
    }

    #[test]
    fn from_wrapper_inspects_command_failed_via_auth_kind() {
        // The constructor would have caught this in real exec.rs --
        // simulate a hand-built CommandFailed (older code path) to
        // confirm the inspection fallback still adds the hint.
        let e = Error::CommandFailed {
            command: "claude --print".to_string(),
            exit_code: 1,
            stdout: String::new(),
            stderr: "401 Unauthorized".to_string(),
            working_dir: None,
        };
        let mcp = from_wrapper(e);
        let msg = format!("{mcp}");
        assert!(msg.contains("hint:"), "missing hint: {msg}");
        assert!(
            msg.contains("Check ANTHROPIC_API_KEY") || msg.contains("rejected"),
            "expected invalid_credentials hint: {msg}"
        );
    }

    #[test]
    fn from_wrapper_preserves_non_auth_errors_verbatim() {
        let e = Error::Timeout {
            timeout_seconds: 30,
        };
        let mcp = from_wrapper(e);
        let msg = format!("{mcp}");
        assert!(msg.contains("30s"), "msg: {msg}");
        assert!(!msg.contains("hint:"), "should not add hint: {msg}");
    }

    #[test]
    fn from_wrapper_does_not_add_hint_to_unrelated_command_failed() {
        let e = Error::CommandFailed {
            command: "claude something".to_string(),
            exit_code: 2,
            stdout: String::new(),
            stderr: "syntax error".to_string(),
            working_dir: None,
        };
        let mcp = from_wrapper(e);
        let msg = format!("{mcp}");
        assert!(!msg.contains("hint:"), "should not add hint: {msg}");
    }

    #[test]
    fn internal_stringifies_generic_display() {
        let mcp = internal("a parse error");
        // tower_mcp::Error wraps in JSON-RPC framing; just confirm
        // the original message is preserved verbatim somewhere.
        assert!(format!("{mcp}").contains("a parse error"));
    }
}
