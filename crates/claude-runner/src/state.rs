//! Run state — persisted records of execution attempts.

use serde::{Deserialize, Serialize};

use crate::executor::StageResult;
use crate::workflow::StageKind;

/// State of an issue in the automation pipeline.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IssueState {
    New,
    Triaging,
    NeedsClarification,
    Ready,
    Planning,
    InProgress,
    WaitingOnReview,
    WaitingOnHuman,
    Blocked,
    Completed,
    ClosedUnresolved,
}

/// State of a single run attempt.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunStatus {
    Queued,
    Leased,
    Running,
    Succeeded,
    Failed,
    Canceled,
    Abandoned,
}

/// State of a single stage within a run.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StageStatus {
    Pending,
    Running,
    Succeeded,
    Failed,
    Skipped,
    Waiting,
}

/// A persisted run record.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunRecord {
    /// Stable run identifier.
    pub run_id: String,
    /// Repository.
    pub repo: String,
    /// Issue number.
    pub issue_number: u64,
    /// Overall run status.
    pub status: RunStatus,
    /// Workflow template used.
    pub workflow: String,
    /// Branch name.
    pub branch: String,
    /// PR number if created.
    pub pr_number: Option<u64>,
    /// Per-stage results.
    pub stages: Vec<StageRecord>,
    /// When the run started.
    pub started_at: chrono::DateTime<chrono::Utc>,
    /// When the run completed.
    pub completed_at: Option<chrono::DateTime<chrono::Utc>>,
    /// Total cost across all stages.
    pub total_cost_usd: Option<f64>,
}

/// Per-stage record within a run.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StageRecord {
    /// Stage kind.
    pub kind: StageKind,
    /// Stage status.
    pub status: StageStatus,
    /// Attempt count.
    pub attempts: u32,
    /// Result from the last attempt (if any).
    pub result: Option<StageResult>,
}
