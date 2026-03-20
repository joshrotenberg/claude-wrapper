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

    /// Record a stage as skipped (no result).
    pub fn record_stage_skipped(&mut self, kind: StageKind) {
        if let Some(existing) = self.stages.iter_mut().find(|s| s.kind == kind) {
            existing.status = StageStatus::Skipped;
        } else {
            self.stages.push(StageRecord {
                kind,
                status: StageStatus::Skipped,
                attempts: 0,
                result: None,
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

/// Cost estimate derived from historical runs of the same workflow type.
#[derive(Debug, Clone)]
pub struct CostEstimate {
    /// Minimum cost seen.
    pub min: f64,
    /// Maximum cost seen.
    pub max: f64,
    /// Average cost.
    pub avg: f64,
    /// Number of historical runs used.
    pub count: usize,
    /// Workflow name.
    pub workflow: String,
}

/// Compute a cost estimate from previous completed runs of the same workflow type.
///
/// Returns `None` if no historical data is available.
pub fn estimate_cost(workflow: &str, state_dir: &std::path::Path) -> Option<CostEstimate> {
    let entries = std::fs::read_dir(state_dir).ok()?;
    let costs: Vec<f64> = entries
        .filter_map(|e| e.ok())
        .filter(|e| e.path().extension().and_then(|x| x.to_str()) == Some("json"))
        .filter_map(|e| {
            let content = std::fs::read_to_string(e.path()).ok()?;
            let record: RunRecord = serde_json::from_str(&content).ok()?;
            if record.workflow == workflow && record.status == RunStatus::Succeeded {
                record.total_cost_usd
            } else {
                None
            }
        })
        .collect();

    if costs.is_empty() {
        return None;
    }

    let min = costs.iter().cloned().fold(f64::INFINITY, f64::min);
    let max = costs.iter().cloned().fold(f64::NEG_INFINITY, f64::max);
    let avg = costs.iter().sum::<f64>() / costs.len() as f64;

    Some(CostEstimate {
        min,
        max,
        avg,
        count: costs.len(),
        workflow: workflow.to_string(),
    })
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

#[cfg(test)]
mod tests {
    use super::*;

    fn make_record(workflow: &str, status: RunStatus, cost: Option<f64>) -> RunRecord {
        RunRecord {
            run_id: generate_run_id(),
            repo: "owner/repo".into(),
            issue_number: 1,
            status,
            workflow: workflow.to_string(),
            branch: "fix/test".into(),
            pr_number: None,
            stages: Vec::new(),
            started_at: chrono::Utc::now(),
            completed_at: None,
            total_cost_usd: cost,
        }
    }

    #[test]
    fn estimate_cost_empty_dir() {
        let dir = tempfile::tempdir().unwrap();
        assert!(estimate_cost("bug", dir.path()).is_none());
    }

    #[test]
    fn estimate_cost_no_matching_workflow() {
        let dir = tempfile::tempdir().unwrap();
        let record = make_record("feature", RunStatus::Succeeded, Some(1.00));
        save_run(&record, dir.path()).unwrap();
        assert!(estimate_cost("bug", dir.path()).is_none());
    }

    #[test]
    fn estimate_cost_excludes_failed_runs() {
        let dir = tempfile::tempdir().unwrap();
        let record = make_record("bug", RunStatus::Failed, Some(0.50));
        save_run(&record, dir.path()).unwrap();
        assert!(estimate_cost("bug", dir.path()).is_none());
    }

    #[test]
    fn estimate_cost_excludes_runs_without_cost() {
        let dir = tempfile::tempdir().unwrap();
        let record = make_record("bug", RunStatus::Succeeded, None);
        save_run(&record, dir.path()).unwrap();
        assert!(estimate_cost("bug", dir.path()).is_none());
    }

    #[test]
    fn estimate_cost_single_run() {
        let dir = tempfile::tempdir().unwrap();
        let record = make_record("bug", RunStatus::Succeeded, Some(1.05));
        save_run(&record, dir.path()).unwrap();
        let est = estimate_cost("bug", dir.path()).unwrap();
        assert_eq!(est.count, 1);
        assert!((est.min - 1.05).abs() < 1e-9);
        assert!((est.max - 1.05).abs() < 1e-9);
        assert!((est.avg - 1.05).abs() < 1e-9);
        assert_eq!(est.workflow, "bug");
    }

    #[test]
    fn estimate_cost_multiple_runs() {
        let dir = tempfile::tempdir().unwrap();
        for cost in [0.70, 1.05, 1.50] {
            // Need unique run IDs — generate_run_id uses nanos so sleep briefly
            std::thread::sleep(std::time::Duration::from_millis(2));
            let record = make_record("bug", RunStatus::Succeeded, Some(cost));
            save_run(&record, dir.path()).unwrap();
        }
        let est = estimate_cost("bug", dir.path()).unwrap();
        assert_eq!(est.count, 3);
        assert!((est.min - 0.70).abs() < 1e-9);
        assert!((est.max - 1.50).abs() < 1e-9);
        assert!((est.avg - 1.0833333333333333).abs() < 1e-9);
    }

    #[test]
    fn estimate_cost_mixed_workflows() {
        let dir = tempfile::tempdir().unwrap();
        std::thread::sleep(std::time::Duration::from_millis(2));
        let r1 = make_record("bug", RunStatus::Succeeded, Some(0.80));
        save_run(&r1, dir.path()).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(2));
        let r2 = make_record("feature", RunStatus::Succeeded, Some(2.00));
        save_run(&r2, dir.path()).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(2));
        let r3 = make_record("bug", RunStatus::Succeeded, Some(1.20));
        save_run(&r3, dir.path()).unwrap();

        let est = estimate_cost("bug", dir.path()).unwrap();
        assert_eq!(est.count, 2);
        assert!((est.min - 0.80).abs() < 1e-9);
        assert!((est.max - 1.20).abs() < 1e-9);
        assert!((est.avg - 1.00).abs() < 1e-9);
    }
}
