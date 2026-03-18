//! Run state — structured record of execution results.
//!
//! After each `claudes run`, a state file is written to `.claudes/runs/<run_id>.json`
//! and the run ID is written to `.claudes/latest`.
//! This provides a terraform-style inspectable record of what happened:
//! which tasks ran, their status, duration, cost, working directories, and output.
//!
//! `claudes status` reads the latest run and displays it.

use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::manifest::{Isolation, Manifest};
use crate::runner::RunResult;

/// The default state directory, relative to the project root.
pub const STATE_DIR: &str = ".claudes";

/// The runs subdirectory within the state directory.
pub const RUNS_SUBDIR: &str = "runs";

/// File holding the most recent run ID.
pub const LATEST_FILE: &str = "latest";

/// Persisted state from a run.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunState {
    /// Schema version.
    pub version: u32,

    /// Unique run identifier (e.g. "run-a3b2f1").
    pub run_id: String,

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

    /// Path to the NDJSON log file for this task.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub log_path: Option<String>,
}

/// Task completion status.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TaskStatus {
    /// Task completed successfully.
    Success,
    /// Task failed.
    Failed,
    /// Task hit the max_turns limit.
    Timeout,
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
    /// Number of timed-out tasks (max_turns exceeded).
    #[serde(default)]
    pub timed_out: usize,
    /// Wall-clock duration in seconds (max of all tasks).
    pub wall_time_secs: f64,
    /// Total cost across all tasks (if available).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub total_cost_usd: Option<f64>,
}

/// Generate a run ID like "run-a3b2f1" (prefix + 6 hex chars from timestamp).
/// Generate a run ID with embedded timestamp for sorting and readability.
///
/// Format: `run-YYYYMMDD-HHMMSS-XXXX` (UTC start time + 4-char random suffix).
/// Sorts lexicographically by start time.
pub fn generate_run_id() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let now = chrono::Utc::now();
    let timestamp = now.format("%Y%m%d-%H%M%S");
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos() as u64;
    let suffix = (nanos ^ (nanos >> 24)) & 0xFFFF;
    format!("run-{timestamp}-{suffix:04x}")
}

/// Build a `RunState` from a manifest and run result.
pub fn build_state(manifest: &Manifest, result: &RunResult, started_at: DateTime<Utc>) -> RunState {
    let results: Vec<TaskState> = result
        .tasks
        .iter()
        .map(|t| {
            // Parse session/result from the JSON output; prefer stream-aggregated cost.
            let (stdout_cost, session_id, result_text) = parse_task_output(&t.stdout);
            let cost_usd = t.cost_usd.or(stdout_cost);

            // Find the matching manifest task for branch and isolation info.
            let task_manifest = manifest.tasks.iter().find(|mt| mt.name == t.name);
            let branch = task_manifest.and_then(|mt| mt.branch.clone());

            // Compute the log path based on isolation type.
            let no_isolation = task_manifest
                .map(|mt| matches!(mt.isolation, Some(Isolation::None) | None))
                .unwrap_or(false);
            let log_path = if no_isolation {
                PathBuf::from(&t.work_dir)
                    .join(".claudes")
                    .join("logs")
                    .join(format!("{}.jsonl", t.name))
            } else {
                PathBuf::from(&t.work_dir)
                    .join(".claudes")
                    .join("run.jsonl")
            };
            let log_path = Some(log_path.to_string_lossy().to_string());

            let status = if t.success {
                TaskStatus::Success
            } else if is_timeout(&t.stdout, &t.stderr) {
                TaskStatus::Timeout
            } else {
                TaskStatus::Failed
            };

            let error = if t.success {
                None
            } else {
                Some(t.stderr.clone()).filter(|s| !s.is_empty())
            };

            TaskState {
                name: t.name.clone(),
                status,
                duration_secs: t.duration.as_secs_f64(),
                work_dir: t.work_dir.to_string_lossy().to_string(),
                branch,
                cost_usd,
                session_id,
                result_text,
                error,
                log_path,
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

    let timed_out = results
        .iter()
        .filter(|r| r.status == TaskStatus::Timeout)
        .count();
    let failed = results
        .iter()
        .filter(|r| r.status == TaskStatus::Failed)
        .count();

    let summary = RunSummary {
        total: result.tasks.len(),
        succeeded: result.success_count(),
        failed,
        timed_out,
        wall_time_secs: wall_time,
        total_cost_usd: total_cost,
    };

    RunState {
        version: 1,
        run_id: generate_run_id(),
        started_at,
        completed_at: Utc::now(),
        manifest: manifest.clone(),
        results,
        summary,
    }
}

/// Save state to `.claudes/runs/<run_id>.json` and update `.claudes/latest`.
pub fn save(project_dir: &Path, state: &RunState) -> std::io::Result<PathBuf> {
    let runs_dir = project_dir.join(STATE_DIR).join(RUNS_SUBDIR);
    std::fs::create_dir_all(&runs_dir)?;

    let run_path = runs_dir.join(format!("{}.json", state.run_id));
    let json = serde_json::to_string_pretty(state)
        .map_err(|e| std::io::Error::other(format!("json error: {e}")))?;
    std::fs::write(&run_path, &json)?;

    let latest_path = project_dir.join(STATE_DIR).join(LATEST_FILE);
    std::fs::write(&latest_path, &state.run_id)?;

    Ok(run_path)
}

/// Load the most recent run by reading `.claudes/latest`.
pub fn load(project_dir: &Path) -> Option<RunState> {
    let latest_path = project_dir.join(STATE_DIR).join(LATEST_FILE);
    let run_id = std::fs::read_to_string(&latest_path).ok()?;
    load_run(project_dir, run_id.trim())
}

/// Load a specific run by ID from `.claudes/runs/<run_id>.json`.
pub fn load_run(project_dir: &Path, run_id: &str) -> Option<RunState> {
    let run_path = project_dir
        .join(STATE_DIR)
        .join(RUNS_SUBDIR)
        .join(format!("{run_id}.json"));
    let content = std::fs::read_to_string(&run_path).ok()?;
    serde_json::from_str(&content).ok()
}

/// List all runs from `.claudes/runs/`, sorted newest first.
pub fn list_runs(project_dir: &Path) -> Vec<RunState> {
    let runs_dir = project_dir.join(STATE_DIR).join(RUNS_SUBDIR);
    let Ok(entries) = std::fs::read_dir(&runs_dir) else {
        return vec![];
    };

    let mut runs: Vec<RunState> = entries
        .flatten()
        .filter(|e| e.path().extension().is_some_and(|ext| ext == "json"))
        .filter_map(|e| {
            let content = std::fs::read_to_string(e.path()).ok()?;
            serde_json::from_str(&content).ok()
        })
        .collect();

    runs.sort_by(|a, b| b.started_at.cmp(&a.started_at));
    runs
}

/// Detect whether a task result represents a timeout (max_turns exceeded).
///
/// Returns true if stderr contains "max_turns" or if the stdout JSON has
/// `subtype == "error_max_turns"`.
pub(crate) fn is_timeout(stdout: &str, stderr: &str) -> bool {
    if stderr.contains("max_turns") {
        return true;
    }
    if let Ok(v) = serde_json::from_str::<serde_json::Value>(stdout)
        && v.get("subtype").and_then(|s| s.as_str()) == Some("error_max_turns")
    {
        return true;
    }
    false
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

/// Print a summary table of all runs.
pub fn print_status_list(runs: &[RunState]) {
    if runs.is_empty() {
        println!("no runs found");
        return;
    }
    let header_sep = "-".repeat(80);
    println!(
        "  {:<14} {:<20} {:>6}  {:>4}  {:>8}  {:>8}",
        "Run ID", "Started", "Tasks", "OK", "Wall", "Cost"
    );
    println!("  {header_sep}");
    for run in runs {
        let cost = run
            .summary
            .total_cost_usd
            .map(|c| format!("${c:.4}"))
            .unwrap_or_default();
        let wall = format!("{:.1}s", run.summary.wall_time_secs);
        let started = run.started_at.format("%Y-%m-%d %H:%M:%S");
        println!(
            "  {:<14} {:<20} {:>6}  {:>4}  {:>8}  {:>8}",
            run.run_id, started, run.summary.total, run.summary.succeeded, wall, cost
        );
    }
}

/// Print state as a human-readable table.
pub fn print_status(state: &RunState) {
    println!(
        "Run: {} | {} -> {}",
        state.run_id,
        state.started_at.format("%Y-%m-%d %H:%M:%S UTC"),
        state.completed_at.format("%H:%M:%S UTC"),
    );
    let mut summary_line = format!(
        "Tasks: {}/{} succeeded | Wall time: {:.1}s",
        state.summary.succeeded, state.summary.total, state.summary.wall_time_secs,
    );
    if state.summary.timed_out > 0 {
        summary_line.push_str(&format!(" | {} timed out", state.summary.timed_out));
    }
    println!("{summary_line}");
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
            TaskStatus::Timeout => "TIMEOUT",
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
        if let Some(ref log_path) = task.log_path
            && std::path::Path::new(log_path).exists()
        {
            println!("    log: {log_path}");
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

    fn simple_result(name: &str, success: bool) -> TaskResult {
        TaskResult {
            name: name.into(),
            success,
            stdout: if success { "{}".into() } else { String::new() },
            stderr: if success {
                String::new()
            } else {
                "error".into()
            },
            duration: Duration::from_secs(1),
            work_dir: PathBuf::from("/tmp"),
            cost_usd: None,
        }
    }

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
                cost_usd: None,
            }],
        };

        let started = Utc::now();
        let state = build_state(&manifest, &result, started);

        assert!(state.run_id.starts_with("run-"));
        assert_eq!(state.run_id.len(), 24);
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
                    cost_usd: None,
                },
                TaskResult {
                    name: "bad-task".into(),
                    success: false,
                    stdout: String::new(),
                    stderr: "something went wrong".into(),
                    duration: Duration::from_secs(1),
                    work_dir: PathBuf::from("/tmp/bad"),
                    cost_usd: None,
                },
            ],
        };

        let state = build_state(&manifest, &result, Utc::now());
        assert!(state.run_id.starts_with("run-"));
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
                cost_usd: None,
            }],
        };

        let state = build_state(&manifest, &result, Utc::now());
        let run_id = state.run_id.clone();
        let json = serde_json::to_string_pretty(&state).unwrap();
        let parsed: RunState = serde_json::from_str(&json).unwrap();

        assert_eq!(parsed.run_id, run_id);
        assert_eq!(parsed.summary.total, 1);
        assert_eq!(parsed.results[0].name, "t");
    }

    #[test]
    fn generate_run_id_format() {
        let id = generate_run_id();
        // Format: run-YYYYMMDD-HHMMSS-XXXX
        assert!(id.starts_with("run-"), "should start with run-: {id}");
        assert!(
            id.len() == "run-YYYYMMDD-HHMMSS-XXXX".len(),
            "unexpected length: {id} ({})",
            id.len()
        );
        // Should contain date-like segments.
        let parts: Vec<&str> = id.splitn(4, '-').collect();
        assert_eq!(parts.len(), 4, "should have 4 parts: {id}");
        assert_eq!(parts[0], "run");
        assert_eq!(parts[1].len(), 8, "date part should be 8 chars: {id}");
        assert_eq!(parts[2].len(), 6, "time part should be 6 chars: {id}");
        assert_eq!(parts[3].len(), 4, "suffix should be 4 hex chars: {id}");
    }

    #[test]
    fn run_ids_sort_chronologically() {
        let id1 = generate_run_id();
        std::thread::sleep(std::time::Duration::from_millis(1100));
        let id2 = generate_run_id();
        assert!(
            id2 > id1,
            "later run ID should sort after earlier: {id1} vs {id2}"
        );
    }

    #[test]
    fn save_and_load_latest() {
        let dir = tempfile::tempdir().unwrap();
        let manifest = Manifest::new(vec![Task::new("t", "p")]);
        let run_result = RunResult {
            tasks: vec![simple_result("t", true)],
        };
        let mut state = build_state(&manifest, &run_result, Utc::now());
        state.run_id = "run-aaaaaa".to_string();

        save(dir.path(), &state).unwrap();

        let loaded = load(dir.path()).unwrap();
        assert_eq!(loaded.run_id, "run-aaaaaa");
        assert_eq!(loaded.summary.total, 1);
    }

    #[test]
    fn load_specific_run() {
        let dir = tempfile::tempdir().unwrap();
        let manifest = Manifest::new(vec![Task::new("t", "p")]);
        let run_result = RunResult {
            tasks: vec![simple_result("t", true)],
        };
        let mut state = build_state(&manifest, &run_result, Utc::now());
        state.run_id = "run-bbbbbb".to_string();

        save(dir.path(), &state).unwrap();

        let loaded = load_run(dir.path(), "run-bbbbbb").unwrap();
        assert_eq!(loaded.run_id, "run-bbbbbb");
        assert!(load_run(dir.path(), "run-000000").is_none());
    }

    #[test]
    fn load_returns_none_when_no_state() {
        let dir = tempfile::tempdir().unwrap();
        assert!(load(dir.path()).is_none());
    }

    #[test]
    fn list_runs_sorted_newest_first() {
        let dir = tempfile::tempdir().unwrap();
        let manifest = Manifest::new(vec![Task::new("t", "p")]);
        let run_result = RunResult {
            tasks: vec![simple_result("t", true)],
        };

        let t1: DateTime<Utc> = "2024-01-01T10:00:00Z".parse().unwrap();
        let t2: DateTime<Utc> = "2024-01-01T12:00:00Z".parse().unwrap();

        let mut s1 = build_state(&manifest, &run_result, t1);
        s1.run_id = "run-111111".to_string();
        let mut s2 = build_state(&manifest, &run_result, t2);
        s2.run_id = "run-222222".to_string();

        save(dir.path(), &s1).unwrap();
        save(dir.path(), &s2).unwrap();

        let runs = list_runs(dir.path());
        assert_eq!(runs.len(), 2);
        assert_eq!(runs[0].run_id, "run-222222");
        assert_eq!(runs[1].run_id, "run-111111");
    }

    #[test]
    fn list_runs_empty_when_no_dir() {
        let dir = tempfile::tempdir().unwrap();
        assert!(list_runs(dir.path()).is_empty());
    }

    #[test]
    fn build_state_detects_timeout() {
        let manifest = Manifest::new(vec![Task::new("timeout-task", "runs too long")]);

        // Detected via stderr containing 'max_turns'.
        let result_stderr = RunResult {
            tasks: vec![TaskResult {
                name: "timeout-task".into(),
                success: false,
                stdout: String::new(),
                stderr: "reached max_turns limit".into(),
                duration: Duration::from_secs(60),
                work_dir: PathBuf::from("/tmp"),
                cost_usd: None,
            }],
        };
        let state = build_state(&manifest, &result_stderr, Utc::now());
        assert_eq!(state.results[0].status, TaskStatus::Timeout);
        assert_eq!(state.summary.timed_out, 1);
        assert_eq!(state.summary.failed, 0);

        // Detected via stdout JSON subtype 'error_max_turns'.
        let result_stdout = RunResult {
            tasks: vec![TaskResult {
                name: "timeout-task".into(),
                success: false,
                stdout: r#"{"subtype":"error_max_turns","result":""}"#.into(),
                stderr: String::new(),
                duration: Duration::from_secs(60),
                work_dir: PathBuf::from("/tmp"),
                cost_usd: None,
            }],
        };
        let state2 = build_state(&manifest, &result_stdout, Utc::now());
        assert_eq!(state2.results[0].status, TaskStatus::Timeout);
        assert_eq!(state2.summary.timed_out, 1);
        assert_eq!(state2.summary.failed, 0);
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
