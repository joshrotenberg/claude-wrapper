//! Error types for claude-pool.

/// Errors that can occur in claude-pool operations.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// A slot with the given ID was not found.
    #[error("slot not found: {0}")]
    SlotNotFound(String),

    /// A task with the given ID was not found.
    #[error("task not found: {0}")]
    TaskNotFound(String),

    /// No slot became available within the timeout period.
    #[error("no slot available after waiting {timeout_secs}s")]
    NoSlotAvailable { timeout_secs: u64 },

    /// The pool has been shut down and is no longer accepting work.
    #[error("pool is shut down")]
    PoolShutdown,

    /// Budget limit has been reached.
    #[error("budget exhausted: spent {spent_microdollars} of {limit_microdollars} microdollars")]
    BudgetExhausted {
        /// Microdollars spent so far.
        spent_microdollars: u64,
        /// Microdollars budget limit.
        limit_microdollars: u64,
    },

    /// An error from the underlying Claude CLI wrapper.
    #[error("claude-wrapper error: {0}")]
    Wrapper(#[from] claude_wrapper::Error),

    /// JSON serialization/deserialization error.
    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),

    /// The Claude CLI appears to have stalled on a permission prompt.
    ///
    /// This occurs when the CLI requests tool approval and no human is present
    /// to respond. The slot blocks on stdin until it times out or is killed.
    #[error(
        "permission prompt detected: {tool_name}. Add it to allowed_tools or use a broader permission mode"
    )]
    PermissionPromptDetected {
        /// The tool or permission that was requested (best-effort extraction).
        tool_name: String,
        /// The raw stderr that triggered detection.
        stderr: String,
        /// The slot that was blocked.
        slot_id: String,
    },

    /// An error from the store backend.
    #[error("store error: {0}")]
    Store(String),
}

/// A convenience type alias for `Result<T, Error>`.
pub type Result<T> = std::result::Result<T, Error>;
