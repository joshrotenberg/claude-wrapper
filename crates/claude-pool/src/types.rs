//! Core types for claude-pool.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

// Re-export shared types from claude-wrapper so consumers don't need
// to depend on both crates for basic config.
pub use claude_wrapper::types::{Effort, PermissionMode};

// ── Identifiers ──────────────────────────────────────────────────────

/// Unique identifier for a task.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct TaskId(pub String);

/// Unique identifier for a worker.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct WorkerId(pub String);

// ── Worker types ─────────────────────────────────────────────────────

/// Worker persistence mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum WorkerMode {
    /// Persistent workers stay alive across tasks, resuming sessions.
    #[default]
    Persistent,
    /// Ephemeral workers are created per task and destroyed after.
    Ephemeral,
}

/// Configuration that applies to all workers by default.
///
/// Individual workers can override any of these fields via [`WorkerConfig`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GlobalWorkerConfig {
    /// Claude model to use (e.g. "claude-haiku-4-5-20251001").
    pub model: Option<String>,

    /// Permission mode for workers.
    pub permission_mode: Option<PermissionMode>,

    /// Maximum turns per task.
    pub max_turns: Option<u32>,

    /// System prompt prepended to all worker tasks.
    pub system_prompt: Option<String>,

    /// Allowed tools for workers.
    pub allowed_tools: Vec<String>,

    /// MCP servers available to workers.
    pub mcp_servers: HashMap<String, serde_json::Value>,

    /// Default effort level for workers (maps to `--effort`).
    pub effort: Option<Effort>,

    /// Total budget cap for the pool in microdollars.
    /// When cumulative spend across all workers reaches this limit,
    /// new tasks are rejected with [`crate::Error::BudgetExhausted`].
    pub budget_microdollars: Option<u64>,

    /// Default worker mode.
    pub worker_mode: WorkerMode,

    /// Maximum number of restarts per worker before marking as errored.
    pub max_restarts: u32,

    /// Enable git worktree isolation for workers.
    pub worktree_isolation: bool,
}

impl Default for GlobalWorkerConfig {
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
            worker_mode: WorkerMode::default(),
            max_restarts: 3,
            worktree_isolation: false,
        }
    }
}

/// Per-worker configuration overrides.
///
/// Any `Some` field here takes precedence over the corresponding field
/// in [`GlobalWorkerConfig`].
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct WorkerConfig {
    /// Override model for this worker.
    pub model: Option<String>,

    /// Override permission mode for this worker.
    pub permission_mode: Option<PermissionMode>,

    /// Override max turns for this worker.
    pub max_turns: Option<u32>,

    /// Override system prompt for this worker.
    pub system_prompt: Option<String>,

    /// Additional allowed tools (merged with global).
    pub allowed_tools: Option<Vec<String>>,

    /// Additional MCP servers (merged with global).
    pub mcp_servers: Option<HashMap<String, serde_json::Value>>,

    /// Override effort level for this worker.
    pub effort: Option<Effort>,

    /// Optional name/role for this worker (e.g. "reviewer", "coder").
    pub role: Option<String>,
}

/// Current state of a worker.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkerState {
    /// Worker is ready to accept a task.
    Idle,
    /// Worker is currently executing a task.
    Busy,
    /// Worker process has exited or been stopped.
    Stopped,
    /// Worker encountered an error and needs attention.
    Errored,
}

/// Record of a worker in the pool.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkerRecord {
    /// Unique worker identifier.
    pub id: WorkerId,

    /// Current state.
    pub state: WorkerState,

    /// Per-worker config overrides.
    pub config: WorkerConfig,

    /// The task currently being executed, if any.
    pub current_task: Option<TaskId>,

    /// Claude session ID for session resumption.
    pub session_id: Option<String>,

    /// Number of tasks completed by this worker.
    pub tasks_completed: u64,

    /// Cumulative cost in microdollars.
    pub cost_microdollars: u64,

    /// Number of times this worker has been restarted.
    pub restart_count: u32,

    /// Git worktree path, if worktree isolation is enabled.
    pub worktree_path: Option<String>,
}

// ── Task types ───────────────────────────────────────────────────────

/// Current state of a task.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskState {
    /// Task is waiting for a worker.
    Pending,
    /// Task is being executed by a worker.
    Running,
    /// Task completed successfully.
    Completed,
    /// Task failed.
    Failed,
    /// Task was cancelled.
    Cancelled,
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

    /// Worker assigned to this task.
    pub worker_id: Option<WorkerId>,

    /// Task result, available when state is `Completed` or `Failed`.
    pub result: Option<TaskResult>,

    /// Optional tags for filtering and grouping.
    pub tags: Vec<String>,

    /// Per-task config overrides (takes precedence over worker and global config).
    pub config: Option<WorkerConfig>,
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

    /// Session ID from the execution.
    pub session_id: Option<String>,
}

/// Filter criteria for listing tasks.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TaskFilter {
    /// Filter by state.
    pub state: Option<TaskState>,

    /// Filter by worker.
    pub worker_id: Option<WorkerId>,

    /// Filter by tags (any match).
    pub tags: Option<Vec<String>>,
}
