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
        clean_tool(),
    ]
}

fn plan_tasks() -> Tool {
    ToolBuilder::new("plan_tasks")
        .description("Generate a claudes manifest from one or more task prompts without executing.")
        .handler(|input: PlanInput| async move {
            let opts = crate::planner::PlanOptions {
                prompts: input.prompts,
                model: input.model,
                isolation: input.isolation,
                effort: input.effort,
                ..Default::default()
            };
            let manifest = crate::plan(&opts);
            Ok(json_result(&manifest))
        })
        .build()
}

fn run_manifest() -> Tool {
    ToolBuilder::new("run_manifest")
        .description("Execute a claudes manifest JSON and return the run result summary.")
        .handler(|input: RunManifestInput| async move {
            let manifest: crate::Manifest = match serde_json::from_str(&input.manifest_json) {
                Ok(m) => m,
                Err(e) => {
                    return Ok(CallToolResult::error(format!("invalid manifest JSON: {e}")));
                }
            };
            let project_dir = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
            let options = crate::RunOptions {
                project_dir: project_dir.clone(),
                force: input.force.unwrap_or(false),
                binary: None,
                env: vec![],
                cleanup: crate::CleanupPolicy::default(),
                event_sender: None,
            };
            let started_at = chrono::Utc::now();
            match crate::run(&manifest, &options).await {
                Ok(result) => {
                    let state = crate::state::build_state(&manifest, &result, started_at);
                    Ok(json_result(&state))
                }
                Err(e) => Ok(CallToolResult::error(format!("{e}"))),
            }
        })
        .build()
}

fn task_status() -> Tool {
    ToolBuilder::new("task_status")
        .description("Get the status of the most recent claudes run, or a specific run by ID.")
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
        .description("List all claudes runs in the current project directory.")
        .read_only_safe()
        .handler(|_input: ListRunsInput| async move {
            let project_dir = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
            let runs = crate::state::list_runs(&project_dir);
            Ok(json_result(&runs))
        })
        .build()
}

fn clean_tool() -> Tool {
    ToolBuilder::new("clean")
        .description(
            "Remove worktrees and optionally run state files and merged claudes/* branches.",
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
