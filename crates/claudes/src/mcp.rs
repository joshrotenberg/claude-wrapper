//! MCP tool definitions for claudes.
//!
//! Exposes claudes operations as MCP tools so Claude Code sessions can
//! orchestrate manifest-driven task execution via stdio transport.

use std::path::PathBuf;

use schemars::JsonSchema;
use serde::Deserialize;
use tower_mcp::ToolBuilder;
use tower_mcp::protocol::CallToolResult;
use tower_mcp::tool::Tool;

fn json_result(value: &impl serde::Serialize) -> CallToolResult {
    match serde_json::to_value(value) {
        Ok(v) => CallToolResult::json(v),
        Err(e) => CallToolResult::error(format!("serialization error: {e}")),
    }
}

#[derive(Debug, Deserialize, JsonSchema)]
struct PlanInput {
    /// Task prompts to generate a manifest from (one prompt per task).
    prompts: Vec<String>,
    /// Model override (e.g. "claude-sonnet-4-6").
    model: Option<String>,
    /// Isolation strategy (worktree|clone|none).
    isolation: Option<String>,
    /// Effort level (low|medium|high).
    effort: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct RunManifestInput {
    /// Manifest JSON to execute.
    manifest_json: String,
    /// Force overwrite existing worktrees.
    force: Option<bool>,
    /// Run in background. Returns run_id immediately; poll task_status to check completion.
    background: Option<bool>,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct TaskStatusInput {
    /// Run ID to query (default: latest run).
    run_id: Option<String>,
    /// Return JSON output (always true for MCP, included for API symmetry).
    json: Option<bool>,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct ListRunsInput {}

#[derive(Debug, Deserialize, JsonSchema)]
struct FixInput {
    /// Run ID to fix (default: latest run).
    run_id: Option<String>,
    /// Re-run only these task(s) (default: all failed/timed-out).
    tasks: Option<Vec<String>>,
    /// Additional guidance to append to the fix prompt.
    guidance: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct MetricsInput {
    /// Limit to the last N runs.
    last: Option<usize>,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct CleanInput {
    /// Force remove even with uncommitted changes.
    force: Option<bool>,
    /// Remove run state files from .claudes/runs/.
    runs: Option<bool>,
    /// Remove merged claudes/* branches.
    branches: Option<bool>,
}

/// Build the complete list of MCP tools for claudes.
pub fn tools() -> Vec<Tool> {
    vec![
        plan_tasks(),
        run_manifest(),
        task_status(),
        list_runs(),
        fix_tasks(),
        metrics(),
        clean_tool(),
    ]
}

fn plan_tasks() -> Tool {
    ToolBuilder::new("plan_tasks")
        .title("Plan Tasks")
        .description(
            "Generate a claudes manifest from one or more task prompts without executing. \
             Returns a JSON manifest that can be reviewed, edited, and then passed to run_manifest. \
             Each prompt becomes a separate task. Use this to preview what would run before executing.",
        )
        .read_only_safe()
        .handler(|input: PlanInput| async move {
            let opts = crate::planner::PlanOptions {
                prompts: input.prompts,
                model: input.model,
                isolation: input.isolation,
                effort: input.effort,
                // Headless runs must have a non-default permission mode or edits
                // will be blocked waiting for human approval that never comes.
                permission_mode: Some("bypassPermissions".into()),
                no_session_persistence: Some(true),
                ..Default::default()
            };
            let manifest = crate::plan(&opts);
            Ok(json_result(&manifest))
        })
        .build()
}

fn run_manifest() -> Tool {
    ToolBuilder::new("run_manifest")
        .title("Run Manifest")
        .description(
            "Execute a claudes manifest. Tasks run in parallel in isolated git worktrees. \
             Returns the full run state including per-task status, cost, duration, and errors. \
             The manifest JSON should include version, tasks array, and optionally a shared block. \
             Use plan_tasks first to generate a manifest, then pass it here to execute. \
             Set background=true to return immediately with a run_id; poll task_status to check completion.",
        )
        .handler(|input: RunManifestInput| async move {
            let manifest: crate::Manifest = match serde_json::from_str(&input.manifest_json) {
                Ok(m) => m,
                Err(e) => {
                    return Ok(CallToolResult::error(format!("invalid manifest JSON: {e}")));
                }
            };
            let project_dir = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));

            // Set up event sender so log files get written even without a renderer.
            let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<crate::TaskEvent>();
            // Drain events in background (no rendering, just enables log writing in runner).
            tokio::spawn(async move {
                while rx.recv().await.is_some() {}
            });

            let options = crate::RunOptions {
                project_dir: project_dir.clone(),
                force: input.force.unwrap_or(false),
                binary: None,
                env: vec![],
                cleanup: crate::CleanupPolicy::default(),
                event_sender: Some(tx),
            };

            if input.background.unwrap_or(false) {
                // Background mode: spawn and return run_id immediately.
                let run_id = crate::state::generate_run_id();
                let rid = run_id.clone();
                tokio::spawn(async move {
                    let started_at = chrono::Utc::now();
                    match crate::run(&manifest, &options).await {
                        Ok(result) => {
                            let mut state =
                                crate::state::build_state(&manifest, &result, started_at);
                            state.run_id = rid;
                            if let Err(e) = crate::state::save(&project_dir, &state) {
                                tracing::error!("failed to write state file: {e}");
                            }
                        }
                        Err(e) => {
                            tracing::error!("background run failed: {e}");
                        }
                    }
                });
                return Ok(json_result(&serde_json::json!({
                    "run_id": run_id,
                    "status": "started",
                    "message": "running in background — poll task_status with this run_id to check completion"
                })));
            }

            // Foreground mode: block until complete.
            let started_at = chrono::Utc::now();
            match crate::run(&manifest, &options).await {
                Ok(result) => {
                    let state = crate::state::build_state(&manifest, &result, started_at);
                    if let Err(e) = crate::state::save(&project_dir, &state) {
                        tracing::warn!("failed to write state file: {e}");
                    }
                    Ok(json_result(&state))
                }
                Err(e) => Ok(CallToolResult::error(format!("{e}"))),
            }
        })
        .build()
}

fn task_status() -> Tool {
    ToolBuilder::new("task_status")
        .title("Task Status")
        .description(
            "Get the full status of a claudes run, including per-task results, costs, \
             durations, branches, and errors. Defaults to the most recent run. \
             Pass a run_id to query a specific historical run.",
        )
        .read_only_safe()
        .handler(|input: TaskStatusInput| async move {
            let _ = input.json; // always JSON in MCP context
            let project_dir = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
            let state = if let Some(ref run_id) = input.run_id {
                crate::state::load_run(&project_dir, run_id)
            } else {
                crate::state::load(&project_dir)
            };
            match state {
                Some(s) => Ok(json_result(&s)),
                None => Ok(CallToolResult::error(
                    "no run state found (run `claudes run` first)",
                )),
            }
        })
        .build()
}

fn list_runs() -> Tool {
    ToolBuilder::new("list_runs")
        .title("List Runs")
        .description(
            "List all claudes runs in the current project, sorted newest first. \
             Returns an array of run summaries with run_id, start time, task count, \
             success count, wall time, and cost. Use task_status with a specific \
             run_id to get full details for any run.",
        )
        .read_only_safe()
        .handler(|_input: ListRunsInput| async move {
            let project_dir = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
            let runs = crate::state::list_runs(&project_dir);
            let summaries: Vec<serde_json::Value> = runs
                .iter()
                .map(|r| {
                    serde_json::json!({
                        "run_id": r.run_id,
                        "started_at": r.started_at.to_rfc3339(),
                        "total_tasks": r.summary.total,
                        "succeeded": r.summary.succeeded,
                        "failed": r.summary.failed,
                        "timed_out": r.summary.timed_out,
                        "wall_time_secs": r.summary.wall_time_secs,
                        "total_cost_usd": r.summary.total_cost_usd,
                    })
                })
                .collect();
            Ok(json_result(&serde_json::json!({ "runs": summaries })))
        })
        .build()
}

fn fix_tasks() -> Tool {
    ToolBuilder::new("fix_tasks")
        .title("Fix Failed Tasks")
        .description(
            "Re-run failed or timed-out tasks from a previous run. Enters the existing \
             worktree and spawns a new claude session with the original prompt plus error \
             context. Optionally provide additional guidance. Re-runs post_hooks to verify \
             the fix. Defaults to fixing all failed/timed-out tasks from the latest run.",
        )
        .handler(|input: FixInput| async move {
            let project_dir = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));

            let state = if let Some(ref run_id) = input.run_id {
                crate::state::load_run(&project_dir, run_id)
            } else {
                crate::state::load(&project_dir)
            };

            let state = match state {
                Some(s) => s,
                None => {
                    return Ok(CallToolResult::error(
                        "no run state found (run a manifest first)",
                    ));
                }
            };

            let task_filter = input.tasks.unwrap_or_default();
            let tasks_to_fix: Vec<&crate::state::TaskState> = state
                .results
                .iter()
                .filter(|t| {
                    if task_filter.is_empty() {
                        matches!(
                            t.status,
                            crate::state::TaskStatus::Failed | crate::state::TaskStatus::Timeout
                        )
                    } else {
                        task_filter.contains(&t.name)
                    }
                })
                .collect();

            if tasks_to_fix.is_empty() {
                return Ok(json_result(
                    &serde_json::json!({"message": "no failed or timed-out tasks to fix"}),
                ));
            }

            let mut results: Vec<serde_json::Value> = Vec::new();

            for task_state in tasks_to_fix {
                let original_task = state
                    .manifest
                    .tasks
                    .iter()
                    .find(|t| t.name == task_state.name);

                let original_prompt = original_task
                    .map(|t| t.prompt.as_str())
                    .unwrap_or("[unknown]");

                let error_context = task_state
                    .error
                    .as_deref()
                    .unwrap_or("task timed out or failed with no error output");

                let mut fix_prompt = format!(
                    "The previous task failed. Original prompt: {original_prompt}. \
                     Error: {error_context}. Fix the issue."
                );
                if let Some(ref guidance) = input.guidance {
                    fix_prompt.push_str(&format!(" {guidance}"));
                }

                let mut fix_task = original_task
                    .cloned()
                    .unwrap_or_else(|| crate::Task::new(&task_state.name, ""));
                fix_task.prompt = fix_prompt;
                fix_task.isolation = Some(crate::Isolation::None);

                let fix_manifest = crate::Manifest::new(vec![fix_task]);

                let work_dir = PathBuf::from(&task_state.work_dir);
                if !work_dir.exists() {
                    results.push(serde_json::json!({
                        "task": task_state.name,
                        "status": "error",
                        "message": format!("work_dir no longer exists: {}", task_state.work_dir),
                    }));
                    continue;
                }

                let options = crate::RunOptions {
                    project_dir: work_dir,
                    force: false,
                    binary: None,
                    env: vec![],
                    cleanup: crate::CleanupPolicy::default(),
                    event_sender: None,
                };

                let started_at = chrono::Utc::now();
                match crate::run(&fix_manifest, &options).await {
                    Ok(result) => {
                        let fix_state =
                            crate::state::build_state(&fix_manifest, &result, started_at);
                        if let Err(e) = crate::state::save(&project_dir, &fix_state) {
                            tracing::warn!("failed to write fix state: {e}");
                        }
                        let succeeded = result.all_succeeded();
                        results.push(serde_json::json!({
                            "task": task_state.name,
                            "status": if succeeded { "fixed" } else { "still_failing" },
                        }));
                    }
                    Err(e) => {
                        results.push(serde_json::json!({
                            "task": task_state.name,
                            "status": "error",
                            "message": format!("{e}"),
                        }));
                    }
                }
            }

            Ok(json_result(&serde_json::json!({ "results": results })))
        })
        .build()
}

fn metrics() -> Tool {
    ToolBuilder::new("metrics")
        .title("Run Metrics")
        .description(
            "Aggregate statistics across historical runs: total tasks, success/failure/timeout \
             rates, total and average cost, average duration. Optionally limit to the last N runs.",
        )
        .read_only_safe()
        .handler(|input: MetricsInput| async move {
            let project_dir = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
            let mut runs = crate::state::list_runs(&project_dir);

            if let Some(n) = input.last {
                runs.truncate(n);
            }

            if runs.is_empty() {
                return Ok(CallToolResult::error(
                    "no runs found (run a manifest first)",
                ));
            }

            let m = crate::state::compute_metrics(&runs);
            Ok(json_result(&m))
        })
        .build()
}

fn clean_tool() -> Tool {
    ToolBuilder::new("clean")
        .title("Clean")
        .description(
            "Remove claudes artifacts. By default, removes git worktrees from .worktrees/. \
             Set runs=true to also remove run state files from .claudes/runs/. \
             Set branches=true to also delete local claudes/* branches that have been merged into main. \
             Set force=true to force-remove worktrees even with uncommitted changes.",
        )
        .handler(|input: CleanInput| async move {
            let project_dir = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
            let force = input.force.unwrap_or(false);
            let clean_runs = input.runs.unwrap_or(false);
            let clean_branches = input.branches.unwrap_or(false);
            let mut messages: Vec<String> = Vec::new();

            // Clean worktrees.
            let worktrees_dir = project_dir.join(".worktrees");
            if worktrees_dir.exists() {
                let mut removed = 0usize;
                if let Ok(entries) = std::fs::read_dir(&worktrees_dir) {
                    for entry in entries.flatten() {
                        if !entry.path().is_dir() {
                            continue;
                        }
                        let name = entry.file_name().to_string_lossy().to_string();
                        let path_str = entry.path().to_string_lossy().to_string();
                        let mut cmd_args: Vec<String> = vec!["worktree".into(), "remove".into()];
                        if force {
                            cmd_args.push("--force".into());
                        }
                        cmd_args.push(path_str);
                        let output = tokio::process::Command::new("git")
                            .args(&cmd_args)
                            .current_dir(&project_dir)
                            .output()
                            .await;
                        match output {
                            Ok(o) if o.status.success() => removed += 1,
                            Ok(o) => {
                                let stderr = String::from_utf8_lossy(&o.stderr).to_string();
                                messages
                                    .push(format!("failed to remove worktree {name}: {stderr}"));
                            }
                            Err(e) => {
                                messages.push(format!("failed to remove worktree {name}: {e}"));
                            }
                        }
                    }
                }
                if removed > 0 {
                    let _ = std::fs::remove_dir(&worktrees_dir);
                }
                messages.push(format!("cleaned {removed} worktree(s)"));
            } else {
                messages.push("no worktrees to clean".into());
            }

            // Clean run state files.
            if clean_runs {
                let runs_dir = project_dir.join(".claudes").join("runs");
                let latest_file = project_dir.join(".claudes").join("latest");
                let mut removed = 0usize;
                if runs_dir.exists()
                    && let Ok(entries) = std::fs::read_dir(&runs_dir)
                {
                    for entry in entries.flatten() {
                        if std::fs::remove_file(entry.path()).is_ok() {
                            removed += 1;
                        }
                    }
                }
                let _ = std::fs::remove_file(&latest_file);
                messages.push(format!("cleaned {removed} run state file(s)"));
            }

            // Clean merged claudes/* branches.
            if clean_branches {
                let list_output = tokio::process::Command::new("git")
                    .args(["branch", "--list", "claudes/*"])
                    .current_dir(&project_dir)
                    .output()
                    .await;
                match list_output {
                    Ok(o) => {
                        let branches: Vec<String> = String::from_utf8_lossy(&o.stdout)
                            .lines()
                            .map(|l| l.trim().trim_start_matches("* ").trim().to_string())
                            .filter(|l| !l.is_empty())
                            .collect();
                        if branches.is_empty() {
                            messages.push("no claudes/* branches to clean".into());
                        } else {
                            let merged_output = tokio::process::Command::new("git")
                                .args(["branch", "--merged", "main"])
                                .current_dir(&project_dir)
                                .output()
                                .await;
                            let merged_set: std::collections::HashSet<String> = match merged_output
                            {
                                Ok(o) => String::from_utf8_lossy(&o.stdout)
                                    .lines()
                                    .map(|l| l.trim().trim_start_matches("* ").trim().to_string())
                                    .filter(|l| !l.is_empty())
                                    .collect(),
                                Err(e) => {
                                    messages.push(format!("failed to check merged branches: {e}"));
                                    std::collections::HashSet::new()
                                }
                            };
                            let mut removed = 0usize;
                            for branch in &branches {
                                if !merged_set.contains(branch) {
                                    continue;
                                }
                                let del = tokio::process::Command::new("git")
                                    .args(["branch", "-d", branch])
                                    .current_dir(&project_dir)
                                    .output()
                                    .await;
                                match del {
                                    Ok(o) if o.status.success() => removed += 1,
                                    Ok(o) => {
                                        let stderr = String::from_utf8_lossy(&o.stderr).to_string();
                                        messages.push(format!(
                                            "failed to delete branch {branch}: {stderr}"
                                        ));
                                    }
                                    Err(e) => {
                                        messages
                                            .push(format!("failed to delete branch {branch}: {e}"));
                                    }
                                }
                            }
                            messages.push(format!("deleted {removed} merged claudes/* branch(es)"));
                        }
                    }
                    Err(e) => messages.push(format!("failed to list branches: {e}")),
                }
            }

            Ok(json_result(&serde_json::json!({ "messages": messages })))
        })
        .build()
}
