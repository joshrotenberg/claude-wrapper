/// Errors returned by claude-wrapper operations.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// The `claude` binary was not found in PATH.
    #[error("claude binary not found in PATH")]
    NotFound,

    /// A claude command failed with a non-zero exit code.
    #[error("claude command failed: {command} (exit code {exit_code})")]
    CommandFailed {
        command: String,
        exit_code: i32,
        stdout: String,
        stderr: String,
    },

    /// An I/O error occurred while spawning or communicating with the process.
    #[error("io error: {message}")]
    Io {
        message: String,
        #[source]
        source: std::io::Error,
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
}

impl From<std::io::Error> for Error {
    fn from(e: std::io::Error) -> Self {
        Self::Io {
            message: e.to_string(),
            source: e,
        }
    }
}

/// Result type alias for claude-wrapper operations.
pub type Result<T> = std::result::Result<T, Error>;
