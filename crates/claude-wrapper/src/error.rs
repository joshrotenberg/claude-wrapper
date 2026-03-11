use std::path::PathBuf;

/// Errors returned by claude-wrapper operations.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// The `claude` binary was not found in PATH.
    #[error("claude binary not found in PATH")]
    NotFound,

    /// A claude command failed with a non-zero exit code.
    #[error("claude command failed: {command} (exit code {exit_code}){}{}", working_dir.as_ref().map(|d| format!(" (in {})", d.display())).unwrap_or_default(), if stderr.is_empty() { String::new() } else { format!("\nstderr: {stderr}") })]
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
