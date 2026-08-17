//! The crate's [`Error`] type and [`Result`] alias.
//!
//! Every fallible operation returns [`Result<T>`]. [`Error`] is
//! `#[non_exhaustive]` and classifies CLI failures into typed variants
//! (auth, rail-stop caps, timeouts) via
//! [`Error::from_command_failure`]; see its variant docs for what each
//! carries.

use std::path::PathBuf;

use crate::auth::AuthErrorKind;

/// Errors returned by claude-wrapper operations.
///
/// This enum is `#[non_exhaustive]`: new variants may be added in
/// future releases without a major version bump, so downstream `match`
/// expressions must include a wildcard (`_ =>`) arm. Matching on the
/// specific variants you care about (e.g. [`Error::Auth`],
/// [`Error::MaxTurnsExceeded`]) keeps working across upgrades.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    /// The `claude` binary was not found in PATH.
    #[error("claude binary not found in PATH")]
    NotFound,

    /// A claude command failed with a non-zero exit code.
    #[error("claude command failed: {command} (exit code {exit_code}){}{}{}", working_dir.as_ref().map(|d| format!(" (in {})", d.display())).unwrap_or_default(), if stdout.is_empty() { String::new() } else { format!("\nstdout: {stdout}") }, if stderr.is_empty() { String::new() } else { format!("\nstderr: {stderr}") })]
    CommandFailed {
        /// The full command line that failed.
        command: String,
        /// Process exit code.
        exit_code: i32,
        /// Captured standard output.
        stdout: String,
        /// Captured standard error.
        stderr: String,
        /// Working directory the command ran in, when set.
        working_dir: Option<PathBuf>,
    },

    /// An I/O error occurred while spawning or communicating with the process.
    #[error("io error: {message}{}", working_dir.as_ref().map(|d| format!(" (in {})", d.display())).unwrap_or_default())]
    Io {
        /// Human-readable description of the I/O failure.
        message: String,
        /// The underlying I/O error.
        #[source]
        source: std::io::Error,
        /// Working directory the operation ran in, when set.
        working_dir: Option<PathBuf>,
    },

    /// The command timed out.
    #[error("claude command timed out after {timeout_seconds}s")]
    Timeout {
        /// The timeout, in seconds, that was exceeded.
        timeout_seconds: u64,
    },

    /// The caller explicitly cancelled an in-flight command.
    ///
    /// The wrapper does not return this error until it has terminated the
    /// owned process group and reaped the direct child.
    #[cfg(feature = "async")]
    #[error("claude command cancelled")]
    Cancelled,

    /// JSON parsing failed.
    #[cfg(feature = "json")]
    #[error("json parse error: {message}")]
    Json {
        /// Human-readable description of what failed to parse.
        message: String,
        /// The underlying serde error.
        #[source]
        source: serde_json::Error,
    },

    /// The installed CLI version does not meet the minimum requirement.
    #[error("CLI version {found} does not meet minimum requirement {minimum}")]
    VersionMismatch {
        /// The version detected on the system.
        found: crate::version::CliVersion,
        /// The minimum version required.
        minimum: crate::version::CliVersion,
    },

    /// The installed CLI is outside the tested-against range and the
    /// caller asked for a hard gate via
    /// [`Claude::ensure_tested_cli_version`](crate::Claude::ensure_tested_cli_version).
    ///
    /// Distinct from [`Error::VersionMismatch`], which is about a
    /// caller-declared minimum for one operation. This one carries
    /// both bounds because being *newer* than the tested maximum is
    /// also a refusable condition.
    #[error(
        "CLI version {found} is outside the tested range {tested_min}..={tested_max}{}",
        if found < tested_min { " (older than the supported minimum)" } else { " (newer than the tested maximum)" }
    )]
    UntestedCliVersion {
        /// The version detected on the system.
        found: crate::version::CliVersion,
        /// Lowest CLI version the wrapper supports.
        tested_min: crate::version::CliVersion,
        /// Highest CLI version the wrapper has been tested against.
        tested_max: crate::version::CliVersion,
    },

    /// Construction of a `dangerous::Client` was attempted without
    /// the opt-in env-var set. The env-var name is a compile-time
    /// constant exported from [`crate::dangerous::ALLOW_ENV`].
    #[error(
        "dangerous operations are not allowed; set the env var `{env_var}=1` at process start if you really mean it"
    )]
    DangerousNotAllowed {
        /// Name of the opt-in env var that must be set.
        env_var: &'static str,
    },

    /// A configured [`BudgetTracker`](crate::budget::BudgetTracker) has
    /// hit its `max_usd` ceiling. Raised before the next call is
    /// dispatched, so the CLI is not invoked.
    #[error("budget exceeded: ${total_usd:.4} spent, ${max_usd:.4} max")]
    BudgetExceeded {
        /// Total spend accumulated so far, in USD.
        total_usd: f64,
        /// The configured ceiling, in USD.
        max_usd: f64,
    },

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
    /// under `~/.claude/agents/`, `~/.claude/skills/`, and friends)
    /// failed in a way that doesn't fit the I/O variant -- e.g.
    /// unknown agent/skill name, missing user home directory.
    #[error("artifacts error: {message}")]
    Artifacts {
        /// Human-readable description of what went wrong.
        message: String,
    },

    /// A worktrees-module operation (running or parsing
    /// `git worktree list --porcelain`) failed in a way that
    /// doesn't fit the I/O variant -- e.g. git not on PATH,
    /// path isn't a git repo, malformed porcelain output.
    #[error("worktrees error: {message}")]
    Worktrees {
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

    /// A `--max-turns`-capped run exhausted its turn budget. The CLI
    /// emits a terminal `result` event with `subtype ==
    /// "error_max_turns"` (exit 1, with the result JSON on stdout),
    /// which would otherwise fold into [`Error::CommandFailed`].
    ///
    /// This is distinct from a genuine failure: the working tree may
    /// be fine and the run simply hit the cap mid-task. Orchestrators
    /// can match this variant to finish the lifecycle (run remaining
    /// gates, commit) rather than treating it as broken or re-parsing
    /// the trace for `error_max_turns`.
    ///
    /// Raised by [`Error::from_command_failure`] ahead of the auth
    /// classifier. Only detected when the result event is present on
    /// stdout (the `json` / `stream-json` output formats); text-mode
    /// failures without it remain [`Error::CommandFailed`].
    ///
    /// This variant is `#[non_exhaustive]`: match with `..` so future
    /// field additions are not breaking.
    #[error("claude hit the --max-turns cap{}: {command} (exit code {exit_code})", max_turns.map(|n| format!(" of {n}")).unwrap_or_default())]
    #[non_exhaustive]
    MaxTurnsExceeded {
        /// The full command line that failed.
        command: String,
        /// Process exit code (1).
        exit_code: i32,
        /// The configured `--max-turns` cap, parsed from the result
        /// event ("Reached maximum number of turns (N)") when present.
        max_turns: Option<u32>,
        /// Actual spend, from the result event's `total_cost_usd`
        /// when present.
        cost_usd: Option<f64>,
        /// Turns completed before the cap, from the result event's
        /// `num_turns` when present.
        num_turns: Option<u32>,
        /// Session id from the result event when present; usable to
        /// resume the capped run.
        session_id: Option<String>,
    },

    /// A `--max-budget-usd`-capped run hit its spend ceiling. The CLI
    /// emits a terminal `result` event with `subtype ==
    /// "error_max_budget_usd"` (exit 1, with the result JSON on
    /// stdout), which would otherwise fold into
    /// [`Error::CommandFailed`].
    ///
    /// This is distinct from a genuine failure: the working tree may
    /// be fine and the run simply hit the cap mid-task. Orchestrators
    /// can match this variant to finish the lifecycle (run remaining
    /// gates, commit) rather than treating it as broken or re-parsing
    /// the trace for `error_max_budget_usd`.
    ///
    /// The `max_usd` is claude's reported cap, not the actual spend.
    /// Detection is post-hoc (claude checks the budget after each API
    /// call completes), so a run can overspend the cap before tripping.
    ///
    /// Raised by [`Error::from_command_failure`] ahead of the auth
    /// classifier. Only detected when the result event is present on
    /// stdout (the `json` / `stream-json` output formats); text-mode
    /// failures without it remain [`Error::CommandFailed`].
    ///
    /// This is separate from [`Error::BudgetExceeded`], which is the
    /// wrapper's own [`BudgetTracker`](crate::budget::BudgetTracker)
    /// ceiling -- a different mechanism from claude's CLI cap.
    ///
    /// This variant is `#[non_exhaustive]`: match with `..` so future
    /// field additions are not breaking.
    #[error("claude hit the --max-budget-usd cap{}: {command} (exit code {exit_code})", max_usd.map(|n| format!(" of ${n:.2}")).unwrap_or_default())]
    #[non_exhaustive]
    MaxBudgetExceeded {
        /// The full command line that failed.
        command: String,
        /// Process exit code (1).
        exit_code: i32,
        /// The configured `--max-budget-usd` cap, parsed from the
        /// result event ("Reached maximum budget ($X)") when present.
        max_usd: Option<f64>,
        /// Actual spend, from the result event's `total_cost_usd`
        /// when present.
        cost_usd: Option<f64>,
        /// Turns completed before the cap, from the result event's
        /// `num_turns` when present.
        num_turns: Option<u32>,
        /// Session id from the result event when present; usable to
        /// resume the capped run.
        session_id: Option<String>,
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
        // A --max-turns cap hit is a terminal `result` event with
        // subtype "error_max_turns" on stdout. Surface it as its own
        // typed variant -- ahead of the auth classifier, since it is
        // never auth-shaped -- so consumers can tell "hit the cap"
        // (recoverable) from a genuine failure.
        if stdout.contains("\"error_max_turns\"") {
            return Self::MaxTurnsExceeded {
                command,
                exit_code,
                max_turns: parse_max_turns_cap(&stdout),
                cost_usd: parse_result_number(&stdout, "total_cost_usd"),
                num_turns: parse_result_number(&stdout, "num_turns"),
                session_id: parse_result_string(&stdout, "session_id"),
            };
        }
        // A --max-budget-usd cap hit mirrors the max-turns shape: a
        // terminal `result` event with subtype "error_max_budget_usd"
        // on stdout. Surface it as its own typed variant -- ahead of
        // the auth classifier, since it is never auth-shaped -- so
        // consumers can tell "hit the cap" (recoverable) from a genuine
        // failure.
        if stdout.contains("\"error_max_budget_usd\"") {
            return Self::MaxBudgetExceeded {
                command,
                exit_code,
                max_usd: parse_max_budget_cap(&stdout),
                cost_usd: parse_result_number(&stdout, "total_cost_usd"),
                num_turns: parse_result_number(&stdout, "num_turns"),
                session_id: parse_result_string(&stdout, "session_id"),
            };
        }
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

/// Parse the configured `--max-turns` cap from a CLI result event's
/// human-readable error ("Reached maximum number of turns (N)").
/// Returns `None` when the phrase or a parseable number is absent.
fn parse_max_turns_cap(stdout: &str) -> Option<u32> {
    stdout
        .split("maximum number of turns (")
        .nth(1)
        .and_then(|rest| rest.split(')').next())
        .and_then(|n| n.trim().parse::<u32>().ok())
}

/// Parse the configured `--max-budget-usd` cap from a CLI result
/// event's human-readable error ("Reached maximum budget ($X)").
/// Returns `None` when the phrase or a parseable amount is absent.
fn parse_max_budget_cap(stdout: &str) -> Option<f64> {
    stdout
        .split("maximum budget ($")
        .nth(1)
        .and_then(|rest| rest.split(')').next())
        .and_then(|n| n.trim().parse::<f64>().ok())
}

/// Extract a top-level numeric field (e.g. `"num_turns":2`) from a
/// result event's raw JSON. String-based, like the cap parsers above:
/// `serde_json` is an optional dependency and this module must work
/// without it. Returns `None` when the field or a parseable value is
/// absent.
fn parse_result_number<T: std::str::FromStr>(stdout: &str, field: &str) -> Option<T> {
    let rest = stdout.split(&format!("\"{field}\":")).nth(1)?;
    let end = rest.find([',', '}']).unwrap_or(rest.len());
    rest[..end].trim().parse::<T>().ok()
}

/// Extract a top-level string field (e.g. `"session_id":"abc"`) from
/// a result event's raw JSON. Assumes the value contains no escaped
/// quotes, which holds for session ids. Returns `None` when the field
/// is absent or not a string.
fn parse_result_string(stdout: &str, field: &str) -> Option<String> {
    let rest = stdout.split(&format!("\"{field}\":")).nth(1)?;
    let rest = rest.trim_start().strip_prefix('"')?;
    rest.split('"').next().map(str::to_string)
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

    // -- max-turns classification (#641) ----------------------------

    // Exact shape of a --max-turns cap-hit result event, from the
    // field (claude 2.1.173, --output-format json).
    const MAX_TURNS_STDOUT: &str = r#"{"type":"result","subtype":"error_max_turns","is_error":true,"num_turns":2,"session_id":"s1","total_cost_usd":0.08,"terminal_reason":"max_turns","errors":["Reached maximum number of turns (1)"]}"#;

    #[test]
    fn from_command_failure_max_turns_yields_typed_variant() {
        let e = Error::from_command_failure(
            "claude --print --max-turns 1".into(),
            1,
            MAX_TURNS_STDOUT.into(),
            String::new(),
            None,
        );
        match e {
            Error::MaxTurnsExceeded {
                max_turns,
                exit_code,
                cost_usd,
                num_turns,
                session_id,
                ..
            } => {
                assert_eq!(max_turns, Some(1));
                assert_eq!(exit_code, 1);
                assert_eq!(cost_usd, Some(0.08));
                assert_eq!(num_turns, Some(2));
                assert_eq!(session_id.as_deref(), Some("s1"));
            }
            other => panic!("expected MaxTurnsExceeded, got {other:?}"),
        }
    }

    #[test]
    fn max_turns_detected_without_parseable_cap() {
        let stdout = r#"{"type":"result","subtype":"error_max_turns","is_error":true}"#;
        let e = Error::from_command_failure("c".into(), 1, stdout.into(), String::new(), None);
        match e {
            Error::MaxTurnsExceeded {
                max_turns,
                cost_usd,
                num_turns,
                session_id,
                ..
            } => {
                assert_eq!(max_turns, None);
                assert_eq!(cost_usd, None);
                assert_eq!(num_turns, None);
                assert_eq!(session_id, None);
            }
            other => panic!("expected MaxTurnsExceeded, got {other:?}"),
        }
    }

    #[test]
    fn non_max_turns_failure_stays_command_failed() {
        let e =
            Error::from_command_failure("c".into(), 1, "other output".into(), "boom".into(), None);
        assert!(matches!(e, Error::CommandFailed { .. }));
    }

    #[test]
    fn max_turns_check_does_not_swallow_auth() {
        // A genuine auth failure (no error_max_turns) still classifies
        // as Auth -- the max-turns guard precedes but doesn't shadow it.
        let e = Error::from_command_failure(
            "c".into(),
            1,
            String::new(),
            "Not authenticated. Run `claude login`.".into(),
            None,
        );
        assert!(matches!(e, Error::Auth { .. }));
    }

    #[test]
    fn parse_max_turns_cap_variants() {
        assert_eq!(
            parse_max_turns_cap("Reached maximum number of turns (3)"),
            Some(3)
        );
        assert_eq!(parse_max_turns_cap(MAX_TURNS_STDOUT), Some(1));
        assert_eq!(parse_max_turns_cap("no such phrase"), None);
        assert_eq!(parse_max_turns_cap("maximum number of turns (nope)"), None);
    }

    #[test]
    fn max_turns_display_includes_cap() {
        let s = Error::MaxTurnsExceeded {
            command: "claude --print".into(),
            exit_code: 1,
            max_turns: Some(5),
            cost_usd: None,
            num_turns: None,
            session_id: None,
        }
        .to_string();
        assert!(s.contains("--max-turns"), "got: {s}");
        assert!(s.contains("of 5"), "got: {s}");
    }

    // -- max-budget-usd classification (#664) -----------------------

    // Shape of a --max-budget-usd cap-hit result event, from the field
    // (claude 2.1.186, --output-format stream-json). The cap was $0.01
    // but actual spend was $0.127 -- detection is post-hoc, so `max_usd`
    // reports the cap and `cost_usd` the spend.
    const MAX_BUDGET_STDOUT: &str = r#"{"type":"result","subtype":"error_max_budget_usd","is_error":true,"errors":["Reached maximum budget ($0.01)"],"num_turns":1,"total_cost_usd":0.1273986,"modelUsage":{"claude-haiku-4-5":{"costUSD":0.1273986}},"session_id":"s1"}"#;

    #[test]
    fn from_command_failure_max_budget_yields_typed_variant() {
        let e = Error::from_command_failure(
            "claude --print --max-budget-usd 0.01".into(),
            1,
            MAX_BUDGET_STDOUT.into(),
            String::new(),
            None,
        );
        match e {
            Error::MaxBudgetExceeded {
                max_usd,
                exit_code,
                cost_usd,
                num_turns,
                session_id,
                ..
            } => {
                assert_eq!(max_usd, Some(0.01));
                assert_eq!(exit_code, 1);
                assert_eq!(cost_usd, Some(0.1273986));
                assert_eq!(num_turns, Some(1));
                assert_eq!(session_id.as_deref(), Some("s1"));
            }
            other => panic!("expected MaxBudgetExceeded, got {other:?}"),
        }
    }

    #[test]
    fn max_budget_detected_without_parseable_cap() {
        let stdout = r#"{"type":"result","subtype":"error_max_budget_usd","is_error":true}"#;
        let e = Error::from_command_failure("c".into(), 1, stdout.into(), String::new(), None);
        match e {
            Error::MaxBudgetExceeded {
                max_usd,
                cost_usd,
                num_turns,
                session_id,
                ..
            } => {
                assert_eq!(max_usd, None);
                assert_eq!(cost_usd, None);
                assert_eq!(num_turns, None);
                assert_eq!(session_id, None);
            }
            other => panic!("expected MaxBudgetExceeded, got {other:?}"),
        }
    }

    #[test]
    fn non_max_budget_failure_stays_command_failed() {
        let e =
            Error::from_command_failure("c".into(), 1, "other output".into(), "boom".into(), None);
        assert!(matches!(e, Error::CommandFailed { .. }));
    }

    #[test]
    fn max_budget_check_does_not_swallow_auth() {
        // A genuine auth failure (no error_max_budget_usd) still
        // classifies as Auth -- the budget guard precedes but doesn't
        // shadow it.
        let e = Error::from_command_failure(
            "c".into(),
            1,
            String::new(),
            "Not authenticated. Run `claude login`.".into(),
            None,
        );
        assert!(matches!(e, Error::Auth { .. }));
    }

    #[test]
    fn parse_max_budget_cap_variants() {
        assert_eq!(
            parse_max_budget_cap("Reached maximum budget ($0.01)"),
            Some(0.01)
        );
        assert_eq!(parse_max_budget_cap(MAX_BUDGET_STDOUT), Some(0.01));
        assert_eq!(
            parse_max_budget_cap("Reached maximum budget ($5)"),
            Some(5.0)
        );
        assert_eq!(parse_max_budget_cap("no such phrase"), None);
        assert_eq!(parse_max_budget_cap("maximum budget ($nope)"), None);
    }

    #[test]
    fn max_budget_display_includes_cap() {
        let s = Error::MaxBudgetExceeded {
            command: "claude --print".into(),
            exit_code: 1,
            max_usd: Some(0.01),
            cost_usd: None,
            num_turns: None,
            session_id: None,
        }
        .to_string();
        assert!(s.contains("--max-budget-usd"), "got: {s}");
        assert!(s.contains("of $0.01"), "got: {s}");
    }

    // -- result-event spend-field extraction (#668) ------------------

    #[test]
    fn parse_result_number_variants() {
        assert_eq!(
            parse_result_number::<f64>(MAX_TURNS_STDOUT, "total_cost_usd"),
            Some(0.08)
        );
        assert_eq!(
            parse_result_number::<u32>(MAX_TURNS_STDOUT, "num_turns"),
            Some(2)
        );
        // Terminal field (closed by `}` rather than `,`).
        assert_eq!(
            parse_result_number::<u32>(r#"{"num_turns":3}"#, "num_turns"),
            Some(3)
        );
        assert_eq!(
            parse_result_number::<f64>("no json here", "total_cost_usd"),
            None
        );
        assert_eq!(
            parse_result_number::<u32>(r#"{"num_turns":"nope"}"#, "num_turns"),
            None
        );
    }

    #[test]
    fn parse_result_string_variants() {
        assert_eq!(
            parse_result_string(MAX_TURNS_STDOUT, "session_id").as_deref(),
            Some("s1")
        );
        assert_eq!(parse_result_string("no json here", "session_id"), None);
        // A non-string value is not misread as a string.
        assert_eq!(
            parse_result_string(r#"{"session_id":42}"#, "session_id"),
            None
        );
    }
}
