//! Manifest runner — takes a manifest and executes it.
//!
//! The runner is the only component that actually launches Claude processes.
//! It reads a manifest, creates isolation environments, builds `QueryCommand`s
//! from task fields, and executes them concurrently.

use std::path::PathBuf;
use std::time::Duration;

use claude_wrapper::streaming::StreamEvent;
use claude_wrapper::{Claude, ClaudeCommand, OutputFormat, QueryCommand};
use tokio::sync::mpsc;
use tokio::task::JoinSet;
use tracing::{error, info, warn};

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
    /// Cost in USD aggregated from stream events (or parsed from stdout as fallback).
    pub cost_usd: Option<f64>,
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

/// When to automatically remove worktrees after execution.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum CleanupPolicy {
    /// Never auto-remove worktrees (default). Use `claudes clean` explicitly.
    #[default]
    None,
    /// Remove worktrees only for tasks that succeeded. Keep failed ones for inspection.
    OnSuccess,
    /// Remove all worktrees after the run, regardless of outcome.
    Always,
}

/// A tagged event from a running task, sent via the event channel.
#[derive(Debug, Clone)]
pub struct TaskEvent {
    /// Which task produced this event.
    pub task_name: String,
    /// The stream event from claude.
    pub event: StreamEvent,
}

/// Options that control runner behavior.
#[derive(Debug, Clone)]
pub struct RunOptions {
    /// Project root directory.
    pub project_dir: PathBuf,
    /// Force overwrite existing worktrees.
    pub force: bool,
    /// Override the claude binary path (default: find `claude` in PATH).
    pub binary: Option<PathBuf>,
    /// Extra environment variables passed to every Claude process.
    pub env: Vec<(String, String)>,
    /// When to auto-remove worktrees after execution.
    pub cleanup: CleanupPolicy,
    /// Channel for streaming events from tasks. If `None`, events are not streamed.
    pub event_sender: Option<mpsc::UnboundedSender<TaskEvent>>,
}

/// Builder for [`RunOptions`].
///
/// `project_dir` is required and passed to [`RunOptionsBuilder::new`].
/// All other fields are optional with sensible defaults.
///
/// # Example
///
/// ```no_run
/// use claudes::runner::{CleanupPolicy, RunOptionsBuilder};
///
/// let options = RunOptionsBuilder::new("/path/to/project")
///     .force(true)
///     .cleanup(CleanupPolicy::OnSuccess)
///     .build();
/// ```
#[derive(Debug)]
pub struct RunOptionsBuilder {
    project_dir: PathBuf,
    force: bool,
    binary: Option<PathBuf>,
    env: Vec<(String, String)>,
    cleanup: CleanupPolicy,
}

impl RunOptionsBuilder {
    /// Create a new builder with the required project directory.
    pub fn new(project_dir: impl Into<PathBuf>) -> Self {
        Self {
            project_dir: project_dir.into(),
            force: false,
            binary: None,
            env: Vec::new(),
            cleanup: CleanupPolicy::None,
        }
    }

    /// Force overwrite existing worktrees.
    pub fn force(mut self, force: bool) -> Self {
        self.force = force;
        self
    }

    /// Override the claude binary path.
    pub fn binary(mut self, binary: impl Into<PathBuf>) -> Self {
        self.binary = Some(binary.into());
        self
    }

    /// Add an extra environment variable passed to every Claude process.
    pub fn env(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.env.push((key.into(), value.into()));
        self
    }

    /// Set the cleanup policy for worktrees after execution.
    pub fn cleanup(mut self, cleanup: CleanupPolicy) -> Self {
        self.cleanup = cleanup;
        self
    }

    /// Build the [`RunOptions`].
    pub fn build(self) -> RunOptions {
        RunOptions {
            project_dir: self.project_dir,
            force: self.force,
            binary: self.binary,
            env: self.env,
            cleanup: self.cleanup,
            event_sender: None,
        }
    }
}

/// Execute a manifest.
pub async fn run(manifest: &Manifest, options: &RunOptions) -> Result<RunResult> {
    // Validate first.
    manifest
        .validate()
        .map_err(|errors| Error::InvalidManifest(errors.join("; ")))?;

    let manifest = manifest.resolve();

    info!(tasks = manifest.tasks.len(), "executing manifest");

    let mut join_set = JoinSet::new();

    for task in &manifest.tasks {
        let task = task.clone();
        let options = options.clone();

        join_set.spawn(async move { run_task(&task, &options).await });
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

    // Auto-cleanup worktrees based on policy.
    if options.cleanup != CleanupPolicy::None {
        for task_result in &results {
            let should_clean = match options.cleanup {
                CleanupPolicy::Always => true,
                CleanupPolicy::OnSuccess => task_result.success,
                CleanupPolicy::None => false,
            };
            if should_clean {
                let env = IsolatedEnv {
                    work_dir: task_result.work_dir.clone(),
                    kind: isolation::IsolationKind::Worktree {
                        path: task_result.work_dir.clone(),
                    },
                };
                if let Err(e) = isolation::cleanup(&options.project_dir, &env, false).await {
                    // Non-fatal: log and continue.
                    tracing::warn!(
                        task = task_result.name,
                        error = %e,
                        "failed to clean up worktree"
                    );
                }
            }
        }
    }

    Ok(RunResult { tasks: results })
}

/// Execute a single task.
async fn run_task(task: &Task, options: &RunOptions) -> TaskResult {
    let start = std::time::Instant::now();
    let task_name = task.name.clone();

    match run_task_inner(task, options).await {
        Ok((output, env, stream_cost)) => {
            let cost_usd = stream_cost.or_else(|| {
                serde_json::from_str::<serde_json::Value>(&output.stdout)
                    .ok()
                    .and_then(|v| {
                        v.get("total_cost_usd")
                            .or_else(|| v.get("cost_usd"))
                            .and_then(|c| c.as_f64())
                    })
            });
            TaskResult {
                name: task_name,
                success: output.success,
                stdout: output.stdout,
                stderr: output.stderr,
                duration: start.elapsed(),
                work_dir: env.work_dir,
                cost_usd,
            }
        }
        Err(e) => {
            error!(task = task_name, error = %e, "task failed");
            TaskResult {
                name: task_name,
                success: false,
                stdout: String::new(),
                stderr: e.to_string(),
                duration: start.elapsed(),
                work_dir: options.project_dir.to_path_buf(),
                cost_usd: None,
            }
        }
    }
}

/// Inner task execution — returns the command output, isolation env, and stream-aggregated cost.
async fn run_task_inner(
    task: &Task,
    options: &RunOptions,
) -> Result<(
    claude_wrapper::exec::CommandOutput,
    IsolatedEnv,
    Option<f64>,
)> {
    let project_dir = &options.project_dir;
    let force = options.force;

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

    // Run pre-hooks before starting the session.
    if let Some(hooks) = &task.pre_hooks {
        info!(task = task.name, "running pre hooks");
        run_hooks(&task.name, hooks, &env.work_dir, "pre").await?;
    }

    info!(task = task.name, work_dir = %env.work_dir.display(), "running task");

    // Build the Claude client for this task's working directory.
    let mut builder = Claude::builder().working_dir(&env.work_dir);

    if let Some(binary) = &options.binary {
        builder = builder.binary(binary);
    }
    for (k, v) in &options.env {
        builder = builder.env(k, v);
    }

    if let Some(timeout) = task.timeout_secs {
        builder = builder.timeout_secs(timeout);
    }

    let claude = builder.build()?;

    // Build the query command from task fields using the consuming builder pattern.
    let cmd = build_query_command(task);

    // Execute: streaming if event_sender is available, otherwise batch.
    let execution_result = if let Some(sender) = &options.event_sender {
        let task_name = task.name.clone();
        let sender = sender.clone();

        // Determine the log path based on isolation type.
        let log_path = if matches!(env.kind, isolation::IsolationKind::None) {
            env.work_dir
                .join(".claudes")
                .join("logs")
                .join(format!("{}.jsonl", task.name))
        } else {
            env.work_dir.join(".claudes").join("run.jsonl")
        };

        // Create log directory and open the append-only log file.
        let mut log_file: Option<std::fs::File> = None;
        if let Some(log_dir) = log_path.parent() {
            match tokio::fs::create_dir_all(log_dir).await {
                Ok(()) => {
                    match std::fs::OpenOptions::new()
                        .create(true)
                        .append(true)
                        .open(&log_path)
                    {
                        Ok(f) => log_file = Some(f),
                        Err(e) => {
                            warn!(task = task.name, path = %log_path.display(), error = %e, "failed to open log file");
                        }
                    }
                }
                Err(e) => {
                    warn!(task = task.name, path = %log_path.display(), error = %e, "failed to create log dir");
                }
            }
        }

        // Signal that the task has started.
        let _ = sender.send(TaskEvent {
            task_name: task_name.clone(),
            event: StreamEvent {
                data: serde_json::json!({
                    "type": "claudes_task_start",
                    "task_name": task_name,
                }),
            },
        });

        let mut result_json = String::new();
        let mut stream_cost: Option<f64> = None;

        let output = claude_wrapper::streaming::stream_query(&claude, &cmd, |event| {
            // Write the raw JSON event to the NDJSON log file.
            if let Some(ref mut file) = log_file {
                match serde_json::to_string(&event.data) {
                    Ok(json) => {
                        use std::io::Write;
                        if let Err(e) = writeln!(file, "{json}") {
                            warn!(task = task_name, error = %e, "failed to write event to log");
                        }
                    }
                    Err(e) => {
                        warn!(task = task_name, error = %e, "failed to serialize event for log");
                    }
                }
            }

            // Accumulate cost from stream events.
            if let Some(cost) = event.cost_usd() {
                stream_cost = Some(cost);
            }

            // Capture the result event's JSON for the TaskResult stdout.
            if event.is_result() {
                result_json = serde_json::to_string(&event.data).unwrap_or_default();
            }
            let _ = sender.send(TaskEvent {
                task_name: task_name.clone(),
                event,
            });
        })
        .await?;

        // stream_query returns empty stdout since it was consumed via handler.
        // Replace with the result event JSON so state parsing still works.
        let output = claude_wrapper::exec::CommandOutput {
            stdout: result_json,
            ..output
        };
        (output, env, stream_cost)
    } else {
        let output = cmd.execute(&claude).await?;
        (output, env, None)
    };

    let (output, env, stream_cost) = execution_result;

    // Run post-hooks if the session succeeded.
    if output.success
        && let Some(hooks) = &task.post_hooks
    {
        run_hooks(&task.name, hooks, &env.work_dir, "post").await?;
    }

    Ok((output, env, stream_cost))
}

/// Execute hooks (pre or post) for a task in the given working directory.
async fn run_hooks(
    task_name: &str,
    hooks: &[String],
    work_dir: &std::path::Path,
    kind: &str,
) -> Result<()> {
    for hook in hooks {
        info!(task = task_name, hook = hook, "running {kind} hook");
        let output = tokio::process::Command::new("sh")
            .arg("-c")
            .arg(hook)
            .current_dir(work_dir)
            .output()
            .await
            .map_err(|e| Error::TaskFailed {
                name: task_name.to_owned(),
                message: format!("{kind} hook '{hook}' failed to spawn: {e}"),
            })?;
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
            return Err(Error::TaskFailed {
                name: task_name.to_owned(),
                message: format!("{kind} hook '{hook}' exited non-zero: {stderr}"),
            });
        }
    }
    Ok(())
}

/// Build a QueryCommand from a Task's fields.
fn build_query_command(task: &Task) -> QueryCommand {
    let mut cmd = QueryCommand::new(&task.prompt).output_format(OutputFormat::StreamJson);

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

    cmd
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

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn pre_hooks_success() {
        let dir = tempfile::tempdir().unwrap();
        run_hooks("test-task", &["echo hello".into()], dir.path(), "pre")
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn pre_hooks_failure_returns_task_failed() {
        let dir = tempfile::tempdir().unwrap();
        let err = run_hooks("test-task", &["exit 1".into()], dir.path(), "pre")
            .await
            .unwrap_err();
        match err {
            Error::TaskFailed { name, message } => {
                assert_eq!(name, "test-task");
                assert!(message.contains("exit 1"));
            }
            _ => panic!("expected Error::TaskFailed"),
        }
    }

    #[tokio::test]
    async fn pre_hooks_stops_on_first_failure() {
        let dir = tempfile::tempdir().unwrap();
        let sentinel = dir.path().join("sentinel");
        let hooks = vec!["exit 1".into(), format!("touch {}", sentinel.display())];
        let _ = run_hooks("test-task", &hooks, dir.path(), "pre").await;
        assert!(!sentinel.exists(), "second hook should not have run");
    }

    #[tokio::test]
    async fn pre_hooks_empty_list_is_noop() {
        let dir = tempfile::tempdir().unwrap();
        run_hooks("test-task", &[], dir.path(), "pre")
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn post_hooks_success() {
        let dir = tempfile::tempdir().unwrap();
        run_hooks("test-task", &["echo hello".into()], dir.path(), "post")
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn post_hooks_failure_returns_task_failed() {
        let dir = tempfile::tempdir().unwrap();
        let err = run_hooks("test-task", &["exit 1".into()], dir.path(), "post")
            .await
            .unwrap_err();
        match err {
            Error::TaskFailed { name, message } => {
                assert_eq!(name, "test-task");
                assert!(message.contains("exit 1"));
            }
            _ => panic!("expected Error::TaskFailed"),
        }
    }

    #[tokio::test]
    async fn post_hooks_stops_on_first_failure() {
        let dir = tempfile::tempdir().unwrap();
        let sentinel = dir.path().join("sentinel");
        let hooks = vec!["exit 1".into(), format!("touch {}", sentinel.display())];
        let _ = run_hooks("test-task", &hooks, dir.path(), "post").await;
        assert!(!sentinel.exists(), "second hook should not have run");
    }

    #[tokio::test]
    async fn post_hooks_empty_list_is_noop() {
        let dir = tempfile::tempdir().unwrap();
        run_hooks("test-task", &[], dir.path(), "post")
            .await
            .unwrap();
    }

    #[test]
    fn builder_defaults() {
        let opts = RunOptionsBuilder::new("/tmp/project").build();
        assert_eq!(opts.project_dir, PathBuf::from("/tmp/project"));
        assert!(!opts.force);
        assert!(opts.binary.is_none());
        assert!(opts.env.is_empty());
        assert_eq!(opts.cleanup, CleanupPolicy::None);
    }

    #[test]
    fn builder_force() {
        let opts = RunOptionsBuilder::new("/tmp/project").force(true).build();
        assert!(opts.force);
    }

    #[test]
    fn builder_binary() {
        let opts = RunOptionsBuilder::new("/tmp/project")
            .binary("/usr/local/bin/claude")
            .build();
        assert_eq!(opts.binary, Some(PathBuf::from("/usr/local/bin/claude")));
    }

    #[test]
    fn builder_env_repeatable() {
        let opts = RunOptionsBuilder::new("/tmp/project")
            .env("FOO", "bar")
            .env("BAZ", "qux")
            .build();
        assert_eq!(
            opts.env,
            vec![
                ("FOO".to_string(), "bar".to_string()),
                ("BAZ".to_string(), "qux".to_string()),
            ]
        );
    }

    #[test]
    fn builder_cleanup() {
        let opts = RunOptionsBuilder::new("/tmp/project")
            .cleanup(CleanupPolicy::Always)
            .build();
        assert_eq!(opts.cleanup, CleanupPolicy::Always);
    }

    #[test]
    fn builder_on_success_cleanup() {
        let opts = RunOptionsBuilder::new("/tmp/project")
            .cleanup(CleanupPolicy::OnSuccess)
            .build();
        assert_eq!(opts.cleanup, CleanupPolicy::OnSuccess);
    }
}
