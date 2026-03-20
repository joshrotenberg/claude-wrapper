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

impl StageRecord {
    /// Human-readable name.
    pub fn kind_name(&self) -> &'static str {
        match self.kind {
            StageKind::Triage => "triage",
            StageKind::Clarify => "clarify",
            StageKind::Plan => "plan",
            StageKind::Implement => "implement",
            StageKind::Test => "test",
            StageKind::Review => "review",
            StageKind::OpenPr => "open_pr",
            StageKind::RevisePr => "revise_pr",
            StageKind::FixCi => "fix_ci",
            StageKind::Merge => "merge",
            StageKind::Research => "research",
            StageKind::Comment => "comment",
        }
    }
}

impl RunRecord {
    /// Human-readable status text.
    pub fn status_text(&self) -> &'static str {
        match self.status {
            RunStatus::Queued => "queued",
            RunStatus::Leased => "leased",
            RunStatus::Running => "running",
            RunStatus::Succeeded => "succeeded",
            RunStatus::Failed => "failed",
            RunStatus::Canceled => "canceled",
            RunStatus::Abandoned => "abandoned",
        }
    }

    /// Create a new run record.
    pub fn new(repo: &str, issue_number: u64, workflow: &str, branch: &str) -> Self {
        Self {
            run_id: generate_run_id(),
            repo: repo.to_string(),
            issue_number,
            status: RunStatus::Running,
            workflow: workflow.to_string(),
            branch: branch.to_string(),
            pr_number: None,
            stages: Vec::new(),
            started_at: chrono::Utc::now(),
            completed_at: None,
            total_cost_usd: None,
        }
    }

    /// Record a stage result.
    pub fn record_stage(&mut self, kind: StageKind, status: StageStatus, result: StageResult) {
        // Update or append.
        if let Some(existing) = self.stages.iter_mut().find(|s| s.kind == kind) {
            existing.status = status;
            existing.attempts += 1;
            existing.result = Some(result);
        } else {
            self.stages.push(StageRecord {
                kind,
                status,
                attempts: 1,
                result: Some(result),
            });
        }
    }

    /// Mark the run as finished.
    pub fn finish(&mut self, status: RunStatus) {
        self.status = status;
        self.completed_at = Some(chrono::Utc::now());
        self.total_cost_usd = {
            let costs: Vec<f64> = self
                .stages
                .iter()
                .filter_map(|s| s.result.as_ref())
                .filter_map(|r| r.cost_usd)
                .collect();
            if costs.is_empty() {
                None
            } else {
                Some(costs.iter().sum())
            }
        };
    }
}

/// Save a run record to disk.
pub fn save_run(record: &RunRecord, state_dir: &std::path::Path) -> crate::error::Result<()> {
    std::fs::create_dir_all(state_dir)?;
    let path = state_dir.join(format!("{}.json", record.run_id));
    let json = serde_json::to_string_pretty(record)?;
    std::fs::write(&path, json)?;

    // Update latest pointer.
    let latest = state_dir.join("latest");
    std::fs::write(latest, &record.run_id)?;

    Ok(())
}

/// Load the most recent run.
pub fn load_latest(state_dir: &std::path::Path) -> Option<RunRecord> {
    let latest = state_dir.join("latest");
    let run_id = std::fs::read_to_string(latest).ok()?;
    load_run(run_id.trim(), state_dir)
}

/// Load a specific run by ID.
pub fn load_run(run_id: &str, state_dir: &std::path::Path) -> Option<RunRecord> {
    let path = state_dir.join(format!("{run_id}.json"));
    let content = std::fs::read_to_string(path).ok()?;
    serde_json::from_str(&content).ok()
}

fn generate_run_id() -> String {
    let now = chrono::Utc::now();
    let timestamp = now.format("%Y%m%d-%H%M%S");
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos() as u64;
    let suffix = (nanos ^ (nanos >> 24)) & 0xFFFF_FFFF;
    format!("run-{timestamp}-{suffix:08x}")
}
