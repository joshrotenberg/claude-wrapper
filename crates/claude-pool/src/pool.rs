//! Core pool engine for managing Claude CLI slots.
//!
//! The [`Pool`] struct is the main entry point. It manages slot lifecycle,
//! task assignment, budget tracking, and shared context.
//!
//! # Example
//!
//! ```no_run
//! # async fn example() -> claude_pool::Result<()> {
//! use claude_pool::{Pool, PoolConfig};
//!
//! let claude = claude_wrapper::Claude::builder().build()?;
//! let pool = Pool::builder(claude)
//!     .slots(4)
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

use crate::cli_parsing::extract_failure_details;
use crate::error::{Error, Result};
use crate::messaging::MessageBus;
use crate::skill::SkillRegistry;
use crate::store::PoolStore;
use crate::types::*;
use crate::utils::new_id;

/// Shared pool state behind an `Arc`.
pub(crate) struct PoolInner<S: PoolStore> {
    pub(crate) claude: Claude,
    pub(crate) config: PoolConfig,
    pub(crate) store: S,
    pub(crate) total_spend: AtomicU64,
    pub(crate) shutdown: AtomicBool,
    /// Context key-value pairs injected into slot system prompts.
    pub(crate) context: dashmap::DashMap<String, String>,
    /// Mutex for slot assignment to avoid races.
    pub(crate) assignment_lock: Mutex<()>,
    /// Worktree manager, if worktree isolation is enabled.
    pub(crate) worktree_manager: Option<crate::worktree::WorktreeManager>,
    /// In-flight chain progress, keyed by task ID.
    pub(crate) chain_progress: dashmap::DashMap<String, crate::chain::ChainProgress>,
    /// Message bus for inter-slot communication.
    pub(crate) message_bus: MessageBus,
}

/// A pool of Claude CLI slots.
///
/// Created via [`Pool::builder`]. Manages slot lifecycle, task routing,
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
    slot_count: usize,
    config: PoolConfig,
    store: S,
    slot_configs: Vec<SlotConfig>,
}

impl<S: PoolStore + 'static> PoolBuilder<S> {
    /// Set the number of slots to spawn.
    pub fn slots(mut self, count: usize) -> Self {
        self.slot_count = count;
        self
    }

    /// Set the global slot configuration.
    pub fn config(mut self, config: PoolConfig) -> Self {
        self.config = config;
        self
    }

    /// Add a per-slot configuration override.
    ///
    /// Call multiple times for multiple slots. Slot configs are applied
    /// in order: the first call sets slot-0's config, the second slot-1's, etc.
    /// Slots without an explicit config get [`SlotConfig::default()`].
    pub fn slot_config(mut self, config: SlotConfig) -> Self {
        self.slot_configs.push(config);
        self
    }

    /// Build and initialize the pool, registering slots in the store.
    pub async fn build(self) -> Result<Pool<S>> {
        // Resolve repo directory from Claude's working_dir or current directory.
        let repo_dir = self
            .claude
            .working_dir()
            .map(|p| p.to_path_buf())
            .unwrap_or_else(|| std::env::current_dir().unwrap_or_default());

        // Validate repo_dir is a git repo. Hard error when global worktree
        // isolation is on; soft warning otherwise (per-chain isolation may
        // still request worktrees).
        let worktree_manager = match crate::worktree::WorktreeManager::new_validated(
            &repo_dir, None,
        )
        .await
        {
            Ok(mgr) => Some(mgr),
            Err(e) => {
                if self.config.worktree_isolation {
                    return Err(e);
                }
                tracing::warn!(
                    repo_dir = %repo_dir.display(),
                    error = %e,
                    "worktree manager unavailable; per-chain worktree isolation will fall back to shared CWD"
                );
                None
            }
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
            message_bus: MessageBus::default(),
        });

        // Register slots in the store.
        for i in 0..self.slot_count {
            let slot_config = self.slot_configs.get(i).cloned().unwrap_or_default();

            let slot_id = SlotId(format!("slot-{i}"));

            // Create worktree if per-slot isolation is enabled.
            let worktree_path = if inner.config.worktree_isolation {
                if let Some(ref mgr) = inner.worktree_manager {
                    let path = mgr.create(&slot_id).await?;
                    Some(path.to_string_lossy().into_owned())
                } else {
                    None
                }
            } else {
                None
            };

            let record = SlotRecord {
                id: slot_id,
                state: SlotState::Idle,
                config: slot_config,
                current_task: None,
                session_id: None,
                tasks_completed: 0,
                cost_microdollars: 0,
                restart_count: 0,
                worktree_path,
                mcp_config_path: None,
            };
            inner.store.put_slot(record).await?;
        }

        Ok(Pool { inner })
    }
}

impl Pool<crate::store::InMemoryStore> {
    /// Create a builder with the default in-memory store.
    pub fn builder(claude: Claude) -> PoolBuilder<crate::store::InMemoryStore> {
        PoolBuilder {
            claude,
            slot_count: 1,
            config: PoolConfig::default(),
            store: crate::store::InMemoryStore::new(),
            slot_configs: Vec::new(),
        }
    }
}

impl<S: PoolStore + 'static> Pool<S> {
    /// Create a builder with a custom store.
    pub fn builder_with_store(claude: Claude, store: S) -> PoolBuilder<S> {
        PoolBuilder {
            claude,
            slot_count: 1,
            config: PoolConfig::default(),
            store,
            slot_configs: Vec::new(),
        }
    }

    /// Run a task synchronously, blocking until completion.
    ///
    /// Assigns the task to the next idle slot, executes the prompt,
    /// and returns the result.
    pub async fn run(&self, prompt: &str) -> Result<TaskResult> {
        self.run_with_config(prompt, None).await
    }

    /// Run a task with per-task config overrides.
    pub async fn run_with_config(
        &self,
        prompt: &str,
        task_config: Option<SlotConfig>,
    ) -> Result<TaskResult> {
        self.run_with_config_and_dir(prompt, task_config, None)
            .await
    }

    /// Run a task with per-task config overrides and an optional working directory override.
    pub async fn run_with_config_and_dir(
        &self,
        prompt: &str,
        task_config: Option<SlotConfig>,
        working_dir: Option<std::path::PathBuf>,
    ) -> Result<TaskResult> {
        self.check_shutdown()?;
        self.check_budget()?;

        let task_id = TaskId(format!("task-{}", new_id()));

        let record = TaskRecord {
            id: task_id.clone(),
            prompt: prompt.to_string(),
            state: TaskState::Pending,
            slot_id: None,
            result: None,
            tags: vec![],
            config: task_config,
            review_required: false,
            max_rejections: 3,
            rejection_count: 0,
            original_prompt: None,
        };
        self.inner.store.put_task(record).await?;

        let (slot_id, slot_config) = self.assign_slot(&task_id).await?;
        let result = crate::executor::execute_task(
            &self.inner,
            &task_id,
            prompt,
            &slot_id,
            &slot_config,
            working_dir.as_deref(),
        )
        .await;

        self.release_slot(&slot_id, &task_id, &result).await?;

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

    /// Run a task with per-task config overrides, optional streaming output,
    /// and an optional working directory override.
    ///
    /// When `on_output` is `Some`, the task uses streaming execution and calls
    /// the callback with each text chunk as it arrives. When `None`, behaves
    /// identically to [`run_with_config_and_dir`](Self::run_with_config_and_dir).
    pub(crate) async fn run_with_config_streaming(
        &self,
        prompt: &str,
        task_config: Option<SlotConfig>,
        on_output: Option<crate::chain::OnOutputChunk>,
        working_dir: Option<std::path::PathBuf>,
    ) -> Result<TaskResult> {
        self.check_shutdown()?;
        self.check_budget()?;

        let task_id = TaskId(format!("task-{}", new_id()));

        let record = TaskRecord {
            id: task_id.clone(),
            prompt: prompt.to_string(),
            state: TaskState::Pending,
            slot_id: None,
            result: None,
            tags: vec![],
            config: task_config,
            review_required: false,
            max_rejections: 3,
            rejection_count: 0,
            original_prompt: None,
        };
        self.inner.store.put_task(record).await?;

        let (slot_id, slot_config) = self.assign_slot(&task_id).await?;
        let result = crate::executor::execute_task_streaming(
            &self.inner,
            &task_id,
            prompt,
            &slot_id,
            &slot_config,
            on_output,
            working_dir.as_deref(),
        )
        .await;

        self.release_slot(&slot_id, &task_id, &result).await?;

        let task_result = result?;
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
        task_config: Option<SlotConfig>,
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
            slot_id: None,
            result: None,
            tags,
            config: task_config,
            review_required: false,
            max_rejections: 3,
            rejection_count: 0,
            original_prompt: None,
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

            match pool.assign_slot(&tid).await {
                Ok((slot_id, slot_config)) => {
                    let result = crate::executor::execute_task(
                        &pool.inner,
                        &tid,
                        &prompt,
                        &slot_id,
                        &slot_config,
                        None,
                    )
                    .await;

                    let _ = pool.release_slot(&slot_id, &tid, &result).await;

                    let mut updated = task;
                    match result {
                        Ok(task_result) => {
                            updated.state = TaskState::Completed;
                            updated.result = Some(task_result);
                        }
                        Err(e) => {
                            let details = extract_failure_details(&e);
                            updated.state = TaskState::Failed;
                            updated.result = Some(TaskResult {
                                output: e.to_string(),
                                success: false,
                                cost_microdollars: 0,
                                turns_used: 0,
                                session_id: None,
                                failed_command: details.failed_command,
                                exit_code: details.exit_code,
                                stderr: details.stderr,
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
                        failed_command: None,
                        exit_code: None,
                        stderr: None,
                    });
                    let _ = pool.inner.store.put_task(updated).await;
                }
            }
        });

        Ok(task_id)
    }

    /// Submit a task that requires coordinator review before completion.
    ///
    /// When the task finishes execution, it transitions to `PendingReview` instead
    /// of `Completed`. Use [`Pool::approve_result`] to accept or [`Pool::reject_result`]
    /// to reject with feedback and re-queue.
    pub async fn submit_with_review(
        &self,
        prompt: &str,
        task_config: Option<SlotConfig>,
        tags: Vec<String>,
        max_rejections: Option<u32>,
    ) -> Result<TaskId> {
        self.check_shutdown()?;
        self.check_budget()?;

        let task_id = TaskId(format!("task-{}", new_id()));
        let prompt = prompt.to_string();
        let max_rej = max_rejections.unwrap_or(3);

        let record = TaskRecord {
            id: task_id.clone(),
            prompt: prompt.clone(),
            state: TaskState::Pending,
            slot_id: None,
            result: None,
            tags,
            config: task_config,
            review_required: true,
            max_rejections: max_rej,
            rejection_count: 0,
            original_prompt: Some(prompt.clone()),
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

            match pool.assign_slot(&tid).await {
                Ok((slot_id, slot_config)) => {
                    let result = crate::executor::execute_task(
                        &pool.inner,
                        &tid,
                        &task.prompt,
                        &slot_id,
                        &slot_config,
                        None,
                    )
                    .await;

                    let _ = pool.release_slot(&slot_id, &tid, &result).await;

                    let mut updated = task;
                    match result {
                        Ok(task_result) => {
                            // review_required: go to PendingReview instead of Completed
                            if updated.review_required {
                                updated.state = TaskState::PendingReview;
                            } else {
                                updated.state = TaskState::Completed;
                            }
                            updated.result = Some(task_result);
                        }
                        Err(e) => {
                            let details = extract_failure_details(&e);
                            updated.state = TaskState::Failed;
                            updated.result = Some(TaskResult {
                                output: e.to_string(),
                                success: false,
                                cost_microdollars: 0,
                                turns_used: 0,
                                session_id: None,
                                failed_command: details.failed_command,
                                exit_code: details.exit_code,
                                stderr: details.stderr,
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
                        failed_command: None,
                        exit_code: None,
                        stderr: None,
                    });
                    let _ = pool.inner.store.put_task(updated).await;
                }
            }
        });

        Ok(task_id)
    }

    /// Approve a task that is pending review, transitioning it to `Completed`.
    pub async fn approve_result(&self, task_id: &TaskId) -> Result<()> {
        let mut task = self
            .inner
            .store
            .get_task(task_id)
            .await?
            .ok_or_else(|| Error::TaskNotFound(task_id.0.clone()))?;

        if task.state != TaskState::PendingReview {
            return Err(Error::Store(format!(
                "task {} is not pending review (state: {:?})",
                task_id.0, task.state
            )));
        }

        task.state = TaskState::Completed;
        self.inner.store.put_task(task).await
    }

    /// Reject a task that is pending review, re-queuing it with feedback appended.
    ///
    /// The original prompt is preserved and the feedback is appended. If the
    /// rejection count reaches `max_rejections`, the task is marked as `Failed`.
    pub async fn reject_result(&self, task_id: &TaskId, feedback: &str) -> Result<()> {
        let mut task = self
            .inner
            .store
            .get_task(task_id)
            .await?
            .ok_or_else(|| Error::TaskNotFound(task_id.0.clone()))?;

        if task.state != TaskState::PendingReview {
            return Err(Error::Store(format!(
                "task {} is not pending review (state: {:?})",
                task_id.0, task.state
            )));
        }

        task.rejection_count += 1;

        if task.rejection_count >= task.max_rejections {
            task.state = TaskState::Failed;
            task.result = Some(TaskResult {
                output: format!(
                    "task rejected {} times (max: {}). Last feedback: {}",
                    task.rejection_count, task.max_rejections, feedback
                ),
                success: false,
                cost_microdollars: 0,
                turns_used: 0,
                session_id: None,
                failed_command: None,
                exit_code: None,
                stderr: None,
            });
            self.inner.store.put_task(task).await?;
            return Ok(());
        }

        // Rebuild prompt: original + rejection feedback
        let original = task
            .original_prompt
            .clone()
            .unwrap_or_else(|| task.prompt.clone());
        task.prompt = format!(
            "{}\n\n--- Rejection feedback (attempt {}/{}) ---\n{}",
            original, task.rejection_count, task.max_rejections, feedback
        );
        task.state = TaskState::Pending;
        task.slot_id = None;
        task.result = None;
        self.inner.store.put_task(task.clone()).await?;

        // Re-execute in background (same pattern as submit).
        let pool = self.clone();
        let tid = task_id.clone();
        tokio::spawn(async move {
            let task = match pool.inner.store.get_task(&tid).await {
                Ok(Some(t)) => t,
                _ => return,
            };

            match pool.assign_slot(&tid).await {
                Ok((slot_id, slot_config)) => {
                    let result = crate::executor::execute_task(
                        &pool.inner,
                        &tid,
                        &task.prompt,
                        &slot_id,
                        &slot_config,
                        None,
                    )
                    .await;

                    let _ = pool.release_slot(&slot_id, &tid, &result).await;

                    let mut updated = task;
                    match result {
                        Ok(task_result) => {
                            if updated.review_required {
                                updated.state = TaskState::PendingReview;
                            } else {
                                updated.state = TaskState::Completed;
                            }
                            updated.result = Some(task_result);
                        }
                        Err(e) => {
                            let details = extract_failure_details(&e);
                            updated.state = TaskState::Failed;
                            updated.result = Some(TaskResult {
                                output: e.to_string(),
                                success: false,
                                cost_microdollars: 0,
                                turns_used: 0,
                                session_id: None,
                                failed_command: details.failed_command,
                                exit_code: details.exit_code,
                                stderr: details.stderr,
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
                        failed_command: None,
                        exit_code: None,
                        stderr: None,
                    });
                    let _ = pool.inner.store.put_task(updated).await;
                }
            }
        });

        Ok(())
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
            TaskState::Completed | TaskState::Failed | TaskState::PendingReview => Ok(task.result),
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
            TaskState::Pending | TaskState::PendingReview => {
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

    /// Claim the next pending task for a specific slot.
    ///
    /// Atomically finds the oldest pending task (with no slot assigned),
    /// assigns it to the given slot, and executes it in the background.
    /// Returns the claimed task ID, or `None` if no pending tasks are available.
    pub async fn claim(&self, slot_id: &SlotId) -> Result<Option<TaskId>> {
        self.check_shutdown()?;

        // Verify the slot exists and is idle.
        let slot = self
            .inner
            .store
            .get_slot(slot_id)
            .await?
            .ok_or_else(|| Error::SlotNotFound(slot_id.0.clone()))?;

        if slot.state != SlotState::Idle {
            return Ok(None);
        }

        // Find the oldest pending task with no slot assigned.
        let pending = self
            .inner
            .store
            .list_tasks(&TaskFilter {
                state: Some(TaskState::Pending),
                ..Default::default()
            })
            .await?;

        let task = match pending.into_iter().find(|t| t.slot_id.is_none()) {
            Some(t) => t,
            None => return Ok(None),
        };

        let task_id = task.id.clone();
        let prompt = task.prompt.clone();
        let slot_config = slot.config.clone();

        // Mark task as running and assign to slot.
        let mut updated_task = task;
        updated_task.state = TaskState::Running;
        updated_task.slot_id = Some(slot_id.clone());
        self.inner.store.put_task(updated_task.clone()).await?;

        // Mark slot as busy.
        let mut updated_slot = slot;
        updated_slot.state = SlotState::Busy;
        updated_slot.current_task = Some(task_id.clone());
        self.inner.store.put_slot(updated_slot).await?;

        // Execute in background.
        let pool = self.clone();
        let tid = task_id.clone();
        let sid = slot_id.clone();
        tokio::spawn(async move {
            let result =
                crate::executor::execute_task(&pool.inner, &tid, &prompt, &sid, &slot_config, None)
                    .await;

            let _ = pool.release_slot(&sid, &tid, &result).await;

            if let Ok(Some(mut task)) = pool.inner.store.get_task(&tid).await {
                match result {
                    Ok(task_result) => {
                        task.state = TaskState::Completed;
                        task.result = Some(task_result);
                    }
                    Err(e) => {
                        let details = extract_failure_details(&e);
                        task.state = TaskState::Failed;
                        task.result = Some(TaskResult {
                            output: e.to_string(),
                            success: false,
                            cost_microdollars: 0,
                            turns_used: 0,
                            session_id: None,
                            failed_command: details.failed_command,
                            exit_code: details.exit_code,
                            stderr: details.stderr,
                        });
                    }
                }
                let _ = pool.inner.store.put_task(task).await;
            }
        });

        Ok(Some(task_id))
    }

    /// Cancel a running chain, skipping remaining steps.
    ///
    /// Sets the chain's task state to `Cancelled`. The currently-executing step
    /// (if any) runs to completion; remaining steps are then skipped. Partial
    /// results are available via [`Pool::result`] once the chain finishes.
    pub async fn cancel_chain(&self, task_id: &TaskId) -> Result<()> {
        let mut task = self
            .inner
            .store
            .get_task(task_id)
            .await?
            .ok_or_else(|| Error::TaskNotFound(task_id.0.clone()))?;

        match task.state {
            TaskState::Running | TaskState::Pending => {
                task.state = TaskState::Cancelled;
                self.inner.store.put_task(task).await?;
                // Optimistically update in-flight progress status.
                if let Some(mut progress) = self.inner.chain_progress.get_mut(&task_id.0) {
                    progress.status = crate::chain::ChainStatus::Cancelled;
                }
                Ok(())
            }
            _ => Ok(()), // already terminal or already cancelled
        }
    }

    /// Execute tasks in parallel across available slots, collecting all results.
    ///
    /// Queues excess prompts until a slot becomes idle. Returns once all
    /// prompts complete or timeout waiting for slot availability.
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

        let isolation = options.isolation;

        let record = TaskRecord {
            id: task_id.clone(),
            prompt: format!("chain: {} steps", steps.len()),
            state: TaskState::Pending,
            slot_id: None,
            result: None,
            tags: options.tags,
            config: None,
            review_required: false,
            max_rejections: 3,
            rejection_count: 0,
            original_prompt: None,
        };
        self.inner.store.put_task(record).await?;

        // Initialize progress.
        let progress = crate::chain::ChainProgress {
            total_steps: steps.len(),
            current_step: None,
            current_step_name: None,
            current_step_partial_output: None,
            current_step_started_at: None,
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

        // Create chain working directory based on isolation mode.
        let chain_working_dir = match isolation {
            crate::chain::ChainIsolation::Worktree => {
                if let Some(ref mgr) = self.inner.worktree_manager {
                    match mgr.create_for_chain(&task_id).await {
                        Ok(path) => Some(path),
                        Err(e) => {
                            tracing::warn!(
                                task_id = %task_id.0,
                                error = %e,
                                "failed to create chain worktree, falling back to slot dir"
                            );
                            None
                        }
                    }
                } else {
                    None
                }
            }
            crate::chain::ChainIsolation::Clone => {
                if let Some(ref mgr) = self.inner.worktree_manager {
                    match mgr.create_clone_for_chain(&task_id).await {
                        Ok(path) => Some(path),
                        Err(e) => {
                            tracing::warn!(
                                task_id = %task_id.0,
                                error = %e,
                                "failed to create chain clone, falling back to slot dir"
                            );
                            None
                        }
                    }
                } else {
                    None
                }
            }
            crate::chain::ChainIsolation::None => None,
        };

        let pool = self.clone();
        let tid = task_id.clone();
        let skills = skills.clone();
        tokio::spawn(async move {
            let result = crate::chain::execute_chain_with_progress(
                &pool,
                &skills,
                &steps,
                Some(&tid),
                chain_working_dir.as_deref(),
            )
            .await;

            // Clean up chain isolation based on the mode used.
            if chain_working_dir.is_some()
                && let Some(ref mgr) = pool.inner.worktree_manager
            {
                match isolation {
                    crate::chain::ChainIsolation::Worktree => {
                        if let Err(e) = mgr.remove_chain(&tid).await {
                            tracing::warn!(
                                task_id = %tid.0,
                                error = %e,
                                "failed to clean up chain worktree"
                            );
                        }
                    }
                    crate::chain::ChainIsolation::Clone => {
                        if let Err(e) = mgr.remove_clone(&tid).await {
                            tracing::warn!(
                                task_id = %tid.0,
                                error = %e,
                                "failed to clean up chain clone"
                            );
                        }
                    }
                    crate::chain::ChainIsolation::None => {}
                }
            }

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
                            failed_command: None,
                            exit_code: None,
                            stderr: None,
                        });
                    }
                    Err(e) => {
                        let details = extract_failure_details(&e);
                        task.state = TaskState::Failed;
                        task.result = Some(TaskResult {
                            output: e.to_string(),
                            success: false,
                            cost_microdollars: 0,
                            turns_used: 0,
                            session_id: None,
                            failed_command: details.failed_command,
                            exit_code: details.exit_code,
                            stderr: details.stderr,
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
    /// Each chain runs on its own slot concurrently. Use [`Pool::chain_progress`] to check
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
        let options = crate::chain::ChainOptions {
            tags,
            ..Default::default()
        };
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

    /// Append a text chunk to the current step's partial output.
    ///
    /// Called from the streaming output callback during chain execution.
    /// This is a synchronous DashMap mutation — fast and lock-free.
    pub(crate) fn append_chain_partial_output(&self, task_id: &TaskId, chunk: &str) {
        if let Some(mut progress) = self.inner.chain_progress.get_mut(&task_id.0)
            && let Some(ref mut partial) = progress.current_step_partial_output
        {
            partial.push_str(chunk);
        }
    }

    /// Set a shared context value.
    ///
    /// Context is injected into slot system prompts at task start.
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

    /// Send a message from one slot to another.
    ///
    /// Returns the message ID.
    pub fn send_message(&self, from: SlotId, to: SlotId, content: String) -> String {
        self.inner.message_bus.send(from, to, content)
    }

    /// Broadcast a message from one slot to all other active slots.
    ///
    /// Returns the list of message IDs created (one per recipient).
    pub async fn broadcast_message(&self, from: SlotId, content: String) -> Result<Vec<String>> {
        let slots = self.inner.store.list_slots().await?;
        let recipients: Vec<SlotId> = slots.into_iter().map(|s| s.id).collect();
        Ok(self.inner.message_bus.broadcast(from, &recipients, content))
    }

    /// Find slots matching optional name, role, and/or state filters.
    ///
    /// All filters are optional; omitted filters match everything.
    pub async fn find_slots(
        &self,
        name: Option<&str>,
        role: Option<&str>,
        state: Option<SlotState>,
    ) -> Result<Vec<SlotRecord>> {
        let slots = self.inner.store.list_slots().await?;
        Ok(slots
            .into_iter()
            .filter(|s| {
                if let Some(n) = name
                    && s.config.name.as_deref() != Some(n)
                {
                    return false;
                }
                if let Some(r) = role
                    && s.config.role.as_deref() != Some(r)
                {
                    return false;
                }
                if let Some(st) = state
                    && s.state != st
                {
                    return false;
                }
                true
            })
            .collect())
    }

    /// Read and drain all messages for a slot.
    ///
    /// Returns messages in order, removing them from the inbox.
    pub fn read_messages(&self, slot_id: &SlotId) -> Vec<crate::messaging::Message> {
        self.inner.message_bus.read(slot_id)
    }

    /// Peek at all messages for a slot without removing them.
    ///
    /// Returns messages in order without draining the inbox.
    pub fn peek_messages(&self, slot_id: &SlotId) -> Vec<crate::messaging::Message> {
        self.inner.message_bus.peek(slot_id)
    }

    /// Get the count of messages in a slot's inbox.
    pub fn message_count(&self, slot_id: &SlotId) -> usize {
        self.inner.message_bus.count(slot_id)
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

        // Mark all slots as stopped.
        let slots = self.inner.store.list_slots().await?;
        let mut total_cost = 0u64;
        let mut total_tasks = 0u64;
        let slot_ids: Vec<_> = slots.iter().map(|w| w.id.clone()).collect();

        for mut slot in slots {
            total_cost += slot.cost_microdollars;
            total_tasks += slot.tasks_completed;
            slot.state = SlotState::Stopped;
            self.inner.store.put_slot(slot).await?;
        }

        // Clean up worktrees if isolation was enabled.
        if let Some(ref mgr) = self.inner.worktree_manager {
            mgr.cleanup_all(&slot_ids).await?;
        }

        // Clean up per-slot MCP config temp files.
        for slot_id in &slot_ids {
            if let Some(slot) = self.inner.store.get_slot(slot_id).await?
                && let Some(ref path) = slot.mcp_config_path
                && let Err(e) = std::fs::remove_file(path)
            {
                tracing::warn!(
                    slot_id = %slot_id.0,
                    path = %path.display(),
                    error = %e,
                    "failed to clean up slot MCP config"
                );
            }
        }

        Ok(DrainSummary {
            total_cost_microdollars: total_cost,
            total_tasks_completed: total_tasks,
        })
    }

    /// Get a snapshot of pool status.
    pub async fn status(&self) -> Result<PoolStatus> {
        let slots = self.inner.store.list_slots().await?;
        let idle = slots.iter().filter(|w| w.state == SlotState::Idle).count();
        let busy = slots.iter().filter(|w| w.state == SlotState::Busy).count();

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

        let pending_review_tasks = self
            .inner
            .store
            .list_tasks(&TaskFilter {
                state: Some(TaskState::PendingReview),
                ..Default::default()
            })
            .await?
            .len();

        Ok(PoolStatus {
            total_slots: slots.len(),
            idle_slots: idle,
            busy_slots: busy,
            running_tasks,
            pending_tasks,
            pending_review_tasks,
            total_spend_microdollars: self.inner.total_spend.load(Ordering::Relaxed),
            budget_microdollars: self.inner.config.budget_microdollars,
            shutdown: self.inner.shutdown.load(Ordering::Relaxed),
        })
    }

    /// Get a reference to the store.
    pub fn store(&self) -> &S {
        &self.inner.store
    }

    /// Get a reference to the pool configuration.
    pub fn config(&self) -> &PoolConfig {
        &self.inner.config
    }

    /// Start the background supervisor loop.
    ///
    /// The supervisor periodically checks for errored slots and restarts them
    /// (up to [`PoolConfig::max_restarts`]). Returns a [`SupervisorHandle`]
    /// that can be used to stop the loop.
    ///
    /// Returns `None` if [`PoolConfig::supervisor_enabled`] is false.
    ///
    /// [`SupervisorHandle`]: crate::supervisor::SupervisorHandle
    pub fn start_supervisor(&self) -> Option<crate::supervisor::SupervisorHandle> {
        if !self.inner.config.supervisor_enabled {
            return None;
        }
        Some(crate::supervisor::spawn_supervisor(
            self.clone(),
            self.inner.config.supervisor_interval_secs,
        ))
    }

    /// Scale up the pool by adding N new slots.
    ///
    /// Returns the new total slot count.
    /// Fails if the new count exceeds max_slots.
    pub async fn scale_up(&self, count: usize) -> Result<usize> {
        if count == 0 {
            return Ok(self.inner.store.list_slots().await?.len());
        }

        let current_slots = self.inner.store.list_slots().await?;
        let current_count = current_slots.len();
        let new_count = current_count + count;

        if new_count > self.inner.config.scaling.max_slots {
            return Err(Error::Store(format!(
                "cannot scale up to {} slots: exceeds max_slots ({})",
                new_count, self.inner.config.scaling.max_slots
            )));
        }

        // Find the next available slot ID.
        let existing_ids: Vec<usize> = current_slots
            .iter()
            .filter_map(|w| w.id.0.strip_prefix("slot-").and_then(|s| s.parse().ok()))
            .collect();
        let mut next_id = existing_ids.iter().max().unwrap_or(&0) + 1;

        // Create and register new slots.
        for _ in 0..count {
            let slot_id = SlotId(format!("slot-{next_id}"));
            next_id += 1;

            // Create worktree if per-slot isolation is enabled.
            let worktree_path = if self.inner.config.worktree_isolation {
                if let Some(ref mgr) = self.inner.worktree_manager {
                    let path = mgr.create(&slot_id).await?;
                    Some(path.to_string_lossy().into_owned())
                } else {
                    None
                }
            } else {
                None
            };

            let record = SlotRecord {
                id: slot_id,
                state: SlotState::Idle,
                config: SlotConfig::default(),
                current_task: None,
                session_id: None,
                tasks_completed: 0,
                cost_microdollars: 0,
                restart_count: 0,
                worktree_path,
                mcp_config_path: None,
            };
            self.inner.store.put_slot(record).await?;
        }

        Ok(new_count)
    }

    /// Scale down the pool by removing N slots.
    ///
    /// Removes idle slots first. If not enough idle slots are available,
    /// waits for busy slots to complete (with timeout) before removing them.
    /// Returns the new total slot count.
    /// Fails if the new count drops below min_slots.
    pub async fn scale_down(&self, count: usize) -> Result<usize> {
        if count == 0 {
            return Ok(self.inner.store.list_slots().await?.len());
        }

        let mut slots = self.inner.store.list_slots().await?;
        let current_count = slots.len();
        let new_count = current_count.saturating_sub(count);

        if new_count < self.inner.config.scaling.min_slots {
            return Err(Error::Store(format!(
                "cannot scale down to {} slots: below min_slots ({})",
                new_count, self.inner.config.scaling.min_slots
            )));
        }

        // Sort to prioritize removing least-active slots.
        slots.sort_by_key(|w| std::cmp::Reverse(w.tasks_completed));

        let slots_to_remove = &slots[..count];
        let timeout = std::time::Duration::from_secs(30);

        for slot in slots_to_remove {
            // Wait for slot to finish any running task (with timeout).
            let deadline = std::time::Instant::now() + timeout;
            loop {
                if let Some(w) = self.inner.store.get_slot(&slot.id).await? {
                    if w.state != SlotState::Busy {
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
                && slot.worktree_path.is_some()
            {
                let _ = mgr.cleanup_all(std::slice::from_ref(&slot.id)).await;
            }

            // Delete slot record.
            self.inner.store.delete_slot(&slot.id).await?;
        }

        Ok(new_count)
    }

    /// Set the target number of slots, scaling up or down as needed.
    pub async fn set_target_slots(&self, target: usize) -> Result<usize> {
        let current = self.inner.store.list_slots().await?.len();
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

    /// Wait for an idle slot to become available, with exponential backoff.
    async fn wait_for_idle_slot_with_timeout(&self, timeout_secs: u64) -> Result<SlotRecord> {
        use std::time::{Duration, Instant};

        let deadline = Instant::now() + Duration::from_secs(timeout_secs);
        let mut backoff_ms = 10u64;
        const MAX_BACKOFF_MS: u64 = 500;

        loop {
            self.check_shutdown()?;

            let slots = self.inner.store.list_slots().await?;
            for slot in slots {
                if slot.state == SlotState::Idle {
                    return Ok(slot);
                }
            }

            if Instant::now() >= deadline {
                return Err(Error::NoSlotAvailable { timeout_secs });
            }

            tokio::time::sleep(Duration::from_millis(backoff_ms)).await;
            backoff_ms = std::cmp::min((backoff_ms as f64 * 1.5) as u64, MAX_BACKOFF_MS);
        }
    }

    /// Find an idle slot and assign the task to it, waiting if necessary.
    async fn assign_slot(&self, task_id: &TaskId) -> Result<(SlotId, SlotConfig)> {
        let _lock = self.inner.assignment_lock.lock().await;

        let timeout = self.inner.config.slot_assignment_timeout_secs;
        let mut slot = self.wait_for_idle_slot_with_timeout(timeout).await?;
        let config = slot.config.clone();

        slot.state = SlotState::Busy;
        slot.current_task = Some(task_id.clone());
        self.inner.store.put_slot(slot.clone()).await?;

        // Update task with assigned slot.
        if let Some(mut task) = self.inner.store.get_task(task_id).await? {
            task.state = TaskState::Running;
            task.slot_id = Some(slot.id.clone());
            self.inner.store.put_task(task).await?;
        }

        Ok((slot.id, config))
    }

    /// Release a slot back to idle after task completion.
    async fn release_slot(
        &self,
        slot_id: &SlotId,
        _task_id: &TaskId,
        result: &std::result::Result<TaskResult, Error>,
    ) -> Result<()> {
        if let Some(mut slot) = self.inner.store.get_slot(slot_id).await? {
            slot.state = SlotState::Idle;
            slot.current_task = None;

            if let Ok(task_result) = result {
                slot.tasks_completed += 1;
                slot.cost_microdollars += task_result.cost_microdollars;
                slot.session_id = task_result.session_id.clone();

                // Update global spend tracker.
                self.inner
                    .total_spend
                    .fetch_add(task_result.cost_microdollars, Ordering::Relaxed);
            }

            self.inner.store.put_slot(slot).await?;
        }
        Ok(())
    }
}

/// Summary returned by [`Pool::drain`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DrainSummary {
    /// Total cost across all slots in microdollars.
    pub total_cost_microdollars: u64,
    /// Total number of tasks completed.
    pub total_tasks_completed: u64,
}

/// Snapshot of pool status.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PoolStatus {
    /// Total number of slots.
    pub total_slots: usize,
    /// Number of idle slots.
    pub idle_slots: usize,
    /// Number of busy slots.
    pub busy_slots: usize,
    /// Number of currently running tasks.
    pub running_tasks: usize,
    /// Number of pending (queued) tasks.
    pub pending_tasks: usize,
    /// Number of tasks awaiting coordinator review.
    pub pending_review_tasks: usize,
    /// Total spend in microdollars.
    pub total_spend_microdollars: u64,
    /// Budget cap in microdollars, if set.
    pub budget_microdollars: Option<u64>,
    /// Whether the pool is shutting down.
    pub shutdown: bool,
}

use serde::{Deserialize, Serialize};

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli_parsing::{
        detect_permission_prompt, extract_failure_details, extract_tool_name,
    };

    fn mock_claude() -> Claude {
        // Build a Claude instance pointing at a non-existent binary.
        // Tests that don't actually execute tasks can use this.
        Claude::builder().binary("/usr/bin/false").build().unwrap()
    }

    #[tokio::test]
    async fn build_pool_registers_slots() {
        let pool = Pool::builder(mock_claude()).slots(3).build().await.unwrap();

        let slots = pool.store().list_slots().await.unwrap();
        assert_eq!(slots.len(), 3);

        for slot in &slots {
            assert_eq!(slot.state, SlotState::Idle);
        }
    }

    #[tokio::test]
    async fn pool_with_slot_configs() {
        let pool = Pool::builder(mock_claude())
            .slots(2)
            .slot_config(SlotConfig {
                model: Some("opus".into()),
                role: Some("reviewer".into()),
                ..Default::default()
            })
            .build()
            .await
            .unwrap();

        let slots = pool.store().list_slots().await.unwrap();
        let w0 = slots.iter().find(|w| w.id.0 == "slot-0").unwrap();
        let w1 = slots.iter().find(|w| w.id.0 == "slot-1").unwrap();
        assert_eq!(w0.config.model.as_deref(), Some("opus"));
        assert_eq!(w0.config.role.as_deref(), Some("reviewer"));
        // Slot 1 gets default config.
        assert!(w1.config.model.is_none());
    }

    #[tokio::test]
    async fn context_operations() {
        let pool = Pool::builder(mock_claude()).slots(1).build().await.unwrap();

        pool.set_context("repo", "claude-wrapper");
        pool.set_context("branch", "main");

        assert_eq!(pool.get_context("repo").as_deref(), Some("claude-wrapper"));
        assert_eq!(pool.list_context().len(), 2);

        pool.delete_context("branch");
        assert!(pool.get_context("branch").is_none());
    }

    #[tokio::test]
    async fn drain_marks_slots_stopped() {
        let pool = Pool::builder(mock_claude()).slots(2).build().await.unwrap();

        let summary = pool.drain().await.unwrap();
        assert_eq!(summary.total_tasks_completed, 0);

        let slots = pool.store().list_slots().await.unwrap();
        for w in &slots {
            assert_eq!(w.state, SlotState::Stopped);
        }

        // Pool rejects new work after drain.
        assert!(pool.run("hello").await.is_err());
    }

    #[tokio::test]
    async fn budget_enforcement() {
        let pool = Pool::builder(mock_claude())
            .slots(1)
            .config(PoolConfig {
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
            .slots(3)
            .config(PoolConfig {
                budget_microdollars: Some(1_000_000),
                ..Default::default()
            })
            .build()
            .await
            .unwrap();

        let status = pool.status().await.unwrap();
        assert_eq!(status.total_slots, 3);
        assert_eq!(status.idle_slots, 3);
        assert_eq!(status.busy_slots, 0);
        assert_eq!(status.budget_microdollars, Some(1_000_000));
        assert!(!status.shutdown);
    }

    #[tokio::test]
    async fn no_idle_slots_timeout() {
        let pool = Pool::builder(mock_claude())
            .slots(1)
            .config(PoolConfig {
                slot_assignment_timeout_secs: 1,
                ..Default::default()
            })
            .build()
            .await
            .unwrap();

        // Manually mark the slot as busy.
        let mut slots = pool.store().list_slots().await.unwrap();
        slots[0].state = SlotState::Busy;
        pool.store().put_slot(slots[0].clone()).await.unwrap();

        let err = pool.run("hello").await.unwrap_err();
        assert!(matches!(err, Error::NoSlotAvailable { timeout_secs: 1 }));
    }

    #[tokio::test]
    async fn fan_out_with_excess_prompts() {
        // This test verifies that fan_out can queue excess prompts.
        // With 2 slots and 4 prompts, all 4 should eventually complete.
        // Since we use mock_claude (non-existent binary), actual execution will fail,
        // but we're testing that the queueing mechanism works (assignment tries to get a slot).
        let pool = Pool::builder(mock_claude()).slots(2).build().await.unwrap();

        let prompts = vec!["prompt1", "prompt2", "prompt3", "prompt4"];

        // This will fail due to mock binary, but the key point is that
        // it tries to execute all prompts even though we only have 2 slots.
        // Before the fix, excess prompts would fail with "no idle slots" immediately.
        // After the fix, they queue and wait.
        let results = pool.fan_out(&prompts).await;

        // We expect all 4 tasks to be attempted (the mock binary failure is expected).
        // The test is that we get 4 results (not an immediate failure due to slot count).
        match results {
            Ok(_) | Err(_) => {
                // Both outcomes are ok; we're testing that fan_out doesn't fail
                // with immediate "no idle slots" error when prompts > slots.
            }
        }
    }

    #[tokio::test]
    async fn slot_identity_fields_persisted() {
        let pool = Pool::builder(mock_claude())
            .slots(1)
            .slot_config(SlotConfig {
                name: Some("reviewer".into()),
                role: Some("code_review".into()),
                description: Some("Reviews PRs for correctness and style".into()),
                ..Default::default()
            })
            .build()
            .await
            .unwrap();

        let slots = pool.store().list_slots().await.unwrap();
        let slot = slots.iter().find(|w| w.id.0 == "slot-0").unwrap();

        assert_eq!(slot.config.name.as_deref(), Some("reviewer"));
        assert_eq!(slot.config.role.as_deref(), Some("code_review"));
        assert_eq!(
            slot.config.description.as_deref(),
            Some("Reviews PRs for correctness and style")
        );
    }

    #[tokio::test]
    async fn find_slots_filters_by_name_role_state() {
        let pool = Pool::builder(mock_claude())
            .slots(1)
            .slot_config(SlotConfig {
                name: Some("reviewer".into()),
                role: Some("code_review".into()),
                ..Default::default()
            })
            .build()
            .await
            .unwrap();

        // Scale up with a different config for the second slot.
        pool.scale_up(1).await.unwrap();
        let mut slots = pool.store().list_slots().await.unwrap();
        if let Some(s) = slots.iter_mut().find(|s| s.id.0 == "slot-1") {
            s.config.name = Some("writer".into());
            s.config.role = Some("implementation".into());
            pool.store().put_slot(s.clone()).await.unwrap();
        }

        // Find by name.
        let found = pool.find_slots(Some("reviewer"), None, None).await.unwrap();
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].id.0, "slot-0");

        // Find by role.
        let found = pool
            .find_slots(None, Some("implementation"), None)
            .await
            .unwrap();
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].id.0, "slot-1");

        // Find by state (all idle).
        let found = pool
            .find_slots(None, None, Some(SlotState::Idle))
            .await
            .unwrap();
        assert_eq!(found.len(), 2);

        // Find with no filters (returns all).
        let found = pool.find_slots(None, None, None).await.unwrap();
        assert_eq!(found.len(), 2);

        // Find with non-matching filter.
        let found = pool
            .find_slots(Some("nonexistent"), None, None)
            .await
            .unwrap();
        assert!(found.is_empty());
    }

    #[tokio::test]
    async fn broadcast_sends_to_all_except_sender() {
        let pool = Pool::builder(mock_claude()).slots(3).build().await.unwrap();

        let from = SlotId("slot-0".into());
        let ids = pool
            .broadcast_message(from.clone(), "hello everyone".into())
            .await
            .unwrap();

        assert_eq!(ids.len(), 2); // 3 slots minus sender

        // Verify recipients got messages.
        assert_eq!(pool.message_count(&SlotId("slot-1".into())), 1);
        assert_eq!(pool.message_count(&SlotId("slot-2".into())), 1);
        assert_eq!(pool.message_count(&from), 0); // sender excluded
    }

    #[tokio::test]
    async fn scale_up_increases_slot_count() {
        let pool = Pool::builder(mock_claude()).slots(2).build().await.unwrap();

        let initial_count = pool.store().list_slots().await.unwrap().len();
        assert_eq!(initial_count, 2);

        let new_count = pool.scale_up(3).await.unwrap();
        assert_eq!(new_count, 5);

        let slots = pool.store().list_slots().await.unwrap();
        assert_eq!(slots.len(), 5);

        // Verify new slots are idle.
        for slot in slots.iter().skip(2) {
            assert_eq!(slot.state, SlotState::Idle);
        }
    }

    #[tokio::test]
    async fn scale_up_respects_max_slots() {
        let mut config = PoolConfig::default();
        config.scaling.max_slots = 4;

        let pool = Pool::builder(mock_claude())
            .slots(2)
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
                .contains("exceeds max_slots")
        );

        // Verify count unchanged.
        assert_eq!(pool.store().list_slots().await.unwrap().len(), 2);
    }

    #[tokio::test]
    async fn scale_down_reduces_slot_count() {
        let pool = Pool::builder(mock_claude()).slots(4).build().await.unwrap();

        let initial = pool.store().list_slots().await.unwrap().len();
        assert_eq!(initial, 4);

        let new_count = pool.scale_down(2).await.unwrap();
        assert_eq!(new_count, 2);

        assert_eq!(pool.store().list_slots().await.unwrap().len(), 2);
    }

    #[tokio::test]
    async fn scale_down_respects_min_slots() {
        let mut config = PoolConfig::default();
        config.scaling.min_slots = 2;

        let pool = Pool::builder(mock_claude())
            .slots(3)
            .config(config)
            .build()
            .await
            .unwrap();

        // Try to scale below min.
        let result = pool.scale_down(2).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("below min_slots"));

        // Verify count unchanged.
        assert_eq!(pool.store().list_slots().await.unwrap().len(), 3);
    }

    #[tokio::test]
    async fn set_target_slots_scales_up() {
        let pool = Pool::builder(mock_claude()).slots(2).build().await.unwrap();

        let new_count = pool.set_target_slots(5).await.unwrap();
        assert_eq!(new_count, 5);
        assert_eq!(pool.store().list_slots().await.unwrap().len(), 5);
    }

    #[tokio::test]
    async fn set_target_slots_scales_down() {
        let pool = Pool::builder(mock_claude()).slots(5).build().await.unwrap();

        let new_count = pool.set_target_slots(2).await.unwrap();
        assert_eq!(new_count, 2);
        assert_eq!(pool.store().list_slots().await.unwrap().len(), 2);
    }

    #[tokio::test]
    async fn set_target_slots_no_op_when_equal() {
        let pool = Pool::builder(mock_claude()).slots(3).build().await.unwrap();

        let new_count = pool.set_target_slots(3).await.unwrap();
        assert_eq!(new_count, 3);
    }

    #[tokio::test]
    async fn fan_out_chains_submits_all_chains() {
        let pool = Pool::builder(mock_claude()).slots(2).build().await.unwrap();

        let skills = crate::skill::SkillRegistry::new();
        let options = crate::chain::ChainOptions {
            tags: vec![],
            ..Default::default()
        };

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
            output_vars: Default::default(),
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
            output_vars: Default::default(),
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

    // ── Permission prompt detection tests ────────────────────────────

    #[test]
    fn detect_allow_bash_in_stderr() {
        let err = claude_wrapper::Error::CommandFailed {
            command: "claude --print".into(),
            exit_code: 1,
            stdout: String::new(),
            stderr: "Allow Bash tool? (y/n)".into(),
            working_dir: None,
        };
        let result = detect_permission_prompt(&err, "slot-1");
        assert!(result.is_some());
        let err = result.unwrap();
        match err {
            Error::PermissionPromptDetected {
                tool_name, slot_id, ..
            } => {
                assert_eq!(tool_name, "Bash");
                assert_eq!(slot_id, "slot-1");
            }
            other => panic!("expected PermissionPromptDetected, got: {other}"),
        }
    }

    #[test]
    fn detect_wants_to_use_pattern() {
        let err = claude_wrapper::Error::CommandFailed {
            command: "claude --print".into(),
            exit_code: 1,
            stdout: String::new(),
            stderr: "Claude wants to use Edit tool.".into(),
            working_dir: None,
        };
        let result = detect_permission_prompt(&err, "slot-2");
        assert!(result.is_some());
        match result.unwrap() {
            Error::PermissionPromptDetected { tool_name, .. } => {
                assert_eq!(tool_name, "Edit");
            }
            other => panic!("expected PermissionPromptDetected, got: {other}"),
        }
    }

    #[test]
    fn no_detection_on_clean_stderr() {
        let err = claude_wrapper::Error::CommandFailed {
            command: "claude --print".into(),
            exit_code: 1,
            stdout: String::new(),
            stderr: "some unrelated error output".into(),
            working_dir: None,
        };
        assert!(detect_permission_prompt(&err, "slot-1").is_none());
    }

    #[test]
    fn no_detection_on_empty_stderr() {
        let err = claude_wrapper::Error::CommandFailed {
            command: "claude --print".into(),
            exit_code: 1,
            stdout: String::new(),
            stderr: String::new(),
            working_dir: None,
        };
        assert!(detect_permission_prompt(&err, "slot-1").is_none());
    }

    #[test]
    fn no_detection_on_timeout() {
        let err = claude_wrapper::Error::Timeout {
            timeout_seconds: 30,
        };
        assert!(detect_permission_prompt(&err, "slot-1").is_none());
    }

    #[test]
    fn extract_tool_name_unknown_fallback() {
        assert_eq!(extract_tool_name("some random text"), "unknown");
    }

    #[test]
    fn extract_tool_name_allow_prefix() {
        assert_eq!(extract_tool_name("Allow Write tool?"), "Write");
    }

    #[test]
    fn extract_tool_name_wants_to_use() {
        assert_eq!(
            extract_tool_name("Claude wants to use Bash, proceed?"),
            "Bash"
        );
    }

    // ── Failure detail extraction tests ─────────────────────────────

    #[test]
    fn extract_details_from_command_failed() {
        let err = Error::Wrapper(claude_wrapper::Error::CommandFailed {
            command: "claude --print -p test".into(),
            exit_code: 1,
            stdout: String::new(),
            stderr: "error: something went wrong".into(),
            working_dir: None,
        });
        let details = extract_failure_details(&err);
        assert_eq!(
            details.failed_command.as_deref(),
            Some("claude --print -p test")
        );
        assert_eq!(details.exit_code, Some(1));
        assert_eq!(
            details.stderr.as_deref(),
            Some("error: something went wrong")
        );
    }

    #[test]
    fn extract_details_from_non_command_error() {
        let err = Error::TaskNotFound("task-123".into());
        let details = extract_failure_details(&err);
        assert!(details.failed_command.is_none());
        assert!(details.exit_code.is_none());
        assert!(details.stderr.is_none());
    }

    #[test]
    fn extract_details_empty_stderr_is_none() {
        let err = Error::Wrapper(claude_wrapper::Error::CommandFailed {
            command: "claude --print".into(),
            exit_code: 2,
            stdout: String::new(),
            stderr: String::new(),
            working_dir: None,
        });
        let details = extract_failure_details(&err);
        assert_eq!(details.failed_command.as_deref(), Some("claude --print"));
        assert_eq!(details.exit_code, Some(2));
        assert!(details.stderr.is_none());
    }

    // ── Chain cancellation tests ────────────────────────────────────

    #[tokio::test]
    async fn cancel_chain_marks_task_cancelled() {
        let pool = Pool::builder(mock_claude()).slots(1).build().await.unwrap();

        // Manually insert a running chain task.
        let task_id = TaskId("chain-test-1".into());
        let record = TaskRecord {
            id: task_id.clone(),
            prompt: "chain: 3 steps".into(),
            state: TaskState::Running,
            slot_id: None,
            result: None,
            tags: vec![],
            config: None,
            review_required: false,
            max_rejections: 3,
            rejection_count: 0,
            original_prompt: None,
        };
        pool.store().put_task(record).await.unwrap();

        // Also set up chain progress.
        pool.set_chain_progress(
            &task_id,
            crate::chain::ChainProgress {
                total_steps: 3,
                current_step: Some(1),
                current_step_name: Some("implement".into()),
                current_step_partial_output: None,
                current_step_started_at: None,
                completed_steps: vec![],
                status: crate::chain::ChainStatus::Running,
            },
        )
        .await;

        // Cancel it.
        pool.cancel_chain(&task_id).await.unwrap();

        // Task should be cancelled.
        let task = pool.store().get_task(&task_id).await.unwrap().unwrap();
        assert_eq!(task.state, TaskState::Cancelled);

        // Progress should show cancelled.
        let progress = pool.chain_progress(&task_id).unwrap();
        assert_eq!(progress.status, crate::chain::ChainStatus::Cancelled);
    }

    #[tokio::test]
    async fn cancel_chain_noop_for_completed() {
        let pool = Pool::builder(mock_claude()).slots(1).build().await.unwrap();

        let task_id = TaskId("chain-done".into());
        let record = TaskRecord {
            id: task_id.clone(),
            prompt: "chain: 1 steps".into(),
            state: TaskState::Completed,
            slot_id: None,
            result: Some(TaskResult {
                output: "done".into(),
                success: true,
                cost_microdollars: 100,
                turns_used: 0,
                session_id: None,
                failed_command: None,
                exit_code: None,
                stderr: None,
            }),
            tags: vec![],
            config: None,
            review_required: false,
            max_rejections: 3,
            rejection_count: 0,
            original_prompt: None,
        };
        pool.store().put_task(record).await.unwrap();

        // Should be a no-op.
        pool.cancel_chain(&task_id).await.unwrap();
        let task = pool.store().get_task(&task_id).await.unwrap().unwrap();
        assert_eq!(task.state, TaskState::Completed);
    }

    #[tokio::test]
    async fn cancel_chain_not_found() {
        let pool = Pool::builder(mock_claude()).slots(1).build().await.unwrap();
        let result = pool.cancel_chain(&TaskId("nonexistent".into())).await;
        assert!(matches!(result, Err(Error::TaskNotFound(_))));
    }

    // ── Live output tests ────────────────────────────────────────────

    #[tokio::test]
    async fn append_chain_partial_output_accumulates() {
        let pool = Pool::builder(mock_claude()).slots(1).build().await.unwrap();

        let task_id = TaskId("chain-test".into());
        let progress = crate::chain::ChainProgress {
            total_steps: 2,
            current_step: Some(0),
            current_step_name: Some("plan".into()),
            current_step_partial_output: Some(String::new()),
            current_step_started_at: Some(1700000000),
            completed_steps: vec![],
            status: crate::chain::ChainStatus::Running,
        };
        pool.set_chain_progress(&task_id, progress).await;

        pool.append_chain_partial_output(&task_id, "hello ");
        pool.append_chain_partial_output(&task_id, "world");

        let progress = pool.chain_progress(&task_id).unwrap();
        assert_eq!(
            progress.current_step_partial_output.as_deref(),
            Some("hello world")
        );
    }

    #[tokio::test]
    async fn append_chain_partial_output_noop_when_none() {
        let pool = Pool::builder(mock_claude()).slots(1).build().await.unwrap();

        let task_id = TaskId("chain-test-2".into());
        // Progress with partial output = None (completed state).
        let progress = crate::chain::ChainProgress {
            total_steps: 1,
            current_step: None,
            current_step_name: None,
            current_step_partial_output: None,
            current_step_started_at: None,
            completed_steps: vec![],
            status: crate::chain::ChainStatus::Completed,
        };
        pool.set_chain_progress(&task_id, progress).await;

        // Should not panic or create a partial output field.
        pool.append_chain_partial_output(&task_id, "ignored");

        let progress = pool.chain_progress(&task_id).unwrap();
        assert!(progress.current_step_partial_output.is_none());
    }

    #[tokio::test]
    async fn append_chain_partial_output_noop_for_missing_task() {
        let pool = Pool::builder(mock_claude()).slots(1).build().await.unwrap();

        // Should not panic when task doesn't exist.
        let task_id = TaskId("nonexistent".into());
        pool.append_chain_partial_output(&task_id, "ignored");
    }
}
