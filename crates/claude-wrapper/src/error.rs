use std::path::PathBuf;

use crate::auth::AuthErrorKind;

/// Errors returned by claude-wrapper operations.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// The `claude` binary was not found in PATH.
    #[error("claude binary not found in PATH")]
    NotFound,

    /// A claude command failed with a non-zero exit code.
    #[error("claude command failed: {command} (exit code {exit_code}){}{}{}", working_dir.as_ref().map(|d| format!(" (in {})", d.display())).unwrap_or_default(), if stdout.is_empty() { String::new() } else { format!("\nstdout: {stdout}") }, if stderr.is_empty() { String::new() } else { format!("\nstderr: {stderr}") })]
    CommandFailed {
        command: String,
        exit_code: i32,
        stdout: String,
        stderr: String,
        working_dir: Option<PathBuf>,
    },

    /// An I/O error occurred while spawning or communicating with the process.
    #[error("io error: {message}{}", working_dir.as_ref().map(|d| format!(" (in {})", d.display())).unwrap_or_default())]
    Io {
        message: String,
        #[source]
        source: std::io::Error,
        working_dir: Option<PathBuf>,
    },

    /// The command timed out.
    #[error("claude command timed out after {timeout_seconds}s")]
    Timeout { timeout_seconds: u64 },

    /// JSON parsing failed.
    #[cfg(feature = "json")]
    #[error("json parse error: {message}")]
    Json {
        message: String,
        #[source]
        source: serde_json::Error,
    },

    /// The installed CLI version does not meet the minimum requirement.
    #[error("CLI version {found} does not meet minimum requirement {minimum}")]
    VersionMismatch {
        found: crate::version::CliVersion,
        minimum: crate::version::CliVersion,
    },

    /// Construction of a `dangerous::Client` was attempted without
    /// the opt-in env-var set. The env-var name is a compile-time
    /// constant exported from [`crate::dangerous::ALLOW_ENV`].
    #[error(
        "dangerous operations are not allowed; set the env var `{env_var}=1` at process start if you really mean it"
    )]
    DangerousNotAllowed { env_var: &'static str },

    /// A configured [`BudgetTracker`](crate::budget::BudgetTracker) has
    /// hit its `max_usd` ceiling. Raised before the next call is
    /// dispatched, so the CLI is not invoked.
    #[error("budget exceeded: ${total_usd:.4} spent, ${max_usd:.4} max")]
    BudgetExceeded { total_usd: f64, max_usd: f64 },

    /// A [`DuplexSession`](crate::duplex::DuplexSession) operation was
    /// attempted after the session task exited (child died, EOF on
    /// stdout, or the session was closed). Pending replies are
    /// resolved with this error.
    #[cfg(feature = "async")]
    #[error("duplex session is closed")]
    DuplexClosed,

    /// [`DuplexSession::send`](crate::duplex::DuplexSession::send) was
    /// called while another turn is already in flight. Wait for the
    /// outstanding turn to resolve before issuing another.
    #[cfg(feature = "async")]
    #[error("duplex session has a turn in flight")]
    DuplexTurnInFlight,

    /// A control request issued from
    /// [`DuplexSession::interrupt`](crate::duplex::DuplexSession::interrupt)
    /// (or any other outbound `control_request`) was answered by the
    /// CLI with a `subtype: "error"` payload.
    #[cfg(feature = "async")]
    #[error("duplex control request failed: {message}")]
    DuplexControlFailed {
        /// The error message extracted from the CLI's control_response.
        message: String,
    },

    /// A history-module operation (parsing or locating session
    /// JSONL under `~/.claude/projects/`) failed in a way that
    /// doesn't fit the I/O or JSON variants -- e.g. unknown
    /// session id, missing user home directory.
    #[error("history error: {message}")]
    History {
        /// Human-readable description of what went wrong.
        message: String,
    },

    /// An artifacts-module operation (parsing or locating files
    /// under `~/.claude/agents/` and friends) failed in a way that
    /// doesn't fit the I/O variant -- e.g. unknown agent name,
    /// missing user home directory.
    #[error("artifacts error: {message}")]
    Artifacts {
        /// Human-readable description of what went wrong.
        message: String,
    },

    /// A `claude` invocation failed and looked auth-shaped to the
    /// classifier. Hosts can match on this variant to trigger a
    /// re-auth flow, surface a clean message, or skip retries.
    /// `kind` carries the best-effort subcategory; `message` is the
    /// stderr (or stdout fallback) the classifier matched against.
    ///
    /// Raised at exec time when [`crate::auth::classify_failure`]
    /// returns `Some(_)` for a CLI failure that would otherwise
    /// have been [`Error::CommandFailed`]. Cases the classifier
    /// missed remain `CommandFailed`; call
    /// [`Error::auth_kind`] for opt-in inspection of those.
    #[error("auth error ({kind:?}): {command} (exit code {exit_code}): {message}")]
    Auth {
        /// Best-effort classification.
        kind: AuthErrorKind,
        /// The full command line that failed.
        command: String,
        /// Process exit code.
        exit_code: i32,
        /// Human-readable message extracted from stderr (or stdout).
        message: String,
    },
}

impl Error {
    /// Construct an [`Error`] from a CLI failure. Runs the
    /// auth-error classifier; if it matches, returns
    /// [`Error::Auth`]. Otherwise returns [`Error::CommandFailed`]
    /// unchanged.
    ///
    /// This is the canonical entry point for raising failures from
    /// `exec.rs`-shaped sites -- replacing direct construction of
    /// `CommandFailed` ensures every consumer benefits from typed
    /// auth errors automatically.
    pub fn from_command_failure(
        command: String,
        exit_code: i32,
        stdout: String,
        stderr: String,
        working_dir: Option<PathBuf>,
    ) -> Self {
        if let Some(kind) = crate::auth::classify_failure(exit_code, &stdout, &stderr) {
            // Prefer stderr for the human-facing message; fall back
            // to stdout when stderr is empty (some CLIs send all
            // diagnostics to stdout).
            let message = if !stderr.trim().is_empty() {
                stderr.trim().to_string()
            } else {
                stdout.trim().to_string()
            };
            Self::Auth {
                kind,
                command,
                exit_code,
                message,
            }
        } else {
            Self::CommandFailed {
                command,
                exit_code,
                stdout,
                stderr,
                working_dir,
            }
        }
    }

    /// Inspect whether this error is auth-shaped. Returns
    /// `Some(kind)` for [`Error::Auth`] (the auto-typed path) and
    /// also re-runs [`crate::auth::classify_failure`] on
    /// [`Error::CommandFailed`] for cases the constructor missed.
    /// Returns `None` for everything else (`Io`, `Timeout`, etc.).
    ///
    /// Most consumers should match on [`Error::Auth`] directly --
    /// this method is the escape hatch for low-confidence
    /// classifier patterns the constructor was too conservative
    /// about.
    pub fn auth_kind(&self) -> Option<AuthErrorKind> {
        match self {
            Self::Auth { kind, .. } => Some(*kind),
            Self::CommandFailed {
                exit_code,
                stdout,
                stderr,
                ..
            } => crate::auth::classify_failure(*exit_code, stdout, stderr),
            _ => None,
        }
    }
}

impl From<std::io::Error> for Error {
    fn from(e: std::io::Error) -> Self {
        Self::Io {
            message: e.to_string(),
            source: e,
            working_dir: None,
        }
    }
}

/// Result type alias for claude-wrapper operations.
pub type Result<T> = std::result::Result<T, Error>;

#[cfg(test)]
mod tests {
    use super::*;

    fn command_failed(stdout: &str, stderr: &str, working_dir: Option<PathBuf>) -> Error {
        Error::CommandFailed {
            command: "/bin/claude --print".to_string(),
            exit_code: 7,
            stdout: stdout.to_string(),
            stderr: stderr.to_string(),
            working_dir,
        }
    }

    #[test]
    fn command_failed_display_includes_command_and_exit_code() {
        let e = command_failed("", "", None);
        let s = e.to_string();
        assert!(s.contains("/bin/claude --print"));
        assert!(s.contains("exit code 7"));
    }

    #[test]
    fn command_failed_display_omits_empty_stdout_and_stderr() {
        let s = command_failed("", "", None).to_string();
        assert!(!s.contains("stdout:"));
        assert!(!s.contains("stderr:"));
    }

    #[test]
    fn command_failed_display_includes_nonempty_stdout() {
        let s = command_failed("hello", "", None).to_string();
        assert!(s.contains("stdout: hello"));
    }

    #[test]
    fn command_failed_display_includes_nonempty_stderr() {
        let s = command_failed("", "boom", None).to_string();
        assert!(s.contains("stderr: boom"));
    }

    #[test]
    fn command_failed_display_includes_both_streams_when_present() {
        let s = command_failed("out", "err", None).to_string();
        assert!(s.contains("stdout: out"));
        assert!(s.contains("stderr: err"));
    }

    #[test]
    fn command_failed_display_includes_working_dir_when_present() {
        let s = command_failed("", "", Some(PathBuf::from("/tmp/proj"))).to_string();
        assert!(s.contains("/tmp/proj"));
    }

    #[test]
    fn command_failed_display_omits_working_dir_when_absent() {
        let s = command_failed("", "", None).to_string();
        assert!(!s.contains("(in "));
    }

    #[test]
    fn timeout_display_formats_seconds() {
        let s = Error::Timeout {
            timeout_seconds: 42,
        }
        .to_string();
        assert!(s.contains("42s"));
    }

    #[test]
    fn io_error_display_includes_working_dir_when_present() {
        let e = Error::Io {
            message: "spawn failed".to_string(),
            source: std::io::Error::new(std::io::ErrorKind::NotFound, "no file"),
            working_dir: Some(PathBuf::from("/work")),
        };
        let s = e.to_string();
        assert!(s.contains("spawn failed"));
        assert!(s.contains("/work"));
    }

    // -- from_command_failure / auth_kind ---------------------------

    #[test]
    fn from_command_failure_unrelated_stderr_yields_command_failed() {
        let e = Error::from_command_failure(
            "claude --print".into(),
            1,
            String::new(),
            "syntax error".into(),
            None,
        );
        assert!(matches!(e, Error::CommandFailed { .. }));
        assert_eq!(e.auth_kind(), None);
    }

    #[test]
    fn from_command_failure_auth_stderr_yields_auth_variant() {
        let e = Error::from_command_failure(
            "claude --print".into(),
            1,
            String::new(),
            "Not authenticated. Run `claude login`.".into(),
            None,
        );
        match &e {
            Error::Auth { kind, message, .. } => {
                assert_eq!(*kind, AuthErrorKind::NotAuthenticated);
                assert!(message.contains("Not authenticated"));
            }
            other => panic!("expected Auth, got {other:?}"),
        }
        assert_eq!(e.auth_kind(), Some(AuthErrorKind::NotAuthenticated));
    }

    #[test]
    fn from_command_failure_uses_stdout_message_when_stderr_empty() {
        let e = Error::from_command_failure(
            "claude --print".into(),
            1,
            "Invalid API key".into(),
            String::new(),
            None,
        );
        match &e {
            Error::Auth { message, kind, .. } => {
                assert_eq!(*kind, AuthErrorKind::InvalidCredentials);
                assert_eq!(message, "Invalid API key");
            }
            other => panic!("expected Auth, got {other:?}"),
        }
    }

    #[test]
    fn auth_kind_inspects_command_failed_for_missed_classifications() {
        // The constructor would have caught this, but a hand-built
        // CommandFailed (e.g. constructed by older code or by a
        // caller not going through the helper) is still inspectable.
        let e = Error::CommandFailed {
            command: "claude --print".into(),
            exit_code: 1,
            stdout: String::new(),
            stderr: "401 Unauthorized".into(),
            working_dir: None,
        };
        assert_eq!(e.auth_kind(), Some(AuthErrorKind::InvalidCredentials));
    }

    #[test]
    fn auth_kind_returns_none_for_non_command_errors() {
        assert_eq!(Error::NotFound.auth_kind(), None);
        assert_eq!(Error::Timeout { timeout_seconds: 5 }.auth_kind(), None);
    }
}
