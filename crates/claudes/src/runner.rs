//! Manifest runner — takes a manifest and executes it.
//!
//! The runner is the only component that actually launches Claude processes.
//! It reads a manifest, creates isolation environments, builds `QueryCommand`s
//! from task fields, and executes them concurrently.

use std::path::{Path, PathBuf};
use std::time::Duration;

use claude_wrapper::{Claude, ClaudeCommand, OutputFormat, QueryCommand};
use tokio::task::JoinSet;
use tracing::{error, info};

use crate::error::{Error, Result};
use crate::isolation::{self, IsolatedEnv};
use crate::manifest::{Manifest, Task};

/// Result of executing a single task.
#[derive(Debug)]
pub struct TaskResult {
    /// Task name.
    pub name: String,
    /// Whether the task succeeded.
    pub success: bool,
    /// Stdout from the claude process.
    pub stdout: String,
    /// Stderr from the claude process.
    pub stderr: String,
    /// Duration of the task.
    pub duration: Duration,
    /// Working directory where the task ran.
    pub work_dir: PathBuf,
}

/// Result of executing an entire manifest.
#[derive(Debug)]
pub struct RunResult {
    /// Per-task results.
    pub tasks: Vec<TaskResult>,
}

impl RunResult {
    /// Whether all tasks succeeded.
    pub fn all_succeeded(&self) -> bool {
        self.tasks.iter().all(|t| t.success)
    }

    /// Count of succeeded tasks.
    pub fn success_count(&self) -> usize {
        self.tasks.iter().filter(|t| t.success).count()
    }
}

/// Options that control runner behavior.
#[derive(Debug, Clone)]
pub struct RunOptions {
    /// Project root directory.
    pub project_dir: PathBuf,
    /// Force overwrite existing worktrees.
    pub force: bool,
}

/// Execute a manifest.
pub async fn run(manifest: &Manifest, options: &RunOptions) -> Result<RunResult> {
    // Validate first.
    manifest
        .validate()
        .map_err(|errors| Error::InvalidManifest(errors.join("; ")))?;

    info!(tasks = manifest.tasks.len(), "executing manifest");

    let mut join_set = JoinSet::new();

    for task in &manifest.tasks {
        let task = task.clone();
        let project_dir = options.project_dir.clone();
        let force = options.force;

        join_set.spawn(async move { run_task(&task, &project_dir, force).await });
    }

    let mut results = Vec::new();
    while let Some(result) = join_set.join_next().await {
        match result {
            Ok(task_result) => results.push(task_result),
            Err(join_err) => {
                error!("task panicked: {join_err}");
            }
        }
    }

    Ok(RunResult { tasks: results })
}

/// Execute a single task.
async fn run_task(task: &Task, project_dir: &Path, force: bool) -> TaskResult {
    let start = std::time::Instant::now();
    let task_name = task.name.clone();

    match run_task_inner(task, project_dir, force).await {
        Ok((output, env)) => TaskResult {
            name: task_name,
            success: output.success,
            stdout: output.stdout,
            stderr: output.stderr,
            duration: start.elapsed(),
            work_dir: env.work_dir,
        },
        Err(e) => {
            error!(task = task_name, error = %e, "task failed");
            TaskResult {
                name: task_name,
                success: false,
                stdout: String::new(),
                stderr: e.to_string(),
                duration: start.elapsed(),
                work_dir: project_dir.to_path_buf(),
            }
        }
    }
}

/// Inner task execution — returns the command output and isolation env.
async fn run_task_inner(
    task: &Task,
    project_dir: &Path,
    force: bool,
) -> Result<(claude_wrapper::exec::CommandOutput, IsolatedEnv)> {
    // Set up isolation.
    let env = if force {
        // If force, try to clean up existing worktree first.
        match isolation::setup(
            project_dir,
            &task.name,
            task.branch.as_deref(),
            task.isolation.as_ref(),
        )
        .await
        {
            Ok(env) => env,
            Err(Error::Worktree(msg)) if msg.contains("already exists") => {
                // Force remove and retry.
                let worktree_dir = match &task.isolation {
                    Some(crate::manifest::Isolation::Worktree { base_dir }) => {
                        project_dir.join(base_dir).join(&task.name)
                    }
                    _ => project_dir.join(".worktrees").join(&task.name),
                };
                let dummy_env = IsolatedEnv {
                    work_dir: worktree_dir.clone(),
                    kind: isolation::IsolationKind::Worktree { path: worktree_dir },
                };
                isolation::cleanup(project_dir, &dummy_env, true).await?;
                isolation::setup(
                    project_dir,
                    &task.name,
                    task.branch.as_deref(),
                    task.isolation.as_ref(),
                )
                .await?
            }
            Err(e) => return Err(e),
        }
    } else {
        isolation::setup(
            project_dir,
            &task.name,
            task.branch.as_deref(),
            task.isolation.as_ref(),
        )
        .await?
    };

    info!(task = task.name, work_dir = %env.work_dir.display(), "running task");

    // Build the Claude client for this task's working directory.
    let mut builder = Claude::builder().working_dir(&env.work_dir);

    if let Some(timeout) = task.timeout_secs {
        builder = builder.timeout_secs(timeout);
    }

    let claude = builder.build()?;

    // Build the query command from task fields using the consuming builder pattern.
    let mut cmd = QueryCommand::new(&task.prompt).output_format(OutputFormat::Json);

    if let Some(model) = &task.model {
        cmd = cmd.model(model);
    }
    if let Some(fallback) = &task.fallback_model {
        cmd = cmd.fallback_model(fallback);
    }
    if let Some(turns) = task.max_turns {
        cmd = cmd.max_turns(turns);
    }
    if let Some(budget) = task.max_budget_usd {
        cmd = cmd.max_budget_usd(budget);
    }
    if let Some(mode) = &task.permission_mode {
        cmd = cmd.permission_mode(parse_permission_mode(mode));
    }
    if let Some(tools) = &task.allowed_tools {
        cmd = cmd.allowed_tools(tools.clone());
    }
    if let Some(tools) = &task.disallowed_tools {
        cmd = cmd.disallowed_tools(tools.clone());
    }
    if let Some(sp) = &task.system_prompt {
        cmd = cmd.system_prompt(sp);
    }
    if let Some(asp) = &task.append_system_prompt {
        cmd = cmd.append_system_prompt(asp);
    }
    if let Some(effort) = &task.effort {
        cmd = cmd.effort(parse_effort(effort));
    }
    if task.no_session_persistence == Some(true) {
        cmd = cmd.no_session_persistence();
    }
    if let Some(mcp) = &task.mcp_config {
        cmd = cmd.mcp_config(mcp);
    }
    if task.strict_mcp_config == Some(true) {
        cmd = cmd.strict_mcp_config();
    }
    if let Some(dirs) = &task.add_dirs {
        for dir in dirs {
            cmd = cmd.add_dir(dir);
        }
    }

    // Execute.
    let output = cmd.execute(&claude).await?;

    Ok((output, env))
}

fn parse_permission_mode(s: &str) -> claude_wrapper::PermissionMode {
    match s {
        "acceptEdits" => claude_wrapper::PermissionMode::AcceptEdits,
        "bypassPermissions" => claude_wrapper::PermissionMode::BypassPermissions,
        "dontAsk" => claude_wrapper::PermissionMode::DontAsk,
        "plan" => claude_wrapper::PermissionMode::Plan,
        "auto" => claude_wrapper::PermissionMode::Auto,
        _ => claude_wrapper::PermissionMode::Default,
    }
}

fn parse_effort(s: &str) -> claude_wrapper::Effort {
    match s {
        "low" => claude_wrapper::Effort::Low,
        "high" => claude_wrapper::Effort::High,
        _ => claude_wrapper::Effort::Medium,
    }
}
