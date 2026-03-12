//! Core types for claude-pool.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::time::{SystemTime, UNIX_EPOCH};

/// Current time in milliseconds since epoch.
pub fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

// Re-export shared types from claude-wrapper so consumers don't need
// to depend on both crates for basic config.
pub use claude_wrapper::types::{Effort, PermissionMode};

// ── Identifiers ──────────────────────────────────────────────────────

/// Unique identifier for a task.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct TaskId(pub String);

/// Unique identifier for a slot.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct SlotId(pub String);

// ── Slot types ─────────────────────────────────────────────────────

/// Slot persistence mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum SlotMode {
    /// Persistent slots stay alive across tasks, resuming sessions.
    #[default]
    Persistent,
    /// Ephemeral slots are created per task and destroyed after.
    Ephemeral,
}

/// Configuration for dynamic slot pool scaling.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScalingConfig {
    /// Minimum number of slots (default: 1).
    pub min_slots: usize,
    /// Maximum number of slots (default: 16).
    pub max_slots: usize,
}

impl Default for ScalingConfig {
    fn default() -> Self {
        Self {
            min_slots: 1,
            max_slots: 16,
        }
    }
}

/// Configuration that applies to all slots by default.
///
/// Individual slots can override any of these fields via [`SlotConfig`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PoolConfig {
    /// Claude model to use (e.g. "claude-haiku-4-5-20251001").
    pub model: Option<String>,

    /// Permission mode for slots.
    pub permission_mode: Option<PermissionMode>,

    /// Maximum turns per task.
    pub max_turns: Option<u32>,

    /// System prompt prepended to all slot tasks.
    pub system_prompt: Option<String>,

    /// Allowed tools for slots.
    pub allowed_tools: Vec<String>,

    /// MCP servers available to slots.
    pub mcp_servers: HashMap<String, serde_json::Value>,

    /// Default effort level for slots (maps to `--effort`).
    pub effort: Option<Effort>,

    /// Total budget cap for the pool in microdollars.
    /// When cumulative spend across all slots reaches this limit,
    /// new tasks are rejected with [`crate::Error::BudgetExhausted`].
    pub budget_microdollars: Option<u64>,

    /// Default slot mode.
    pub slot_mode: SlotMode,

    /// Maximum number of restarts per slot before marking as errored.
    pub max_restarts: u32,

    /// Enable git worktree isolation for slots.
    pub worktree_isolation: bool,

    /// Maximum time to wait for an idle slot before failing a task (in seconds).
    pub slot_assignment_timeout_secs: u64,

    /// Dynamic scaling configuration (min/max bounds).
    pub scaling: ScalingConfig,

    /// Enable unattended mode: use stricter permission defaults to prevent prompts.
    /// When true, defaults to `DontAsk` permission mode if not explicitly set.
    pub unattended_mode: bool,

    /// If true, detect permission prompt patterns in stderr and provide actionable errors.
    pub detect_permission_prompts: bool,

    /// Enable the background supervisor loop for slot health monitoring.
    ///
    /// When enabled, the supervisor periodically checks for errored slots and
    /// restarts them automatically (up to [`max_restarts`](Self::max_restarts)).
    pub supervisor_enabled: bool,

    /// Interval in seconds between supervisor health checks (default: 30).
    ///
    /// Only used when [`supervisor_enabled`](Self::supervisor_enabled) is true.
    pub supervisor_interval_secs: u64,

    /// Use `--strict-mcp-config` when passing MCP config to slots.
    ///
    /// Prevents slots from inheriting the coordinator's `.mcp.json`, which
    /// avoids accidental recursive pool calls (a slot invoking `pool_run` on itself).
    /// Default: `true`.
    pub strict_mcp_config: bool,
}

impl Default for PoolConfig {
    fn default() -> Self {
        Self {
            model: None,
            permission_mode: Some(PermissionMode::Plan),
            max_turns: None,
            system_prompt: None,
            allowed_tools: Vec::new(),
            mcp_servers: HashMap::new(),
            effort: None,
            budget_microdollars: None,
            slot_mode: SlotMode::default(),
            max_restarts: 3,
            worktree_isolation: false,
            slot_assignment_timeout_secs: 300,
            scaling: ScalingConfig::default(),
            unattended_mode: false,
            detect_permission_prompts: true,
            supervisor_enabled: false,
            supervisor_interval_secs: 30,
            strict_mcp_config: true,
        }
    }
}

/// Per-slot configuration overrides.
///
/// Any `Some` field here takes precedence over the corresponding field
/// in [`PoolConfig`].
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SlotConfig {
    /// Override model for this slot.
    pub model: Option<String>,

    /// Override permission mode for this slot.
    pub permission_mode: Option<PermissionMode>,

    /// Override max turns for this slot.
    pub max_turns: Option<u32>,

    /// Override system prompt for this slot.
    pub system_prompt: Option<String>,

    /// Additional allowed tools (merged with global).
    pub allowed_tools: Option<Vec<String>>,

    /// Additional MCP servers (merged with global).
    pub mcp_servers: Option<HashMap<String, serde_json::Value>>,

    /// Override effort level for this slot.
    pub effort: Option<Effort>,

    /// Optional name/role for this slot (e.g. "reviewer", "coder").
    pub role: Option<String>,

    /// Optional human-readable name for the slot (e.g. "reviewer", "writer").
    pub name: Option<String>,

    /// Optional description of the slot's purpose or responsibilities.
    pub description: Option<String>,

    /// Override slot assignment timeout (in seconds).
    pub slot_assignment_timeout_secs: Option<u64>,
}

/// Per-task configuration overrides.
///
/// Applied on top of the slot config for a single task execution. Unlike
/// [`SlotConfig`], this struct contains only execution parameters — it has
/// no identity fields (name, role, description) or slot lifecycle settings.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TaskOverrides {
    /// Override model for this task.
    pub model: Option<String>,

    /// Override permission mode for this task.
    pub permission_mode: Option<PermissionMode>,

    /// Override max turns for this task.
    pub max_turns: Option<u32>,

    /// Override system prompt for this task.
    pub system_prompt: Option<String>,

    /// Additional allowed tools for this task (merged with global and slot).
    pub allowed_tools: Option<Vec<String>>,

    /// Additional MCP servers for this task (merged with global and slot).
    pub mcp_servers: Option<HashMap<String, serde_json::Value>>,

    /// Override effort level for this task.
    pub effort: Option<Effort>,

    /// JSON schema for structured output validation.
    pub json_schema: Option<serde_json::Value>,
}

/// Current state of a slot.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SlotState {
    /// Slot is ready to accept a task.
    Idle,
    /// Slot is currently executing a task.
    Busy,
    /// Slot process has exited or been stopped.
    Stopped,
    /// Slot encountered an error and needs attention.
    Errored,
}

/// Record of a slot in the pool.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SlotRecord {
    /// Unique slot identifier.
    pub id: SlotId,

    /// Current state.
    pub state: SlotState,

    /// Per-slot config overrides.
    pub config: SlotConfig,

    /// The task currently being executed, if any.
    pub current_task: Option<TaskId>,

    /// Claude session ID for session resumption.
    pub session_id: Option<String>,

    /// Number of tasks completed by this slot.
    pub tasks_completed: u64,

    /// Cumulative cost in microdollars.
    pub cost_microdollars: u64,

    /// Number of times this slot has been restarted.
    pub restart_count: u32,

    /// Git worktree path, if worktree isolation is enabled.
    pub worktree_path: Option<String>,

    /// Path to the slot's temp `.mcp.json` file, if MCP servers are configured.
    ///
    /// Written once per slot (on first task that needs it) and reused across
    /// subsequent tasks. Cleaned up on pool drain/shutdown.
    #[serde(skip)]
    pub mcp_config_path: Option<std::path::PathBuf>,
}

// ── Task types ───────────────────────────────────────────────────────

/// Current state of a task.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskState {
    /// Task is waiting for a slot.
    Pending,
    /// Task is being executed by a slot.
    Running,
    /// Task completed successfully.
    Completed,
    /// Task failed.
    Failed,
    /// Task was cancelled.
    Cancelled,
    /// Task completed but awaits coordinator approval before being considered done.
    PendingReview,
}

/// A task submitted to the pool.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskRecord {
    /// Unique task identifier.
    pub id: TaskId,

    /// The prompt/instruction for the task.
    pub prompt: String,

    /// Current state.
    pub state: TaskState,

    /// Slot assigned to this task.
    pub slot_id: Option<SlotId>,

    /// Task result, available when state is `Completed` or `Failed`.
    pub result: Option<TaskResult>,

    /// Optional tags for filtering and grouping.
    pub tags: Vec<String>,

    /// Per-task config overrides (takes precedence over slot and global config).
    pub config: Option<TaskOverrides>,

    /// When true, completed tasks transition to `PendingReview` instead of `Completed`.
    #[serde(default)]
    pub review_required: bool,

    /// Maximum number of rejections before the task is marked as failed (default: 3).
    #[serde(default = "default_max_rejections")]
    pub max_rejections: u32,

    /// Number of times this task has been rejected and re-queued.
    #[serde(default)]
    pub rejection_count: u32,

    /// The original prompt before any rejection feedback was appended.
    #[serde(default)]
    pub original_prompt: Option<String>,

    /// When this task was submitted (millis since epoch).
    #[serde(default)]
    pub created_at_ms: Option<u64>,

    /// When this task started executing (millis since epoch).
    #[serde(default)]
    pub started_at_ms: Option<u64>,

    /// When this task completed/failed/was cancelled (millis since epoch).
    #[serde(default)]
    pub completed_at_ms: Option<u64>,
}

fn default_max_rejections() -> u32 {
    3
}

/// The result of a completed task.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskResult {
    /// The text output from Claude.
    pub output: String,

    /// Whether the task succeeded.
    pub success: bool,

    /// Cost in microdollars.
    pub cost_microdollars: u64,

    /// Number of turns used.
    pub turns_used: u32,

    /// Wall-clock execution time in milliseconds.
    #[serde(default)]
    pub elapsed_ms: u64,

    /// Model that executed this task.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,

    /// Session ID from the execution.
    pub session_id: Option<String>,

    /// On failure: the CLI command that was run.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub failed_command: Option<String>,

    /// On failure: the exit code from the CLI.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub exit_code: Option<i32>,

    /// On failure: stderr output from the CLI.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stderr: Option<String>,
}

impl TaskResult {
    /// Create a successful task result.
    pub fn success(output: impl Into<String>, cost_microdollars: u64, turns_used: u32) -> Self {
        Self {
            output: output.into(),
            success: true,
            cost_microdollars,
            turns_used,
            elapsed_ms: 0,
            model: None,
            session_id: None,
            failed_command: None,
            exit_code: None,
            stderr: None,
        }
    }

    /// Create a failed task result.
    pub fn failure(output: impl Into<String>) -> Self {
        Self {
            output: output.into(),
            success: false,
            cost_microdollars: 0,
            turns_used: 0,
            elapsed_ms: 0,
            model: None,
            session_id: None,
            failed_command: None,
            exit_code: None,
            stderr: None,
        }
    }

    /// Set the model that executed this task.
    pub fn with_model(mut self, model: impl Into<String>) -> Self {
        self.model = Some(model.into());
        self
    }

    /// Set the elapsed execution time in milliseconds.
    pub fn with_elapsed_ms(mut self, elapsed_ms: u64) -> Self {
        self.elapsed_ms = elapsed_ms;
        self
    }

    /// Set the session ID.
    pub fn with_session_id(mut self, session_id: impl Into<String>) -> Self {
        self.session_id = Some(session_id.into());
        self
    }

    /// Set failure details (command, exit code, stderr).
    pub fn with_failure_details(
        mut self,
        command: Option<String>,
        exit_code: Option<i32>,
        stderr: Option<String>,
    ) -> Self {
        self.failed_command = command;
        self.exit_code = exit_code;
        self.stderr = stderr;
        self
    }
}

impl TaskRecord {
    /// Create a new pending task record with timestamps.
    pub fn new_pending(id: TaskId, prompt: impl Into<String>) -> Self {
        Self {
            id,
            prompt: prompt.into(),
            state: TaskState::Pending,
            slot_id: None,
            result: None,
            tags: vec![],
            config: None,
            review_required: false,
            max_rejections: 3,
            rejection_count: 0,
            original_prompt: None,
            created_at_ms: Some(now_ms()),
            started_at_ms: None,
            completed_at_ms: None,
        }
    }

    /// Set tags.
    pub fn with_tags(mut self, tags: Vec<String>) -> Self {
        self.tags = tags;
        self
    }

    /// Set per-task config overrides.
    pub fn with_config(mut self, config: Option<TaskOverrides>) -> Self {
        self.config = config;
        self
    }

    /// Enable review-required mode.
    pub fn with_review(mut self, max_rejections: u32) -> Self {
        self.review_required = true;
        self.max_rejections = max_rejections;
        self.original_prompt = Some(self.prompt.clone());
        self
    }

    /// Transition the task to a new state, setting timestamps automatically.
    ///
    /// - `Running`: sets `started_at_ms`
    /// - `Completed`, `Failed`, `Cancelled`, `PendingReview`: sets `completed_at_ms`
    pub fn transition_to(&mut self, state: TaskState) {
        self.state = state;
        let now = now_ms();
        match state {
            TaskState::Running => {
                self.started_at_ms = Some(now);
            }
            TaskState::Completed
            | TaskState::Failed
            | TaskState::Cancelled
            | TaskState::PendingReview => {
                self.completed_at_ms = Some(now);
            }
            TaskState::Pending => {}
        }
    }
}

/// Filter criteria for listing tasks.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TaskFilter {
    /// Filter by state.
    pub state: Option<TaskState>,

    /// Filter by slot.
    pub slot_id: Option<SlotId>,

    /// Filter by tags (any match).
    pub tags: Option<Vec<String>>,
}

/// Aggregated metrics for the current pool session.
///
/// Provides developer-focused insights: spend tracking, task timing,
/// and sizing data useful for optimizing pool usage patterns.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SessionMetrics {
    /// Total number of tasks submitted this session.
    pub total_tasks: u64,
    /// Number of completed tasks.
    pub completed_tasks: u64,
    /// Number of failed tasks.
    pub failed_tasks: u64,
    /// Number of cancelled tasks.
    pub cancelled_tasks: u64,
    /// Number of currently running tasks.
    pub running_tasks: u64,
    /// Number of pending tasks.
    pub pending_tasks: u64,

    /// Total spend across all tasks in microdollars.
    pub total_spend_microdollars: u64,
    /// Average cost per completed task in microdollars.
    pub avg_cost_microdollars: u64,
    /// Highest single-task cost in microdollars.
    pub max_cost_microdollars: u64,

    /// Average execution time for completed tasks in milliseconds.
    pub avg_elapsed_ms: u64,
    /// Median execution time for completed tasks in milliseconds.
    pub median_elapsed_ms: u64,
    /// Maximum execution time for completed tasks in milliseconds.
    pub max_elapsed_ms: u64,
    /// Minimum execution time for completed tasks in milliseconds.
    pub min_elapsed_ms: u64,

    /// Average number of turns per completed task.
    pub avg_turns: f64,

    /// Breakdown of tasks by model (count only).
    pub tasks_by_model: HashMap<String, u64>,

    /// Detailed per-model metrics.
    pub model_breakdown: Vec<ModelMetrics>,

    /// Session start time (millis since epoch).
    pub session_start_ms: u64,
    /// Session duration so far in milliseconds.
    pub session_duration_ms: u64,
}

/// Per-model aggregated metrics.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ModelMetrics {
    /// Model identifier.
    pub model: String,
    /// Number of tasks run on this model.
    pub task_count: u64,
    /// Total spend for this model in microdollars.
    pub total_cost_microdollars: u64,
    /// Average cost per task in microdollars.
    pub avg_cost_microdollars: u64,
    /// Average execution time in milliseconds.
    pub avg_elapsed_ms: u64,
    /// Total turns used by this model.
    pub total_turns: u64,
}

/// Filter criteria for session metrics queries.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct MetricsFilter {
    /// Only include tasks created after this time (millis since epoch).
    pub since_ms: Option<u64>,
    /// Only include tasks created before this time (millis since epoch).
    pub until_ms: Option<u64>,
    /// Only include tasks with these tags (any match).
    pub tags: Option<Vec<String>>,
    /// Only include tasks that ran on this model.
    pub model: Option<String>,
}
