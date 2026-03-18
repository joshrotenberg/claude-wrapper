//! Run state — structured record of execution results.
//!
//! After each `claudes run`, a state file is written to `.claudes/state.json`.
//! This provides a terraform-style inspectable record of what happened:
//! which tasks ran, their status, duration, cost, working directories, and output.
//!
//! `claudes status` reads this file and displays it.

use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::manifest::Manifest;
use crate::runner::RunResult;

/// The default state directory, relative to the project root.
pub const STATE_DIR: &str = ".claudes";

/// The state file name within the state directory.
pub const STATE_FILE: &str = "state.json";

/// Persisted state from the most recent run.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunState {
    /// Schema version.
    pub version: u32,

    /// When this run started.
    pub started_at: DateTime<Utc>,

    /// When this run completed.
    pub completed_at: DateTime<Utc>,

    /// The manifest that was executed.
    pub manifest: Manifest,

    /// Per-task results.
    pub results: Vec<TaskState>,

    /// Summary statistics.
    pub summary: RunSummary,
}

/// Persisted result for a single task.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskState {
    /// Task name.
    pub name: String,

    /// Whether the task succeeded.
    pub status: TaskStatus,

    /// Duration in seconds.
    pub duration_secs: f64,

    /// Working directory where the task ran.
    pub work_dir: String,

    /// Git branch (if any).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub branch: Option<String>,

    /// Cost in USD (parsed from claude output, if available).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cost_usd: Option<f64>,

    /// Session ID (parsed from claude output, if available).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,

    /// Result text (parsed from JSON output, if available).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result_text: Option<String>,

    /// Error message (if task failed).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// Task completion status.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TaskStatus {
    /// Task completed successfully.
    Success,
    /// Task failed.
    Failed,
}

/// Summary statistics for the entire run.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunSummary {
    /// Total number of tasks.
    pub total: usize,
    /// Number of successful tasks.
    pub succeeded: usize,
    /// Number of failed tasks.
    pub failed: usize,
    /// Wall-clock duration in seconds (max of all tasks).
    pub wall_time_secs: f64,
    /// Total cost across all tasks (if available).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub total_cost_usd: Option<f64>,
}

/// Build a `RunState` from a manifest and run result.
pub fn build_state(manifest: &Manifest, result: &RunResult, started_at: DateTime<Utc>) -> RunState {
    let results: Vec<TaskState> = result
        .tasks
        .iter()
        .map(|t| {
            // Try to parse cost/session/result from the JSON output.
            let (cost_usd, session_id, result_text) = parse_task_output(&t.stdout);

            // Find the matching manifest task for branch info.
            let branch = manifest
                .tasks
                .iter()
                .find(|mt| mt.name == t.name)
                .and_then(|mt| mt.branch.clone());

            let error = if t.success {
                None
            } else {
                Some(t.stderr.clone()).filter(|s| !s.is_empty())
            };

            TaskState {
                name: t.name.clone(),
                status: if t.success {
                    TaskStatus::Success
                } else {
                    TaskStatus::Failed
                },
                duration_secs: t.duration.as_secs_f64(),
                work_dir: t.work_dir.to_string_lossy().to_string(),
                branch,
                cost_usd,
                session_id,
                result_text,
                error,
            }
        })
        .collect();

    let total_cost: Option<f64> = {
        let costs: Vec<f64> = results.iter().filter_map(|r| r.cost_usd).collect();
        if costs.is_empty() {
            None
        } else {
            Some(costs.iter().sum())
        }
    };

    let wall_time = result
        .tasks
        .iter()
        .map(|t| t.duration.as_secs_f64())
        .max_by(|a, b| a.partial_cmp(b).unwrap())
        .unwrap_or(0.0);

    let summary = RunSummary {
        total: result.tasks.len(),
        succeeded: result.success_count(),
        failed: result.tasks.len() - result.success_count(),
        wall_time_secs: wall_time,
        total_cost_usd: total_cost,
    };

    RunState {
        version: 1,
        started_at,
        completed_at: Utc::now(),
        manifest: manifest.clone(),
        results,
        summary,
    }
}

/// Save state to the project's `.claudes/state.json`.
pub fn save(project_dir: &Path, state: &RunState) -> std::io::Result<PathBuf> {
    let state_dir = project_dir.join(STATE_DIR);
    std::fs::create_dir_all(&state_dir)?;

    let state_path = state_dir.join(STATE_FILE);
    let json = serde_json::to_string_pretty(state)
        .map_err(|e| std::io::Error::other(format!("json error: {e}")))?;
    std::fs::write(&state_path, json)?;

    Ok(state_path)
}

/// Load state from the project's `.claudes/state.json`.
pub fn load(project_dir: &Path) -> Option<RunState> {
    let state_path = project_dir.join(STATE_DIR).join(STATE_FILE);
    let content = std::fs::read_to_string(&state_path).ok()?;
    serde_json::from_str(&content).ok()
}

/// Try to parse cost, session_id, and result text from claude JSON output.
fn parse_task_output(stdout: &str) -> (Option<f64>, Option<String>, Option<String>) {
    let Ok(v) = serde_json::from_str::<serde_json::Value>(stdout) else {
        return (None, None, None);
    };

    let cost = v
        .get("total_cost_usd")
        .or_else(|| v.get("cost_usd"))
        .and_then(|c| c.as_f64());

    let session_id = v
        .get("session_id")
        .and_then(|s| s.as_str())
        .map(String::from);

    let result_text = v.get("result").and_then(|r| r.as_str()).map(String::from);

    (cost, session_id, result_text)
}

/// Print state as a human-readable table.
pub fn print_status(state: &RunState) {
    println!(
        "Run: {} -> {}",
        state.started_at.format("%Y-%m-%d %H:%M:%S UTC"),
        state.completed_at.format("%H:%M:%S UTC"),
    );
    println!(
        "Tasks: {}/{} succeeded | Wall time: {:.1}s",
        state.summary.succeeded, state.summary.total, state.summary.wall_time_secs,
    );
    if let Some(cost) = state.summary.total_cost_usd {
        println!("Cost: ${cost:.4}");
    }
    println!();
    let header_sep = "-".repeat(80);
    println!(
        "  {:<30} {:<10} {:>8}  {:>8}  Branch",
        "Task", "Status", "Time", "Cost"
    );
    println!("  {header_sep}");

    for task in &state.results {
        let status = match task.status {
            TaskStatus::Success => "ok",
            TaskStatus::Failed => "FAILED",
        };
        let cost = task
            .cost_usd
            .map(|c| format!("${c:.4}"))
            .unwrap_or_default();
        let branch = task.branch.as_deref().unwrap_or("-");
        let time = format!("{:.1}s", task.duration_secs);

        let name = &task.name;
        println!("  {name:<30} {status:<10} {time:>8}  {cost:>8}  {branch}");

        if let Some(err) = &task.error {
            for line in err.lines().take(3) {
                println!("    {line}");
            }
        }
    }
}

/// Print state as JSON.
pub fn print_status_json(state: &RunState) {
    println!(
        "{}",
        serde_json::to_string_pretty(state).unwrap_or_default()
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::manifest::{Isolation, Task};
    use crate::runner::TaskResult;
    use std::time::Duration;

    #[test]
    fn build_state_from_results() {
        let manifest = Manifest::new(vec![{
            let mut t = Task::new("test-task", "do something");
            t.branch = Some("claudes/test-task".into());
            t.isolation = Some(Isolation::None);
            t
        }]);

        let result = RunResult {
            tasks: vec![TaskResult {
                name: "test-task".into(),
                success: true,
                stdout: r#"{"result":"done","session_id":"sess-123","total_cost_usd":0.05}"#.into(),
                stderr: String::new(),
                duration: Duration::from_secs(5),
                work_dir: PathBuf::from("/tmp/test"),
            }],
        };

        let started = Utc::now();
        let state = build_state(&manifest, &result, started);

        assert_eq!(state.summary.total, 1);
        assert_eq!(state.summary.succeeded, 1);
        assert_eq!(state.summary.failed, 0);
        assert_eq!(state.results[0].status, TaskStatus::Success);
        assert_eq!(state.results[0].cost_usd, Some(0.05));
        assert_eq!(state.results[0].session_id.as_deref(), Some("sess-123"));
        assert_eq!(state.results[0].result_text.as_deref(), Some("done"));
        assert_eq!(
            state.results[0].branch.as_deref(),
            Some("claudes/test-task")
        );
    }

    #[test]
    fn build_state_with_failure() {
        let manifest = Manifest::new(vec![
            Task::new("ok-task", "succeeds"),
            Task::new("bad-task", "fails"),
        ]);

        let result = RunResult {
            tasks: vec![
                TaskResult {
                    name: "ok-task".into(),
                    success: true,
                    stdout: "{}".into(),
                    stderr: String::new(),
                    duration: Duration::from_secs(3),
                    work_dir: PathBuf::from("/tmp/ok"),
                },
                TaskResult {
                    name: "bad-task".into(),
                    success: false,
                    stdout: String::new(),
                    stderr: "something went wrong".into(),
                    duration: Duration::from_secs(1),
                    work_dir: PathBuf::from("/tmp/bad"),
                },
            ],
        };

        let state = build_state(&manifest, &result, Utc::now());
        assert_eq!(state.summary.succeeded, 1);
        assert_eq!(state.summary.failed, 1);
        assert_eq!(state.results[1].status, TaskStatus::Failed);
        assert_eq!(
            state.results[1].error.as_deref(),
            Some("something went wrong")
        );
    }

    #[test]
    fn state_json_roundtrip() {
        let manifest = Manifest::new(vec![Task::new("t", "p")]);
        let result = RunResult {
            tasks: vec![TaskResult {
                name: "t".into(),
                success: true,
                stdout: "{}".into(),
                stderr: String::new(),
                duration: Duration::from_secs(2),
                work_dir: PathBuf::from("/tmp"),
            }],
        };

        let state = build_state(&manifest, &result, Utc::now());
        let json = serde_json::to_string_pretty(&state).unwrap();
        let parsed: RunState = serde_json::from_str(&json).unwrap();

        assert_eq!(parsed.summary.total, 1);
        assert_eq!(parsed.results[0].name, "t");
    }

    #[test]
    fn parse_output_extracts_fields() {
        let stdout = r#"{"result":"hello","session_id":"s1","total_cost_usd":0.123,"num_turns":3}"#;
        let (cost, session, result) = parse_task_output(stdout);
        assert_eq!(cost, Some(0.123));
        assert_eq!(session.as_deref(), Some("s1"));
        assert_eq!(result.as_deref(), Some("hello"));
    }

    #[test]
    fn parse_output_handles_missing_fields() {
        let (cost, session, result) = parse_task_output("{}");
        assert_eq!(cost, None);
        assert_eq!(session, None);
        assert_eq!(result, None);
    }

    #[test]
    fn parse_output_handles_invalid_json() {
        let (cost, session, result) = parse_task_output("not json");
        assert_eq!(cost, None);
        assert_eq!(session, None);
        assert_eq!(result, None);
    }
}
