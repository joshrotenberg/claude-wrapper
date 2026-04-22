use std::path::PathBuf;

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
}
