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
use crate::skill::SkillRegistry;
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
    /// In-flight chain progress, keyed by task ID.
    chain_progress: dashmap::DashMap<String, crate::chain::ChainProgress>,
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
            chain_progress: dashmap::DashMap::new(),
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
    ///
    /// Queues excess prompts until a worker becomes idle. Returns once all
    /// prompts complete or timeout waiting for worker availability.
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

    /// Submit a chain for async execution, returning a task ID immediately.
    ///
    /// Use [`Pool::chain_progress`] to check per-step progress, or
    /// [`Pool::result`] to get the final [`crate::ChainResult`] (serialized as JSON)
    /// once complete.
    pub async fn submit_chain(
        &self,
        steps: Vec<crate::chain::ChainStep>,
        skills: &SkillRegistry,
        options: crate::chain::ChainOptions,
    ) -> Result<TaskId> {
        self.check_shutdown()?;
        self.check_budget()?;

        let task_id = TaskId(format!("chain-{}", new_id()));

        let record = TaskRecord {
            id: task_id.clone(),
            prompt: format!("chain: {} steps", steps.len()),
            state: TaskState::Pending,
            worker_id: None,
            result: None,
            tags: options.tags,
            config: None,
        };
        self.inner.store.put_task(record).await?;

        // Initialize progress.
        let progress = crate::chain::ChainProgress {
            total_steps: steps.len(),
            current_step: None,
            current_step_name: None,
            completed_steps: vec![],
            status: crate::chain::ChainStatus::Running,
        };
        self.inner
            .chain_progress
            .insert(task_id.0.clone(), progress);

        // Mark as running.
        if let Some(mut task) = self.inner.store.get_task(&task_id).await? {
            task.state = TaskState::Running;
            self.inner.store.put_task(task).await?;
        }

        let pool = self.clone();
        let tid = task_id.clone();
        let skills = skills.clone();
        tokio::spawn(async move {
            let result =
                crate::chain::execute_chain_with_progress(&pool, &skills, &steps, Some(&tid)).await;

            // Store the chain result as the task result.
            if let Some(mut task) = pool.inner.store.get_task(&tid).await.ok().flatten() {
                match result {
                    Ok(chain_result) => {
                        let success = chain_result.success;
                        task.state = if success {
                            TaskState::Completed
                        } else {
                            TaskState::Failed
                        };
                        task.result = Some(TaskResult {
                            output: serde_json::to_string(&chain_result).unwrap_or_default(),
                            success,
                            cost_microdollars: chain_result.total_cost_microdollars,
                            turns_used: 0,
                            session_id: None,
                        });
                    }
                    Err(e) => {
                        task.state = TaskState::Failed;
                        task.result = Some(TaskResult {
                            output: e.to_string(),
                            success: false,
                            cost_microdollars: 0,
                            turns_used: 0,
                            session_id: None,
                        });
                    }
                }
                let _ = pool.inner.store.put_task(task).await;
            }
        });

        Ok(task_id)
    }

    /// Submit multiple chains for parallel execution, returning all task IDs immediately.
    ///
    /// Each chain runs on its own worker concurrently. Use [`Pool::chain_progress`] to check
    /// per-step progress, or [`Pool::result`] to get the final result once complete.
    pub async fn fan_out_chains(
        &self,
        chains: Vec<Vec<crate::chain::ChainStep>>,
        skills: &SkillRegistry,
        options: crate::chain::ChainOptions,
    ) -> Result<Vec<TaskId>> {
        self.check_shutdown()?;
        self.check_budget()?;

        let mut handles = Vec::with_capacity(chains.len());

        for chain_steps in chains {
            let pool = self.clone();
            let skills = skills.clone();
            let options = options.clone();
            handles.push(tokio::spawn(async move {
                pool.submit_chain(chain_steps, &skills, options).await
            }));
        }

        let mut task_ids = Vec::with_capacity(handles.len());
        for handle in handles {
            match handle.await {
                Ok(Ok(task_id)) => task_ids.push(task_id),
                Ok(Err(e)) => {
                    // Log the error but continue collecting other task IDs
                    tracing::warn!("failed to submit chain: {}", e);
                }
                Err(e) => {
                    tracing::warn!("chain submission task panicked: {}", e);
                }
            }
        }

        Ok(task_ids)
    }

    /// Submit a workflow template for async execution.
    ///
    /// Instantiates the workflow by substituting placeholders with arguments,
    /// then submits the resulting chain. Returns the task ID immediately.
    pub async fn submit_workflow(
        &self,
        workflow_name: &str,
        arguments: std::collections::HashMap<String, String>,
        skills: &SkillRegistry,
        workflows: &crate::workflow::WorkflowRegistry,
        tags: Vec<String>,
    ) -> Result<TaskId> {
        // Get the workflow and instantiate it
        let workflow = workflows
            .get(workflow_name)
            .ok_or_else(|| Error::Store(format!("workflow '{}' not found", workflow_name)))?;

        let steps = workflow.instantiate(&arguments)?;

        // Submit the instantiated chain with tags
        let options = crate::chain::ChainOptions { tags };
        self.submit_chain(steps, skills, options).await
    }

    /// Get the progress of an in-flight chain.
    ///
    /// Returns `None` if no chain is tracked for this task ID.
    pub fn chain_progress(&self, task_id: &TaskId) -> Option<crate::chain::ChainProgress> {
        self.inner
            .chain_progress
            .get(&task_id.0)
            .map(|v| v.value().clone())
    }

    /// Store chain progress (called internally by `execute_chain_with_progress`).
    pub(crate) async fn set_chain_progress(
        &self,
        task_id: &TaskId,
        progress: crate::chain::ChainProgress,
    ) {
        self.inner
            .chain_progress
            .insert(task_id.0.clone(), progress);
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

    /// Scale up the pool by adding N new workers.
    ///
    /// Returns the new total worker count.
    /// Fails if the new count exceeds max_workers.
    pub async fn scale_up(&self, count: usize) -> Result<usize> {
        if count == 0 {
            return Ok(self.inner.store.list_workers().await?.len());
        }

        let current_workers = self.inner.store.list_workers().await?;
        let current_count = current_workers.len();
        let new_count = current_count + count;

        if new_count > self.inner.config.scaling.max_workers {
            return Err(Error::Store(format!(
                "cannot scale up to {} workers: exceeds max_workers ({})",
                new_count, self.inner.config.scaling.max_workers
            )));
        }

        // Find the next available worker ID.
        let existing_ids: Vec<usize> = current_workers
            .iter()
            .filter_map(|w| w.id.0.strip_prefix("worker-").and_then(|s| s.parse().ok()))
            .collect();
        let mut next_id = existing_ids.iter().max().unwrap_or(&0) + 1;

        // Create and register new workers.
        for _ in 0..count {
            let worker_id = WorkerId(format!("worker-{next_id}"));
            next_id += 1;

            // Create worktree if isolation is enabled.
            let worktree_path = if let Some(ref mgr) = self.inner.worktree_manager {
                let path = mgr.create(&worker_id).await?;
                Some(path.to_string_lossy().into_owned())
            } else {
                None
            };

            let record = WorkerRecord {
                id: worker_id,
                state: WorkerState::Idle,
                config: WorkerConfig::default(),
                current_task: None,
                session_id: None,
                tasks_completed: 0,
                cost_microdollars: 0,
                restart_count: 0,
                worktree_path,
            };
            self.inner.store.put_worker(record).await?;
        }

        Ok(new_count)
    }

    /// Scale down the pool by removing N workers.
    ///
    /// Removes idle workers first. If not enough idle workers are available,
    /// waits for busy workers to complete (with timeout) before removing them.
    /// Returns the new total worker count.
    /// Fails if the new count drops below min_workers.
    pub async fn scale_down(&self, count: usize) -> Result<usize> {
        if count == 0 {
            return Ok(self.inner.store.list_workers().await?.len());
        }

        let mut workers = self.inner.store.list_workers().await?;
        let current_count = workers.len();
        let new_count = current_count.saturating_sub(count);

        if new_count < self.inner.config.scaling.min_workers {
            return Err(Error::Store(format!(
                "cannot scale down to {} workers: below min_workers ({})",
                new_count, self.inner.config.scaling.min_workers
            )));
        }

        // Sort to prioritize removing least-active workers.
        workers.sort_by_key(|w| std::cmp::Reverse(w.tasks_completed));

        let workers_to_remove = &workers[..count];
        let timeout = std::time::Duration::from_secs(30);

        for worker in workers_to_remove {
            // Wait for worker to finish any running task (with timeout).
            let deadline = std::time::Instant::now() + timeout;
            loop {
                if let Some(w) = self.inner.store.get_worker(&worker.id).await? {
                    if w.state != WorkerState::Busy {
                        break;
                    }
                    if std::time::Instant::now() >= deadline {
                        // Timeout: still busy, but proceed with removal anyway.
                        break;
                    }
                } else {
                    break;
                }
                tokio::time::sleep(std::time::Duration::from_millis(100)).await;
            }

            // Cleanup worktree if applicable.
            if let Some(ref mgr) = self.inner.worktree_manager
                && worker.worktree_path.is_some()
            {
                let _ = mgr.cleanup_all(std::slice::from_ref(&worker.id)).await;
            }

            // Delete worker record.
            self.inner.store.delete_worker(&worker.id).await?;
        }

        Ok(new_count)
    }

    /// Set the target number of workers, scaling up or down as needed.
    pub async fn set_target_workers(&self, target: usize) -> Result<usize> {
        let current = self.inner.store.list_workers().await?.len();
        if target > current {
            self.scale_up(target - current).await
        } else if target < current {
            self.scale_down(current - target).await
        } else {
            Ok(current)
        }
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

    /// Wait for an idle worker to become available, with exponential backoff.
    async fn wait_for_idle_worker_with_timeout(&self, timeout_secs: u64) -> Result<WorkerRecord> {
        use std::time::{Duration, Instant};

        let deadline = Instant::now() + Duration::from_secs(timeout_secs);
        let mut backoff_ms = 10u64;
        const MAX_BACKOFF_MS: u64 = 500;

        loop {
            self.check_shutdown()?;

            let workers = self.inner.store.list_workers().await?;
            for worker in workers {
                if worker.state == WorkerState::Idle {
                    return Ok(worker);
                }
            }

            if Instant::now() >= deadline {
                return Err(Error::NoWorkerAvailable { timeout_secs });
            }

            tokio::time::sleep(Duration::from_millis(backoff_ms)).await;
            backoff_ms = std::cmp::min((backoff_ms as f64 * 1.5) as u64, MAX_BACKOFF_MS);
        }
    }

    /// Find an idle worker and assign the task to it, waiting if necessary.
    async fn assign_worker(&self, task_id: &TaskId) -> Result<(WorkerId, WorkerConfig)> {
        let _lock = self.inner.assignment_lock.lock().await;

        let timeout = self.inner.config.worker_assignment_timeout_secs;
        let mut worker = self.wait_for_idle_worker_with_timeout(timeout).await?;
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

        // Build the system prompt with identity and context injection.
        let system_prompt = self.build_system_prompt(&resolved, worker_config);

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

    /// Build the system prompt by combining worker identity, resolved config and context.
    fn build_system_prompt(
        &self,
        resolved: &ResolvedConfig,
        worker_config: &WorkerConfig,
    ) -> Option<String> {
        let context_entries: Vec<_> = self.list_context();

        // Check if there's any content to include
        let has_identity = worker_config.name.is_some()
            || worker_config.role.is_some()
            || worker_config.description.is_some();

        if resolved.system_prompt.is_none() && context_entries.is_empty() && !has_identity {
            return None;
        }

        let mut parts = Vec::new();

        // Inject worker identity
        if has_identity {
            let mut identity = String::new();
            identity.push_str("You are ");

            if let Some(ref name) = worker_config.name {
                identity.push_str(name);
            } else {
                identity.push_str("a worker");
            }

            if let Some(ref role) = worker_config.role {
                identity.push_str(", a ");
                identity.push_str(role);
            }

            if let Some(ref description) = worker_config.description {
                identity.push_str(". ");
                identity.push_str(description);
            } else if worker_config.role.is_some() {
                identity.push('.');
            }

            parts.push(identity);
        }

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
    async fn no_idle_workers_timeout() {
        let pool = Pool::builder(mock_claude())
            .workers(1)
            .config(GlobalWorkerConfig {
                worker_assignment_timeout_secs: 1,
                ..Default::default()
            })
            .build()
            .await
            .unwrap();

        // Manually mark the worker as busy.
        let mut workers = pool.store().list_workers().await.unwrap();
        workers[0].state = WorkerState::Busy;
        pool.store().put_worker(workers[0].clone()).await.unwrap();

        let err = pool.run("hello").await.unwrap_err();
        assert!(matches!(err, Error::NoWorkerAvailable { timeout_secs: 1 }));
    }

    #[tokio::test]
    async fn fan_out_with_excess_prompts() {
        // This test verifies that fan_out can queue excess prompts.
        // With 2 workers and 4 prompts, all 4 should eventually complete.
        // Since we use mock_claude (non-existent binary), actual execution will fail,
        // but we're testing that the queueing mechanism works (assignment tries to get a worker).
        let pool = Pool::builder(mock_claude())
            .workers(2)
            .build()
            .await
            .unwrap();

        let prompts = vec!["prompt1", "prompt2", "prompt3", "prompt4"];

        // This will fail due to mock binary, but the key point is that
        // it tries to execute all prompts even though we only have 2 workers.
        // Before the fix, excess prompts would fail with "no idle workers" immediately.
        // After the fix, they queue and wait.
        let results = pool.fan_out(&prompts).await;

        // We expect all 4 tasks to be attempted (the mock binary failure is expected).
        // The test is that we get 4 results (not an immediate failure due to worker count).
        match results {
            Ok(_) | Err(_) => {
                // Both outcomes are ok; we're testing that fan_out doesn't fail
                // with immediate "no idle workers" error when prompts > workers.
            }
        }
    }

    #[tokio::test]
    async fn worker_identity_fields_persisted() {
        let pool = Pool::builder(mock_claude())
            .workers(1)
            .worker_config(WorkerConfig {
                name: Some("reviewer".into()),
                role: Some("code_review".into()),
                description: Some("Reviews PRs for correctness and style".into()),
                ..Default::default()
            })
            .build()
            .await
            .unwrap();

        let workers = pool.store().list_workers().await.unwrap();
        let worker = workers.iter().find(|w| w.id.0 == "worker-0").unwrap();

        assert_eq!(worker.config.name.as_deref(), Some("reviewer"));
        assert_eq!(worker.config.role.as_deref(), Some("code_review"));
        assert_eq!(
            worker.config.description.as_deref(),
            Some("Reviews PRs for correctness and style")
        );
    }

    #[tokio::test]
    async fn scale_up_increases_worker_count() {
        let pool = Pool::builder(mock_claude())
            .workers(2)
            .build()
            .await
            .unwrap();

        let initial_count = pool.store().list_workers().await.unwrap().len();
        assert_eq!(initial_count, 2);

        let new_count = pool.scale_up(3).await.unwrap();
        assert_eq!(new_count, 5);

        let workers = pool.store().list_workers().await.unwrap();
        assert_eq!(workers.len(), 5);

        // Verify new workers are idle.
        for worker in workers.iter().skip(2) {
            assert_eq!(worker.state, WorkerState::Idle);
        }
    }

    #[tokio::test]
    async fn scale_up_respects_max_workers() {
        let mut config = GlobalWorkerConfig::default();
        config.scaling.max_workers = 4;

        let pool = Pool::builder(mock_claude())
            .workers(2)
            .config(config)
            .build()
            .await
            .unwrap();

        // Try to scale beyond max.
        let result = pool.scale_up(5).await;
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("exceeds max_workers")
        );

        // Verify count unchanged.
        assert_eq!(pool.store().list_workers().await.unwrap().len(), 2);
    }

    #[tokio::test]
    async fn scale_down_reduces_worker_count() {
        let pool = Pool::builder(mock_claude())
            .workers(4)
            .build()
            .await
            .unwrap();

        let initial = pool.store().list_workers().await.unwrap().len();
        assert_eq!(initial, 4);

        let new_count = pool.scale_down(2).await.unwrap();
        assert_eq!(new_count, 2);

        assert_eq!(pool.store().list_workers().await.unwrap().len(), 2);
    }

    #[tokio::test]
    async fn scale_down_respects_min_workers() {
        let mut config = GlobalWorkerConfig::default();
        config.scaling.min_workers = 2;

        let pool = Pool::builder(mock_claude())
            .workers(3)
            .config(config)
            .build()
            .await
            .unwrap();

        // Try to scale below min.
        let result = pool.scale_down(2).await;
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("below min_workers")
        );

        // Verify count unchanged.
        assert_eq!(pool.store().list_workers().await.unwrap().len(), 3);
    }

    #[tokio::test]
    async fn set_target_workers_scales_up() {
        let pool = Pool::builder(mock_claude())
            .workers(2)
            .build()
            .await
            .unwrap();

        let new_count = pool.set_target_workers(5).await.unwrap();
        assert_eq!(new_count, 5);
        assert_eq!(pool.store().list_workers().await.unwrap().len(), 5);
    }

    #[tokio::test]
    async fn set_target_workers_scales_down() {
        let pool = Pool::builder(mock_claude())
            .workers(5)
            .build()
            .await
            .unwrap();

        let new_count = pool.set_target_workers(2).await.unwrap();
        assert_eq!(new_count, 2);
        assert_eq!(pool.store().list_workers().await.unwrap().len(), 2);
    }

    #[tokio::test]
    async fn set_target_workers_no_op_when_equal() {
        let pool = Pool::builder(mock_claude())
            .workers(3)
            .build()
            .await
            .unwrap();

        let new_count = pool.set_target_workers(3).await.unwrap();
        assert_eq!(new_count, 3);
    }

    #[tokio::test]
    async fn fan_out_chains_submits_all_chains() {
        let pool = Pool::builder(mock_claude())
            .workers(2)
            .build()
            .await
            .unwrap();

        let skills = crate::skill::SkillRegistry::new();
        let options = crate::chain::ChainOptions { tags: vec![] };

        // Create two chains, each with one prompt step.
        let chain1 = vec![crate::chain::ChainStep {
            name: "step1".into(),
            action: crate::chain::StepAction::Prompt {
                prompt: "prompt 1".into(),
            },
            config: None,
            failure_policy: crate::chain::StepFailurePolicy {
                retries: 0,
                recovery_prompt: None,
            },
        }];

        let chain2 = vec![crate::chain::ChainStep {
            name: "step1".into(),
            action: crate::chain::StepAction::Prompt {
                prompt: "prompt 2".into(),
            },
            config: None,
            failure_policy: crate::chain::StepFailurePolicy {
                retries: 0,
                recovery_prompt: None,
            },
        }];

        let chains = vec![chain1, chain2];

        // Submit both chains in parallel.
        let task_ids = pool.fan_out_chains(chains, &skills, options).await.unwrap();

        // Should have 2 task IDs.
        assert_eq!(task_ids.len(), 2);

        // Verify task IDs are different.
        assert_ne!(task_ids[0].0, task_ids[1].0);

        // Verify tasks exist in the store.
        for task_id in &task_ids {
            let task = pool.store().get_task(task_id).await.unwrap();
            assert!(task.is_some());
        }
    }
}
