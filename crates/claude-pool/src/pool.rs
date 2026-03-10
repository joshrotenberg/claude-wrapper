//! Core pool engine for managing Claude CLI workers.
//!
//! The [`Pool`] struct is the main entry point. It manages worker lifecycle,
//! task assignment, budget tracking, and shared context.
//!
//! # Example
//!
//! ```no_run
//! # async fn example() -> claude_pool::Result<()> {
//! use claude_pool::{Pool, GlobalWorkerConfig};
//!
//! let claude = claude_wrapper::Claude::builder().build()?;
//! let pool = Pool::builder(claude)
//!     .workers(4)
//!     .build()
//!     .await?;
//!
//! let result = pool.run("write a haiku about rust").await?;
//! println!("{}", result.output);
//!
//! pool.drain().await?;
//! # Ok(())
//! # }
//! ```

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use tokio::sync::Mutex;

use claude_wrapper::Claude;
use claude_wrapper::types::OutputFormat;

use crate::config::ResolvedConfig;
use crate::error::{Error, Result};
use crate::store::PoolStore;
use crate::types::*;

/// Shared pool state behind an `Arc`.
struct PoolInner<S: PoolStore> {
    claude: Claude,
    config: GlobalWorkerConfig,
    store: S,
    total_spend: AtomicU64,
    shutdown: AtomicBool,
    /// Context key-value pairs injected into worker system prompts.
    context: dashmap::DashMap<String, String>,
    /// Mutex for worker assignment to avoid races.
    assignment_lock: Mutex<()>,
    /// Worktree manager, if worktree isolation is enabled.
    worktree_manager: Option<crate::worktree::WorktreeManager>,
}

/// A pool of Claude CLI workers.
///
/// Created via [`Pool::builder`]. Manages worker lifecycle, task routing,
/// and budget enforcement.
pub struct Pool<S: PoolStore> {
    inner: Arc<PoolInner<S>>,
}

// Manual Clone so we don't require S: Clone
impl<S: PoolStore> Clone for Pool<S> {
    fn clone(&self) -> Self {
        Self {
            inner: Arc::clone(&self.inner),
        }
    }
}

/// Builder for constructing a [`Pool`].
pub struct PoolBuilder<S: PoolStore> {
    claude: Claude,
    worker_count: usize,
    config: GlobalWorkerConfig,
    store: S,
    worker_configs: Vec<WorkerConfig>,
}

impl<S: PoolStore + 'static> PoolBuilder<S> {
    /// Set the number of workers to spawn.
    pub fn workers(mut self, count: usize) -> Self {
        self.worker_count = count;
        self
    }

    /// Set the global worker configuration.
    pub fn config(mut self, config: GlobalWorkerConfig) -> Self {
        self.config = config;
        self
    }

    /// Add a per-worker configuration override.
    ///
    /// Call multiple times for multiple workers. Worker configs are applied
    /// in order: the first call sets worker-0's config, the second worker-1's, etc.
    /// Workers without an explicit config get [`WorkerConfig::default()`].
    pub fn worker_config(mut self, config: WorkerConfig) -> Self {
        self.worker_configs.push(config);
        self
    }

    /// Build and initialize the pool, registering workers in the store.
    pub async fn build(self) -> Result<Pool<S>> {
        // Set up worktree manager if isolation is enabled.
        let worktree_manager = if self.config.worktree_isolation {
            let repo_dir = self
                .claude
                .working_dir()
                .map(|p| p.to_path_buf())
                .unwrap_or_else(|| std::env::current_dir().unwrap_or_default());
            Some(crate::worktree::WorktreeManager::new(repo_dir, None))
        } else {
            None
        };

        let inner = Arc::new(PoolInner {
            claude: self.claude,
            config: self.config,
            store: self.store,
            total_spend: AtomicU64::new(0),
            shutdown: AtomicBool::new(false),
            context: dashmap::DashMap::new(),
            assignment_lock: Mutex::new(()),
            worktree_manager,
        });

        // Register workers in the store.
        for i in 0..self.worker_count {
            let worker_config = self.worker_configs.get(i).cloned().unwrap_or_default();

            let worker_id = WorkerId(format!("worker-{i}"));

            // Create worktree if isolation is enabled.
            let worktree_path = if let Some(ref mgr) = inner.worktree_manager {
                let path = mgr.create(&worker_id).await?;
                Some(path.to_string_lossy().into_owned())
            } else {
                None
            };

            let record = WorkerRecord {
                id: worker_id,
                state: WorkerState::Idle,
                config: worker_config,
                current_task: None,
                session_id: None,
                tasks_completed: 0,
                cost_microdollars: 0,
                restart_count: 0,
                worktree_path,
            };
            inner.store.put_worker(record).await?;
        }

        Ok(Pool { inner })
    }
}

impl Pool<crate::store::InMemoryStore> {
    /// Create a builder with the default in-memory store.
    pub fn builder(claude: Claude) -> PoolBuilder<crate::store::InMemoryStore> {
        PoolBuilder {
            claude,
            worker_count: 1,
            config: GlobalWorkerConfig::default(),
            store: crate::store::InMemoryStore::new(),
            worker_configs: Vec::new(),
        }
    }
}

impl<S: PoolStore + 'static> Pool<S> {
    /// Create a builder with a custom store.
    pub fn builder_with_store(claude: Claude, store: S) -> PoolBuilder<S> {
        PoolBuilder {
            claude,
            worker_count: 1,
            config: GlobalWorkerConfig::default(),
            store,
            worker_configs: Vec::new(),
        }
    }

    /// Run a task synchronously, blocking until completion.
    ///
    /// Assigns the task to the next idle worker, executes the prompt,
    /// and returns the result.
    pub async fn run(&self, prompt: &str) -> Result<TaskResult> {
        self.run_with_config(prompt, None).await
    }

    /// Run a task with per-task config overrides.
    pub async fn run_with_config(
        &self,
        prompt: &str,
        task_config: Option<WorkerConfig>,
    ) -> Result<TaskResult> {
        self.check_shutdown()?;
        self.check_budget()?;

        let task_id = TaskId(format!("task-{}", new_id()));

        let record = TaskRecord {
            id: task_id.clone(),
            prompt: prompt.to_string(),
            state: TaskState::Pending,
            worker_id: None,
            result: None,
            tags: vec![],
            config: task_config,
        };
        self.inner.store.put_task(record).await?;

        let (worker_id, worker_config) = self.assign_worker(&task_id).await?;
        let result = self
            .execute_task(&task_id, prompt, &worker_id, &worker_config)
            .await;

        self.release_worker(&worker_id, &task_id, &result).await?;

        let task_result = result?;
        // Update task record with result.
        let mut task = self
            .inner
            .store
            .get_task(&task_id)
            .await?
            .ok_or_else(|| Error::TaskNotFound(task_id.0.clone()))?;
        task.state = TaskState::Completed;
        task.result = Some(task_result.clone());
        self.inner.store.put_task(task).await?;

        Ok(task_result)
    }

    /// Submit a task for async execution, returning the task ID immediately.
    ///
    /// Use [`Pool::result`] to poll for completion.
    pub async fn submit(&self, prompt: &str) -> Result<TaskId> {
        self.submit_with_config(prompt, None, vec![]).await
    }

    /// Submit a task with config overrides and tags.
    pub async fn submit_with_config(
        &self,
        prompt: &str,
        task_config: Option<WorkerConfig>,
        tags: Vec<String>,
    ) -> Result<TaskId> {
        self.check_shutdown()?;
        self.check_budget()?;

        let task_id = TaskId(format!("task-{}", new_id()));
        let prompt = prompt.to_string();

        let record = TaskRecord {
            id: task_id.clone(),
            prompt: prompt.clone(),
            state: TaskState::Pending,
            worker_id: None,
            result: None,
            tags,
            config: task_config,
        };
        self.inner.store.put_task(record).await?;

        // Spawn the task execution in the background.
        let pool = self.clone();
        let tid = task_id.clone();
        tokio::spawn(async move {
            let task = match pool.inner.store.get_task(&tid).await {
                Ok(Some(t)) => t,
                _ => return,
            };

            match pool.assign_worker(&tid).await {
                Ok((worker_id, worker_config)) => {
                    let result = pool
                        .execute_task(&tid, &prompt, &worker_id, &worker_config)
                        .await;

                    let _ = pool.release_worker(&worker_id, &tid, &result).await;

                    let mut updated = task;
                    match result {
                        Ok(task_result) => {
                            updated.state = TaskState::Completed;
                            updated.result = Some(task_result);
                        }
                        Err(e) => {
                            updated.state = TaskState::Failed;
                            updated.result = Some(TaskResult {
                                output: e.to_string(),
                                success: false,
                                cost_microdollars: 0,
                                turns_used: 0,
                                session_id: None,
                            });
                        }
                    }
                    let _ = pool.inner.store.put_task(updated).await;
                }
                Err(e) => {
                    let mut updated = task;
                    updated.state = TaskState::Failed;
                    updated.result = Some(TaskResult {
                        output: e.to_string(),
                        success: false,
                        cost_microdollars: 0,
                        turns_used: 0,
                        session_id: None,
                    });
                    let _ = pool.inner.store.put_task(updated).await;
                }
            }
        });

        Ok(task_id)
    }

    /// Get the result of a submitted task.
    ///
    /// Returns `None` if the task is still pending/running.
    pub async fn result(&self, task_id: &TaskId) -> Result<Option<TaskResult>> {
        let task = self
            .inner
            .store
            .get_task(task_id)
            .await?
            .ok_or_else(|| Error::TaskNotFound(task_id.0.clone()))?;

        match task.state {
            TaskState::Completed | TaskState::Failed => Ok(task.result),
            _ => Ok(None),
        }
    }

    /// Cancel a pending or running task.
    pub async fn cancel(&self, task_id: &TaskId) -> Result<()> {
        let mut task = self
            .inner
            .store
            .get_task(task_id)
            .await?
            .ok_or_else(|| Error::TaskNotFound(task_id.0.clone()))?;

        match task.state {
            TaskState::Pending => {
                task.state = TaskState::Cancelled;
                self.inner.store.put_task(task).await?;
                Ok(())
            }
            TaskState::Running => {
                // Mark as cancelled; the executing task will check on completion.
                task.state = TaskState::Cancelled;
                self.inner.store.put_task(task).await?;
                Ok(())
            }
            _ => Ok(()), // already terminal
        }
    }

    /// Execute tasks in parallel across available workers, collecting all results.
    pub async fn fan_out(&self, prompts: &[&str]) -> Result<Vec<TaskResult>> {
        self.check_shutdown()?;
        self.check_budget()?;

        let mut handles = Vec::with_capacity(prompts.len());

        for prompt in prompts {
            let pool = self.clone();
            let prompt = prompt.to_string();
            handles.push(tokio::spawn(async move { pool.run(&prompt).await }));
        }

        let mut results = Vec::with_capacity(handles.len());
        for handle in handles {
            results.push(
                handle
                    .await
                    .map_err(|e| Error::Store(format!("task join error: {e}")))?,
            );
        }

        results.into_iter().collect()
    }

    /// Set a shared context value.
    ///
    /// Context is injected into worker system prompts at task start.
    pub fn set_context(&self, key: impl Into<String>, value: impl Into<String>) {
        self.inner.context.insert(key.into(), value.into());
    }

    /// Get a shared context value.
    pub fn get_context(&self, key: &str) -> Option<String> {
        self.inner.context.get(key).map(|v| v.value().clone())
    }

    /// Remove a shared context value.
    pub fn delete_context(&self, key: &str) -> Option<String> {
        self.inner.context.remove(key).map(|(_, v)| v)
    }

    /// List all context keys and values.
    pub fn list_context(&self) -> Vec<(String, String)> {
        self.inner
            .context
            .iter()
            .map(|r| (r.key().clone(), r.value().clone()))
            .collect()
    }

    /// Gracefully shut down the pool.
    ///
    /// Marks the pool as shut down so no new tasks are accepted,
    /// then waits for in-flight tasks to complete.
    pub async fn drain(&self) -> Result<DrainSummary> {
        self.inner.shutdown.store(true, Ordering::SeqCst);

        // Wait for all running tasks to finish.
        loop {
            let running = self
                .inner
                .store
                .list_tasks(&TaskFilter {
                    state: Some(TaskState::Running),
                    ..Default::default()
                })
                .await?;
            if running.is_empty() {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        }

        // Mark all workers as stopped.
        let workers = self.inner.store.list_workers().await?;
        let mut total_cost = 0u64;
        let mut total_tasks = 0u64;
        let worker_ids: Vec<_> = workers.iter().map(|w| w.id.clone()).collect();

        for mut worker in workers {
            total_cost += worker.cost_microdollars;
            total_tasks += worker.tasks_completed;
            worker.state = WorkerState::Stopped;
            self.inner.store.put_worker(worker).await?;
        }

        // Clean up worktrees if isolation was enabled.
        if let Some(ref mgr) = self.inner.worktree_manager {
            mgr.cleanup_all(&worker_ids).await?;
        }

        Ok(DrainSummary {
            total_cost_microdollars: total_cost,
            total_tasks_completed: total_tasks,
        })
    }

    /// Get a snapshot of pool status.
    pub async fn status(&self) -> Result<PoolStatus> {
        let workers = self.inner.store.list_workers().await?;
        let idle = workers
            .iter()
            .filter(|w| w.state == WorkerState::Idle)
            .count();
        let busy = workers
            .iter()
            .filter(|w| w.state == WorkerState::Busy)
            .count();

        let running_tasks = self
            .inner
            .store
            .list_tasks(&TaskFilter {
                state: Some(TaskState::Running),
                ..Default::default()
            })
            .await?
            .len();

        let pending_tasks = self
            .inner
            .store
            .list_tasks(&TaskFilter {
                state: Some(TaskState::Pending),
                ..Default::default()
            })
            .await?
            .len();

        Ok(PoolStatus {
            total_workers: workers.len(),
            idle_workers: idle,
            busy_workers: busy,
            running_tasks,
            pending_tasks,
            total_spend_microdollars: self.inner.total_spend.load(Ordering::Relaxed),
            budget_microdollars: self.inner.config.budget_microdollars,
            shutdown: self.inner.shutdown.load(Ordering::Relaxed),
        })
    }

    /// Get a reference to the store.
    pub fn store(&self) -> &S {
        &self.inner.store
    }

    // ── Internal helpers ─────────────────────────────────────────────

    fn check_shutdown(&self) -> Result<()> {
        if self.inner.shutdown.load(Ordering::SeqCst) {
            Err(Error::PoolShutdown)
        } else {
            Ok(())
        }
    }

    fn check_budget(&self) -> Result<()> {
        if let Some(limit) = self.inner.config.budget_microdollars {
            let spent = self.inner.total_spend.load(Ordering::Relaxed);
            if spent >= limit {
                return Err(Error::BudgetExhausted {
                    spent_microdollars: spent,
                    limit_microdollars: limit,
                });
            }
        }
        Ok(())
    }

    /// Find an idle worker and assign the task to it.
    async fn assign_worker(&self, task_id: &TaskId) -> Result<(WorkerId, WorkerConfig)> {
        let _lock = self.inner.assignment_lock.lock().await;

        let workers = self.inner.store.list_workers().await?;
        let mut idle_worker = None;

        for worker in &workers {
            if worker.state == WorkerState::Idle {
                idle_worker = Some(worker.clone());
                break;
            }
        }

        let mut worker = idle_worker.ok_or(Error::NoIdleWorkers)?;
        let config = worker.config.clone();

        worker.state = WorkerState::Busy;
        worker.current_task = Some(task_id.clone());
        self.inner.store.put_worker(worker.clone()).await?;

        // Update task with assigned worker.
        if let Some(mut task) = self.inner.store.get_task(task_id).await? {
            task.state = TaskState::Running;
            task.worker_id = Some(worker.id.clone());
            self.inner.store.put_task(task).await?;
        }

        Ok((worker.id, config))
    }

    /// Release a worker back to idle after task completion.
    async fn release_worker(
        &self,
        worker_id: &WorkerId,
        _task_id: &TaskId,
        result: &std::result::Result<TaskResult, Error>,
    ) -> Result<()> {
        if let Some(mut worker) = self.inner.store.get_worker(worker_id).await? {
            worker.state = WorkerState::Idle;
            worker.current_task = None;

            if let Ok(task_result) = result {
                worker.tasks_completed += 1;
                worker.cost_microdollars += task_result.cost_microdollars;
                worker.session_id = task_result.session_id.clone();

                // Update global spend tracker.
                self.inner
                    .total_spend
                    .fetch_add(task_result.cost_microdollars, Ordering::Relaxed);
            }

            self.inner.store.put_worker(worker).await?;
        }
        Ok(())
    }

    /// Execute a task on a specific worker by invoking the Claude CLI.
    async fn execute_task(
        &self,
        _task_id: &TaskId,
        prompt: &str,
        worker_id: &WorkerId,
        worker_config: &WorkerConfig,
    ) -> Result<TaskResult> {
        let task_record = self.inner.store.get_task(_task_id).await?;
        let task_cfg = task_record.as_ref().and_then(|t| t.config.as_ref());

        let resolved = ResolvedConfig::resolve(&self.inner.config, worker_config, task_cfg);

        // Build the system prompt with context injection.
        let system_prompt = self.build_system_prompt(&resolved);

        // Build and execute the query.
        let mut cmd = claude_wrapper::QueryCommand::new(prompt)
            .output_format(OutputFormat::Json)
            .permission_mode(resolved.permission_mode);

        if resolved.permission_mode == PermissionMode::BypassPermissions {
            cmd = cmd.dangerously_skip_permissions();
        }

        if let Some(ref model) = resolved.model {
            cmd = cmd.model(model);
        }
        if let Some(max_turns) = resolved.max_turns {
            cmd = cmd.max_turns(max_turns);
        }
        if let Some(ref sp) = system_prompt {
            cmd = cmd.system_prompt(sp);
        }
        if let Some(effort) = resolved.effort {
            cmd = cmd.effort(effort);
        }
        if !resolved.allowed_tools.is_empty() {
            cmd = cmd.allowed_tools(&resolved.allowed_tools);
        }

        // Use worktree working dir if the worker has one, otherwise use default.
        let claude_instance = if let Some(worker) = self.inner.store.get_worker(worker_id).await? {
            // Resume session if the worker has one.
            if let Some(ref session_id) = worker.session_id {
                cmd = cmd.resume(session_id);
            }

            if let Some(ref wt_path) = worker.worktree_path {
                self.inner.claude.with_working_dir(wt_path)
            } else {
                self.inner.claude.clone()
            }
        } else {
            self.inner.claude.clone()
        };

        tracing::debug!(
            worker_id = %worker_id.0,
            model = ?resolved.model,
            effort = ?resolved.effort,
            "executing task"
        );

        let query_result = cmd.execute_json(&claude_instance).await?;

        let cost_microdollars = query_result
            .cost_usd
            .map(|c| (c * 1_000_000.0) as u64)
            .unwrap_or(0);

        Ok(TaskResult {
            output: query_result.result,
            success: !query_result.is_error,
            cost_microdollars,
            turns_used: 0, // TODO: extract from query result when available
            session_id: Some(query_result.session_id),
        })
    }

    /// Build the system prompt by combining resolved config and context.
    fn build_system_prompt(&self, resolved: &ResolvedConfig) -> Option<String> {
        let context_entries: Vec<_> = self.list_context();

        if resolved.system_prompt.is_none() && context_entries.is_empty() {
            return None;
        }

        let mut parts = Vec::new();

        if let Some(ref sp) = resolved.system_prompt {
            parts.push(sp.clone());
        }

        if !context_entries.is_empty() {
            parts.push("\n\n## Shared Context\n".to_string());
            for (key, value) in &context_entries {
                parts.push(format!("- **{key}**: {value}"));
            }
        }

        Some(parts.join("\n"))
    }
}

/// Summary returned by [`Pool::drain`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DrainSummary {
    /// Total cost across all workers in microdollars.
    pub total_cost_microdollars: u64,
    /// Total number of tasks completed.
    pub total_tasks_completed: u64,
}

/// Snapshot of pool status.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PoolStatus {
    /// Total number of workers.
    pub total_workers: usize,
    /// Number of idle workers.
    pub idle_workers: usize,
    /// Number of busy workers.
    pub busy_workers: usize,
    /// Number of currently running tasks.
    pub running_tasks: usize,
    /// Number of pending (queued) tasks.
    pub pending_tasks: usize,
    /// Total spend in microdollars.
    pub total_spend_microdollars: u64,
    /// Budget cap in microdollars, if set.
    pub budget_microdollars: Option<u64>,
    /// Whether the pool is shutting down.
    pub shutdown: bool,
}

use serde::{Deserialize, Serialize};

/// Generate a short unique ID.
fn new_id() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    format!("{nanos:x}")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mock_claude() -> Claude {
        // Build a Claude instance pointing at a non-existent binary.
        // Tests that don't actually execute tasks can use this.
        Claude::builder().binary("/usr/bin/false").build().unwrap()
    }

    #[tokio::test]
    async fn build_pool_registers_workers() {
        let pool = Pool::builder(mock_claude())
            .workers(3)
            .build()
            .await
            .unwrap();

        let workers = pool.store().list_workers().await.unwrap();
        assert_eq!(workers.len(), 3);

        for worker in &workers {
            assert_eq!(worker.state, WorkerState::Idle);
        }
    }

    #[tokio::test]
    async fn pool_with_worker_configs() {
        let pool = Pool::builder(mock_claude())
            .workers(2)
            .worker_config(WorkerConfig {
                model: Some("opus".into()),
                role: Some("reviewer".into()),
                ..Default::default()
            })
            .build()
            .await
            .unwrap();

        let workers = pool.store().list_workers().await.unwrap();
        let w0 = workers.iter().find(|w| w.id.0 == "worker-0").unwrap();
        let w1 = workers.iter().find(|w| w.id.0 == "worker-1").unwrap();
        assert_eq!(w0.config.model.as_deref(), Some("opus"));
        assert_eq!(w0.config.role.as_deref(), Some("reviewer"));
        // Worker 1 gets default config.
        assert!(w1.config.model.is_none());
    }

    #[tokio::test]
    async fn context_operations() {
        let pool = Pool::builder(mock_claude())
            .workers(1)
            .build()
            .await
            .unwrap();

        pool.set_context("repo", "claude-wrapper");
        pool.set_context("branch", "main");

        assert_eq!(pool.get_context("repo").as_deref(), Some("claude-wrapper"));
        assert_eq!(pool.list_context().len(), 2);

        pool.delete_context("branch");
        assert!(pool.get_context("branch").is_none());
    }

    #[tokio::test]
    async fn drain_marks_workers_stopped() {
        let pool = Pool::builder(mock_claude())
            .workers(2)
            .build()
            .await
            .unwrap();

        let summary = pool.drain().await.unwrap();
        assert_eq!(summary.total_tasks_completed, 0);

        let workers = pool.store().list_workers().await.unwrap();
        for w in &workers {
            assert_eq!(w.state, WorkerState::Stopped);
        }

        // Pool rejects new work after drain.
        assert!(pool.run("hello").await.is_err());
    }

    #[tokio::test]
    async fn budget_enforcement() {
        let pool = Pool::builder(mock_claude())
            .workers(1)
            .config(GlobalWorkerConfig {
                budget_microdollars: Some(100),
                ..Default::default()
            })
            .build()
            .await
            .unwrap();

        // Simulate spending past the budget.
        pool.inner.total_spend.store(100, Ordering::Relaxed);

        let err = pool.run("hello").await.unwrap_err();
        assert!(matches!(err, Error::BudgetExhausted { .. }));
    }

    #[tokio::test]
    async fn status_snapshot() {
        let pool = Pool::builder(mock_claude())
            .workers(3)
            .config(GlobalWorkerConfig {
                budget_microdollars: Some(1_000_000),
                ..Default::default()
            })
            .build()
            .await
            .unwrap();

        let status = pool.status().await.unwrap();
        assert_eq!(status.total_workers, 3);
        assert_eq!(status.idle_workers, 3);
        assert_eq!(status.busy_workers, 0);
        assert_eq!(status.budget_microdollars, Some(1_000_000));
        assert!(!status.shutdown);
    }

    #[tokio::test]
    async fn no_idle_workers_returns_error() {
        let pool = Pool::builder(mock_claude())
            .workers(1)
            .build()
            .await
            .unwrap();

        // Manually mark the worker as busy.
        let mut workers = pool.store().list_workers().await.unwrap();
        workers[0].state = WorkerState::Busy;
        pool.store().put_worker(workers[0].clone()).await.unwrap();

        let err = pool.run("hello").await.unwrap_err();
        assert!(matches!(err, Error::NoIdleWorkers));
    }
}
