//! Error types for claudes.

/// Errors that can occur during manifest execution.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// Manifest validation failed.
    #[error("invalid manifest: {0}")]
    InvalidManifest(String),

    /// A task failed during execution.
    #[error("task '{name}' failed: {message}")]
    TaskFailed {
        /// Task name.
        name: String,
        /// Error details.
        message: String,
    },

    /// Git worktree operation failed.
    #[error("worktree error: {0}")]
    Worktree(String),

    /// The claude-wrapper library returned an error.
    #[error("claude error: {0}")]
    Claude(#[from] claude_wrapper::Error),

    /// IO error.
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),

    /// JSON serialization/deserialization error.
    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),
}

/// Result type alias for claudes operations.
pub type Result<T> = std::result::Result<T, Error>;
