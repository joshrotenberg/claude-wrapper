//! Error types for claude-runner.

use thiserror::Error;

#[derive(Debug, Error)]
pub enum Error {
    #[error("github error: {0}")]
    GitHub(String),

    #[error("policy error: {0}")]
    Policy(String),

    #[error("triage error: {0}")]
    Triage(String),

    #[error("planner error: {0}")]
    Planner(String),

    #[error("executor error: {0}")]
    Executor(String),

    #[error("isolation error: {0}")]
    Isolation(String),

    #[error("state error: {0}")]
    State(String),

    #[error("io error: {0}")]
    Io(#[from] std::io::Error),

    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),
}

pub type Result<T> = std::result::Result<T, Error>;
