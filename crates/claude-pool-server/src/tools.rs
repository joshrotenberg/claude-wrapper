//! # MCP Tool Definitions for claude-pool
//!
//! This module provides all 34 MCP tools exposed by the claude-pool server, organized into 10 categories
//! for task execution, slot management, chain coordination, skill registration, messaging, and context sharing.
//!
//! ## Tool Categories
//!
//! ### Task Management (6 tools)
//! - `pool_run` — Run a task synchronously, block until completion
//! - `pool_submit` — Fire a task asynchronously, returns task_id immediately
//! - `pool_result` — Check on a fired task, returns result if complete or pending_review
//! - `pool_cancel` — Cancel a pending or running task
//! - `pool_fan_out` — Fan out N independent tasks in parallel, returns all results
//! - `pool_submit_with_review` — Fire a task requiring coordinator approval before completion
//!
//! ### Review Management (2 tools)
//! - `pool_approve_result` — Approve a pending_review task, mark as completed
//! - `pool_reject_result` — Reject with feedback, re-queue with appended feedback
//!
//! ### Chain Management (5 tools)
//! - `pool_chain` — Chain sequential steps, block until all complete
//! - `pool_submit_chain` — Fire a chain for async execution, returns task_id immediately
//! - `pool_fan_out_chains` — Fan out N independent chains in parallel on separate slots
//! - `pool_chain_result` — Check on fired chain, shows per-step progress
//! - `pool_cancel_chain` — Cancel a running chain, finishes current step before stopping
//!
//! ### Slot Management (6 tools)
//! - `pool_status` — Get pool status: slots, tasks, budget, server metadata
//! - `pool_configure_slot` — Set name/role/description for persistent slot identity
//! - `pool_find_slots` — Query slots by name, role, and/or state (all filters optional)
//! - `pool_claim` — Self-service task claiming: idle slot grabs next pending task
//! - `pool_scale_up` — Add N new slots to the pool
//! - `pool_scale_down` — Remove N slots from pool
//!
//! ### Pool Control (3 tools)
//! - `pool_drain` — Gracefully shut down pool, wait for in-flight tasks
//! - `pool_set_target_slots` — Set pool to specific number of slots
//! - `pool_session_metrics` — Get aggregated session metrics: spend, timing, model breakdown
//!
//! ### Skill Management (7 tools)
//! - `pool_skill_run` — Run a registered skill by name with arguments (blocks)
//! - `pool_skill_list` — List skills with optional scope/source filters
//! - `pool_skill_get` — Get full details of skill by name including prompt template
//! - `pool_skill_add` — Register skill at runtime (ephemeral unless saved)
//! - `pool_skill_remove` — Remove skill by name (runtime-only)
//! - `pool_skill_save` — Persist skill to project skills dir as SKILL.md
//! - `pool_skill_eject` — Eject builtin skill to disk for customization
//!
//! ### Messaging (4 tools)
//! - `pool_send_message` — Send message from one slot to another
//! - `pool_read_messages` — Drain and read all messages for slot
//! - `pool_peek_messages` — Read messages without removing from inbox
//! - `pool_broadcast` — Send message from one slot to all others
//!
//! ### Context Management (4 tools)
//! - `context_set` — Set shared context value (injected into slot system prompts)
//! - `context_get` — Get shared context value by key
//! - `context_delete` — Delete shared context value by key
//! - `context_list` — List all shared context keys and values
//!
//! ### Workflow Management (1 tool)
//! - `pool_invoke_workflow` — Submit named workflow template (issue_to_pr, refactor_and_test, review_and_fix)
//!
//! ## Tool Response Format
//!
//! All tools return a `CallToolResult` containing either:
//! - **Success**: JSON object/array (via `CallToolResult::json()`) or plain text (via `CallToolResult::text()`)
//! - **Error**: Error message (via `CallToolResult::error()`)
//!
//! ## Common Error Cases
//!
//! Tools may fail with:
//! - **TaskNotFound** — Task ID doesn't exist or was already cleaned up
//! - **NoSlotsAvailable** — Pool has no idle slots (for synchronous operations)
//! - **ChainFailure** — Chain step failed, subsequent steps skipped
//! - **InvalidInput** — Input validation failed (missing required field, invalid enum variant)
//! - **StorageError** — Underlying pool store error
//! - **PermissionDenied** — Slot authorization check failed (future)

use std::path::PathBuf;
use std::sync::Arc;

use claude_pool::PoolStore;
use claude_pool::skill::{SkillScope, SkillSource};
use claude_pool::types::TaskOverrides;
use schemars::JsonSchema;
use serde::Deserialize;
use tower_mcp::ToolBuilder;
use tower_mcp::protocol::CallToolResult;
use tower_mcp::tool::Tool;

use crate::State;

// ── Input schemas ────────────────────────────────────────────────────

#[derive(Debug, Deserialize, JsonSchema)]
pub struct RunInput {
    /// The prompt/task to execute.
    pub prompt: String,
    /// Model override for this task.
    pub model: Option<String>,
    /// Effort override for this task (min, low, medium, high, max).
    pub effort: Option<String>,
    /// Tools to explicitly disallow for this task.
    pub disallowed_tools: Option<Vec<String>>,
    /// Built-in tool selection for this task (e.g. "Bash", "Edit", "Read").
    pub tools: Option<Vec<String>>,
    /// Additional MCP servers for this task (merged with global/slot servers).
    /// Keys are server names, values are server config objects.
    pub mcp_servers: Option<std::collections::HashMap<String, serde_json::Value>>,
    /// JSON schema for structured output validation.
    pub json_schema: Option<serde_json::Value>,
    /// Maximum budget cap for this task in USD.
    pub max_budget_usd: Option<f64>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct SubmitInput {
    /// The prompt/task to execute.
    pub prompt: String,
    /// Model override for this task.
    pub model: Option<String>,
    /// Effort override for this task (min, low, medium, high, max).
    pub effort: Option<String>,
    /// Tags for grouping/filtering.
    pub tags: Option<Vec<String>>,
    /// Tools to explicitly disallow for this task.
    pub disallowed_tools: Option<Vec<String>>,
    /// Built-in tool selection for this task (e.g. "Bash", "Edit", "Read").
    pub tools: Option<Vec<String>>,
    /// Additional MCP servers for this task (merged with global/slot servers).
    /// Keys are server names, values are server config objects.
    pub mcp_servers: Option<std::collections::HashMap<String, serde_json::Value>>,
    /// JSON schema for structured output validation.
    pub json_schema: Option<serde_json::Value>,
    /// Maximum budget cap for this task in USD.
    pub max_budget_usd: Option<f64>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct TaskIdInput {
    /// The task ID to look up.
    pub task_id: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct FanOutInput {
    /// List of prompts to execute in parallel.
    pub prompts: Vec<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ContextSetInput {
    /// Context key.
    pub key: String,
    /// Context value.
    pub value: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ContextKeyInput {
    /// Context key.
    pub key: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ConfigureSlotInput {
    /// Slot ID to configure (e.g. "slot-0").
    pub slot_id: String,
    /// Human-readable name for the slot.
    pub name: Option<String>,
    /// Role classification for the slot.
    pub role: Option<String>,
    /// Description of the slot's purpose.
    pub description: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct InvokeWorkflowInput {
    /// Workflow name (e.g. "issue_to_pr", "refactor_and_test", "review_and_fix").
    pub workflow: String,
    /// Workflow arguments as key-value pairs (e.g. {"issue_url": "https://..."}).
    #[serde(default)]
    pub arguments: std::collections::HashMap<String, String>,
    /// Tags for the workflow task.
    pub tags: Option<Vec<String>>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ScalingInput {
    /// Number of slots to add or remove.
    pub count: usize,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct SetTargetSlotsInput {
    /// Target number of slots.
    pub target: usize,
}

// ── Skill management input schemas ──────────────────────────────────

/// Input for listing skills with optional filters.
#[derive(Debug, Deserialize, JsonSchema)]
pub struct SkillListInput {
    /// Filter by scope: "task", "coordinator", "chain".
    pub scope: Option<String>,
    /// Filter by source: "builtin", "project", "runtime".
    pub source: Option<String>,
}

/// Input for getting a skill by name.
#[derive(Debug, Deserialize, JsonSchema)]
pub struct SkillGetInput {
    /// Skill name.
    pub name: String,
}

/// Argument definition for a skill being added.
#[derive(Debug, Deserialize, JsonSchema)]
pub struct SkillArgumentInput {
    /// Argument name (used as `{name}` placeholder in the prompt template).
    pub name: String,
    /// Human-readable description.
    pub description: String,
    /// Whether this argument is required.
    #[serde(default)]
    pub required: bool,
}

/// Input for adding a new skill at runtime.
#[derive(Debug, Deserialize, JsonSchema)]
pub struct SkillAddInput {
    /// Unique skill name.
    pub name: String,
    /// Human-readable description.
    pub description: String,
    /// Prompt template. Use `{arg_name}` placeholders for arguments.
    pub prompt: String,
    /// Argument definitions.
    #[serde(default)]
    pub arguments: Vec<SkillArgumentInput>,
    /// Where this skill runs: "task" (default), "coordinator", "chain".
    pub scope: Option<String>,
    /// Per-skill config overrides as JSON (model, effort, etc.).
    pub config: Option<serde_json::Value>,
}

/// Input for removing a skill by name.
#[derive(Debug, Deserialize, JsonSchema)]
pub struct SkillRemoveInput {
    /// Skill name to remove.
    pub name: String,
}

/// Input for saving a skill to disk.
#[derive(Debug, Deserialize, JsonSchema)]
pub struct SkillSaveInput {
    /// Skill name to save.
    pub name: String,
    /// Directory to save to. Defaults to the configured skills_dir.
    pub dir: Option<String>,
}

/// Input for ejecting a builtin skill to disk for customization.
#[derive(Debug, Deserialize, JsonSchema)]
pub struct SkillEjectInput {
    /// Skill name to eject (must be a builtin skill).
    pub name: String,
    /// Directory to eject to. Defaults to the configured skills_dir.
    pub dir: Option<String>,
}

// ── Messaging input schemas ────────────────────────────────────────────

/// Input for sending a message between slots.
#[derive(Debug, Deserialize, JsonSchema)]
pub struct SendMessageInput {
    /// Sender slot ID (e.g., "slot-0").
    pub from: String,
    /// Recipient slot ID (e.g., "slot-1").
    pub to: String,
    /// Message content.
    pub content: String,
}

/// Input for reading messages from a slot's inbox.
#[derive(Debug, Deserialize, JsonSchema)]
pub struct ReadMessagesInput {
    /// Slot ID to read messages from (e.g., "slot-0").
    pub slot_id: String,
}

/// Input for peeking at messages in a slot's inbox.
#[derive(Debug, Deserialize, JsonSchema)]
pub struct PeekMessagesInput {
    /// Slot ID to peek at messages from (e.g., "slot-0").
    pub slot_id: String,
}

/// Input for broadcasting a message to all slots.
#[derive(Debug, Deserialize, JsonSchema)]
pub struct BroadcastInput {
    /// Sender slot ID (e.g., "slot-0").
    pub from: String,
    /// Message content to broadcast.
    pub content: String,
}

/// Input for finding slots by name, role, or state.
#[derive(Debug, Deserialize, JsonSchema)]
pub struct FindSlotsInput {
    /// Filter by slot name (exact match).
    #[serde(default)]
    pub name: Option<String>,
    /// Filter by slot role (exact match).
    #[serde(default)]
    pub role: Option<String>,
    /// Filter by slot state (idle, busy, stopped, errored).
    #[serde(default)]
    pub state: Option<String>,
}

/// Input for claiming the next pending task.
#[derive(Debug, Deserialize, JsonSchema)]
pub struct ClaimInput {
    /// Slot ID that wants to claim a task (e.g., "slot-0").
    pub slot_id: String,
}

/// Input for submitting a task that requires review before completion.
#[derive(Debug, Deserialize, JsonSchema)]
pub struct SubmitWithReviewInput {
    /// The prompt/task to execute.
    pub prompt: String,
    /// Model override for this task.
    pub model: Option<String>,
    /// Effort override for this task (min, low, medium, high, max).
    pub effort: Option<String>,
    /// Tags for grouping/filtering.
    pub tags: Option<Vec<String>>,
    /// Maximum number of rejections before failing (default: 3).
    pub max_rejections: Option<u32>,
    /// Additional MCP servers for this task (merged with global/slot servers).
    /// Keys are server names, values are server config objects.
    pub mcp_servers: Option<std::collections::HashMap<String, serde_json::Value>>,
    /// JSON schema for structured output validation.
    pub json_schema: Option<serde_json::Value>,
    /// Tools to explicitly deny for this task.
    pub disallowed_tools: Option<Vec<String>>,
    /// Built-in tool selection for this task (Bash, Edit, Read, etc.).
    pub tools: Option<Vec<String>>,
    /// Maximum budget cap for this task in USD.
    pub max_budget_usd: Option<f64>,
}

/// Input for approving a task result.
#[derive(Debug, Deserialize, JsonSchema)]
pub struct ApproveResultInput {
    /// The task ID to approve.
    pub task_id: String,
}

/// Input for rejecting a task result with feedback.
#[derive(Debug, Deserialize, JsonSchema)]
pub struct RejectResultInput {
    /// The task ID to reject.
    pub task_id: String,
    /// Feedback explaining why the result was rejected. This is appended to the
    /// original prompt when the task is re-queued.
    pub feedback: String,
}

// ── Helpers ──────────────────────────────────────────────────────────

fn parse_effort(s: &str) -> Option<claude_pool::Effort> {
    match s.to_lowercase().as_str() {
        "min" | "low" => Some(claude_pool::Effort::Low),
        "medium" => Some(claude_pool::Effort::Medium),
        "high" => Some(claude_pool::Effort::High),
        "max" => Some(claude_pool::Effort::Max),
        _ => None,
    }
}

fn task_config_from(
    model: Option<String>,
    effort: Option<String>,
    mcp_servers: Option<std::collections::HashMap<String, serde_json::Value>>,
    json_schema: Option<serde_json::Value>,
    disallowed_tools: Option<Vec<String>>,
    tools: Option<Vec<String>>,
    max_budget_usd: Option<f64>,
) -> Option<TaskOverrides> {
    if model.is_none()
        && effort.is_none()
        && mcp_servers.is_none()
        && json_schema.is_none()
        && disallowed_tools.is_none()
        && tools.is_none()
        && max_budget_usd.is_none()
    {
        return None;
    }
    Some(TaskOverrides {
        model,
        effort: effort.and_then(|e| parse_effort(&e)),
        mcp_servers,
        json_schema,
        disallowed_tools,
        tools,
        max_budget_usd,
        ..Default::default()
    })
}

fn parse_scope(s: &str) -> SkillScope {
    match s {
        "coordinator" => SkillScope::Coordinator,
        "chain" => SkillScope::Chain,
        _ => SkillScope::Task,
    }
}

fn parse_isolation(s: Option<&str>) -> claude_pool::chain::ChainIsolation {
    match s {
        Some("none") => claude_pool::chain::ChainIsolation::None,
        Some("clone") => claude_pool::chain::ChainIsolation::Clone,
        _ => claude_pool::chain::ChainIsolation::Worktree,
    }
}

fn parse_source(s: &str) -> Option<SkillSource> {
    match s {
        "builtin" => Some(SkillSource::Builtin),
        "project" => Some(SkillSource::Project),
        "runtime" => Some(SkillSource::Runtime),
        _ => None,
    }
}

// ── Tool builders ────────────────────────────────────────────────────

/// **pool_status** — Get pool status: slots, tasks in flight, budget, server metadata
///
/// # Description
/// Query the overall health and state of the pool without modifying anything.
///
/// # Parameters
/// None. This tool takes no parameters.
///
/// # Response Format
/// ```json
/// {
///   "total_slots": 8,
///   "idle_slots": 3,
///   "busy_slots": 5,
///   "in_flight_tasks": 5,
///   "in_flight_chains": 2,
///   "total_cost_tokens": 127430,
///   "budget_exhausted": false,
///   "server_name": "claude-pool-server",
///   "server_version": "0.3.0"
/// }
/// ```
///
/// # Example
/// Coordinator checking pool health before submitting batch work:
/// ```text
/// await client.call_tool("pool_status", {})
/// // Returns: { idle_slots: 3, in_flight_tasks: 5, ... }
/// ```
///
/// # Error Cases
/// - **StorageError** — Underlying pool store unavailable
/// - **Internal** — Concurrent modification during status collection (rare)
pub fn pool_status_tool<S: PoolStore + 'static>(state: Arc<State<S>>) -> Tool {
    ToolBuilder::new("pool_status")
        .title("Pool Status")
        .description("Get pool status: slots, tasks in flight, budget, server metadata")
        .read_only()
        .no_params_handler(move || {
            let state = Arc::clone(&state);
            async move {
                match state.pool.status().await {
                    Ok(status) => {
                        let mut response = serde_json::to_value(&status).unwrap();
                        let response_obj = response.as_object_mut().unwrap();
                        let server_obj = serde_json::to_value(&state.server_info).unwrap();
                        if let Some(server_map) = server_obj.as_object() {
                            for (key, value) in server_map.iter() {
                                response_obj.insert(format!("server_{key}"), value.clone());
                            }
                        }
                        Ok(CallToolResult::json(response))
                    }
                    Err(e) => Ok(CallToolResult::error(e.to_string())),
                }
            }
        })
        .build()
}

/// **pool_run** — Run a task synchronously, block until completion
///
/// # Description
/// Execute a prompt on the next available slot and wait for completion. Returns the full result.
/// Use this for single, clear actions with one clear output where you need the result before proceeding.
///
/// # Parameters
/// - **prompt** (string, required) — The prompt/task to execute
/// - **model** (string, optional) — Model override (e.g., "claude-opus-4-6")
/// - **effort** (string, optional) — Effort override: "min", "low", "medium", "high", "max"
/// - **mcp_servers** (object, optional) — Additional MCP servers: `{"server_name": {...config...}}`
///
/// # Response Format
/// ```json
/// {
///   "output": "the task output",
///   "tokens_used": 1427,
///   "model_used": "claude-sonnet-4-6",
///   "effort_applied": "medium"
/// }
/// ```
///
/// # Example
/// Coordinator running a single analysis task:
/// ```text
/// await client.call_tool("pool_run", {
///   "prompt": "Analyze this PR for security issues",
///   "model": "claude-opus-4-6"
/// })
/// ```
///
/// # Error Cases
/// - **NoSlotsAvailable** — Pool has no idle slots
/// - **TaskFailed** — Task execution failed or timed out
/// - **InvalidInput** — Missing required prompt field
pub fn pool_run_tool<S: PoolStore + 'static>(state: Arc<State<S>>) -> Tool {
    ToolBuilder::new("pool_run")
        .title("Run a Task")
        .description(
            "Run a task on the next available slot. Blocks until completion. \
             Use this for single, clear actions with one clear output.",
        )
        .handler(move |input: RunInput| {
            let state = Arc::clone(&state);
            async move {
                let config = task_config_from(
                    input.model,
                    input.effort,
                    input.mcp_servers,
                    input.json_schema,
                    input.disallowed_tools,
                    input.tools,
                    input.max_budget_usd,
                );
                let mut builder = state.pool.run(&input.prompt);
                if let Some(cfg) = config {
                    builder = builder.config(cfg);
                }
                match builder.await {
                    Ok(result) => Ok(CallToolResult::json(serde_json::to_value(&result).unwrap())),
                    Err(e) => Ok(CallToolResult::error(e.to_string())),
                }
            }
        })
        .build()
}

/// **pool_submit** — Fire a task asynchronously, returns task_id immediately
///
/// # Description
/// Submit a task for execution without waiting. The task is queued and executed when a slot becomes available.
/// Use this to fire off independent work and check progress later with `pool_result`.
///
/// # Parameters
/// - **prompt** (string, required) — The prompt/task to execute
/// - **model** (string, optional) — Model override
/// - **effort** (string, optional) — Effort override: "min", "low", "medium", "high", "max"
/// - **tags** (array, optional) — Tags for grouping/filtering (e.g., ["code-review", "urgent"])
/// - **mcp_servers** (object, optional) — Additional MCP servers
///
/// # Response Format
/// ```json
/// {
///   "task_id": "task-1234567890abcdef"
/// }
/// ```
///
/// # Example
/// Coordinator firing off a batch of independent reviews:
/// ```text
/// await client.call_tool("pool_submit", {
///   "prompt": "Review this feature PR",
///   "tags": ["review", "feature"]
/// })
/// // Returns: { task_id: "task-abc123" }
/// ```
///
/// # Error Cases
/// - **InvalidInput** — Missing required prompt field
/// - **StorageError** — Task could not be persisted
pub fn pool_submit_tool<S: PoolStore + 'static>(state: Arc<State<S>>) -> Tool {
    ToolBuilder::new("pool_submit")
        .title("Fire a Task")
        .description("Fire off a task for async execution. Returns a task_id immediately. Check on it later with pool_result.")
        .handler(move |input: SubmitInput| {
            let state = Arc::clone(&state);
            async move {
                let config = task_config_from(
                    input.model,
                    input.effort,
                    input.mcp_servers,
                    input.json_schema,
                    input.disallowed_tools,
                    input.tools,
                    input.max_budget_usd,
                );
                let tags = input.tags.unwrap_or_default();
                match state
                    .pool
                    .submit_with_config(&input.prompt, config, tags)
                    .await
                {
                    Ok(task_id) => Ok(CallToolResult::json(
                        serde_json::json!({ "task_id": task_id.0 }),
                    )),
                    Err(e) => Ok(CallToolResult::error(e.to_string())),
                }
            }
        })
        .build()
}

/// **pool_result** — Check on a fired task, returns result if complete or pending_review
///
/// # Description
/// Poll for a task's result. Returns result object if complete/pending_review, or `{"status": "running"}` if still executing.
/// For tasks with `review_required=true`, use `pool_approve_result` or `pool_reject_result` to finalize.
///
/// # Parameters
/// - **task_id** (string, required) — Task ID from `pool_submit` or `pool_submit_chain`
///
/// # Response Format
/// Success (task complete):
/// ```json
/// {
///   "output": "the task result",
///   "state": "completed",
///   "tokens_used": 1427
/// }
/// ```
/// Pending review:
/// ```json
/// {
///   "output": "...",
///   "state": "pending_review",
///   "review_required": true,
///   "rejection_count": 0,
///   "max_rejections": 3
/// }
/// ```
/// Still running:
/// ```json
/// {
///   "status": "running"
/// }
/// ```
///
/// # Example
/// Coordinator polling for task completion:
/// ```text
/// await client.call_tool("pool_result", { "task_id": "task-abc123" })
/// // Returns: { output: "...", state: "completed" }
/// ```
///
/// # Error Cases
/// - **TaskNotFound** — Task ID doesn't exist or was cleaned up
/// - **InvalidInput** — Missing task_id parameter
pub fn pool_result_tool<S: PoolStore + 'static>(state: Arc<State<S>>) -> Tool {
    ToolBuilder::new("pool_result")
        .title("Check on a Task")
        .description(
            "Check on a fired task. Returns the result if complete or pending_review, \
             null if still running. Tasks with review_required=true will have \
             state='pending_review' when done -- use pool_approve_result or \
             pool_reject_result to finalize.",
        )
        .read_only()
        .handler(move |input: TaskIdInput| {
            let state = Arc::clone(&state);
            async move {
                let task_id = claude_pool::TaskId(input.task_id.clone());
                // Fetch full task record for state info.
                let task = state.pool.store().get_task(&task_id).await.ok().flatten();

                match state.pool.result(&task_id).await {
                    Ok(Some(r)) => {
                        let mut val = serde_json::to_value(&r).unwrap();
                        if let Some(ref t) = task
                            && let Some(obj) = val.as_object_mut()
                        {
                            obj.insert("state".to_string(), serde_json::to_value(t.state).unwrap());
                            if t.review_required {
                                obj.insert(
                                    "review_required".to_string(),
                                    serde_json::Value::Bool(true),
                                );
                                obj.insert(
                                    "rejection_count".to_string(),
                                    serde_json::json!(t.rejection_count),
                                );
                                obj.insert(
                                    "max_rejections".to_string(),
                                    serde_json::json!(t.max_rejections),
                                );
                            }
                        }
                        Ok(CallToolResult::json(val))
                    }
                    Ok(None) => Ok(CallToolResult::json(
                        serde_json::json!({ "status": "running" }),
                    )),
                    Err(e) => Ok(CallToolResult::error(e.to_string())),
                }
            }
        })
        .build()
}

/// **pool_cancel** — Cancel a pending or running task
///
/// # Description
/// Stop a task before it completes. If the task is running, it continues to completion but the result is discarded.
/// If the task is pending, it's removed from the queue.
///
/// # Parameters
/// - **task_id** (string, required) — Task ID to cancel
///
/// # Response Format
/// ```json
/// "cancelled"
/// ```
///
/// # Example
/// Coordinator cancelling a long-running task:
/// ```text
/// await client.call_tool("pool_cancel", { "task_id": "task-abc123" })
/// // Returns: "cancelled"
/// ```
///
/// # Error Cases
/// - **TaskNotFound** — Task ID doesn't exist
/// - **AlreadyCompleted** — Task already finished
pub fn pool_cancel_tool<S: PoolStore + 'static>(state: Arc<State<S>>) -> Tool {
    ToolBuilder::new("pool_cancel")
        .title("Cancel a Task")
        .description("Cancel a pending or running task.")
        .handler(move |input: TaskIdInput| {
            let state = Arc::clone(&state);
            async move {
                let task_id = claude_pool::TaskId(input.task_id);
                match state.pool.cancel(&task_id).await {
                    Ok(()) => Ok(CallToolResult::text("cancelled")),
                    Err(e) => Ok(CallToolResult::error(e.to_string())),
                }
            }
        })
        .build()
}

/// **pool_fan_out** — Fan out N independent tasks in parallel, returns all results
///
/// # Description
/// Submit multiple independent prompts for parallel execution, blocking until all complete.
/// Results are returned in the same order as input prompts.
///
/// # Parameters
/// - **prompts** (array of strings, required) — List of prompts to execute in parallel
///
/// # Response Format
/// ```json
/// {
///   "results": [
///     { "output": "result 1", "tokens_used": 1200 },
///     { "output": "result 2", "tokens_used": 800 },
///     { "output": "result 3", "tokens_used": 950 }
///   ]
/// }
/// ```
///
/// # Example
/// Coordinator fanning out multiple reviews in parallel:
/// ```text
/// await client.call_tool("pool_fan_out", {
///   "prompts": [
///     "Review PR #1 for security",
///     "Review PR #2 for performance",
///     "Review PR #3 for design"
///   ]
/// })
/// // Returns: { results: [{output: "...", tokens_used: 1200}, ...] }
/// ```
///
/// # Error Cases
/// - **NoSlotsAvailable** — Pool has no idle slots for parallel execution
/// - **TaskFailed** — One or more tasks failed; array index indicates which failed
pub fn pool_fan_out_tool<S: PoolStore + 'static>(state: Arc<State<S>>) -> Tool {
    ToolBuilder::new("pool_fan_out")
        .title("Fan Out Tasks")
        .description(
            "Fan out multiple independent tasks in parallel across available slots. Returns all results.",
        )
        .handler(move |input: FanOutInput| {
            let state = Arc::clone(&state);
            async move {
                let prompts: Vec<&str> = input.prompts.iter().map(|s| s.as_str()).collect();
                match state.pool.fan_out(&prompts).await {
                    Ok(results) => Ok(CallToolResult::json(
                        serde_json::json!({ "results": results }),
                    )),
                    Err(e) => Ok(CallToolResult::error(e.to_string())),
                }
            }
        })
        .build()
}

/// **pool_drain** — Gracefully shut down the pool
///
/// # Description
/// Stop accepting new tasks, wait for all in-flight tasks to complete, then shut down all slots.
/// This is a destructive operation and cannot be undone without restarting.
///
/// # Parameters
/// None. This tool takes no parameters.
///
/// # Response Format
/// ```json
/// {
///   "slots_stopped": 8,
///   "tasks_completed": 42,
///   "total_time_seconds": 125
/// }
/// ```
///
/// # Example
/// Coordinator shutting down pool for deployment:
/// ```text
/// await client.call_tool("pool_drain", {})
/// // Returns: { slots_stopped: 8, tasks_completed: 42 }
/// ```
///
/// # Error Cases
/// - **DeadlockDetected** — Circular task dependencies prevent shutdown
pub fn pool_drain_tool<S: PoolStore + 'static>(state: Arc<State<S>>) -> Tool {
    ToolBuilder::new("pool_drain")
        .title("Drain the Pool")
        .description(
            "Gracefully shut down the pool. Waits for in-flight tasks, then stops all slots.",
        )
        .destructive()
        .no_params_handler(move || {
            let state = Arc::clone(&state);
            async move {
                match state.pool.drain().await {
                    Ok(summary) => Ok(CallToolResult::json(
                        serde_json::to_value(&summary).unwrap(),
                    )),
                    Err(e) => Ok(CallToolResult::error(e.to_string())),
                }
            }
        })
        .build()
}

/// **context_set** — Set shared context value (injected into slot system prompts)
///
/// # Description
/// Store a key-value pair in shared context. All slots have access to context variables and
/// can include them in their system prompts for decision-making.
///
/// # Parameters
/// - **key** (string, required) — Context key (e.g., "release_version", "build_status")
/// - **value** (string, required) — Context value (e.g., "0.3.0", "stable")
///
/// # Response Format
/// ```json
/// "ok"
/// ```
///
/// # Example
/// Coordinator setting shared release version:
/// ```text
/// await client.call_tool("context_set", {
///   "key": "release_version",
///   "value": "0.3.0"
/// })
/// // Returns: "ok"
/// ```
///
/// # Error Cases
/// - **KeyTooLong** — Context key exceeds max length
/// - **ValueTooLarge** — Context value exceeds max size
pub fn context_set_tool<S: PoolStore + 'static>(state: Arc<State<S>>) -> Tool {
    ToolBuilder::new("context_set")
        .title("Set Context")
        .description("Set a shared context value. Context is injected into slot system prompts.")
        .handler(move |input: ContextSetInput| {
            let state = Arc::clone(&state);
            async move {
                state.pool.set_context(input.key, input.value);
                Ok(CallToolResult::text("ok"))
            }
        })
        .build()
}

/// **context_get** — Get shared context value by key
///
/// # Description
/// Retrieve a context value set by `context_set`. Returns the value as plain text.
///
/// # Parameters
/// - **key** (string, required) — Context key to retrieve
///
/// # Response Format
/// ```json
/// "value content here"
/// ```
///
/// # Example
/// Coordinator checking release version:
/// ```text
/// await client.call_tool("context_get", { "key": "release_version" })
/// // Returns: "0.3.0"
/// ```
///
/// # Error Cases
/// - **KeyNotFound** — Key doesn't exist in context
pub fn context_get_tool<S: PoolStore + 'static>(state: Arc<State<S>>) -> Tool {
    ToolBuilder::new("context_get")
        .title("Get Context")
        .description("Get a shared context value by key.")
        .read_only()
        .handler(move |input: ContextKeyInput| {
            let state = Arc::clone(&state);
            async move {
                match state.pool.get_context(&input.key) {
                    Some(value) => Ok(CallToolResult::text(value)),
                    None => Ok(CallToolResult::error(format!(
                        "key not found: {}",
                        input.key
                    ))),
                }
            }
        })
        .build()
}

/// **context_delete** — Delete shared context value by key
///
/// # Description
/// Remove a context key-value pair. This is permanent for the pool session.
///
/// # Parameters
/// - **key** (string, required) — Context key to delete
///
/// # Response Format
/// ```json
/// "ok"
/// ```
///
/// # Example
/// Coordinator clearing release version:
/// ```text
/// await client.call_tool("context_delete", { "key": "release_version" })
/// // Returns: "ok"
/// ```
///
/// # Error Cases
/// - **KeyNotFound** — Key doesn't exist (non-fatal, still returns "ok")
pub fn context_delete_tool<S: PoolStore + 'static>(state: Arc<State<S>>) -> Tool {
    ToolBuilder::new("context_delete")
        .title("Delete Context")
        .description("Delete a shared context value by key.")
        .handler(move |input: ContextKeyInput| {
            let state = Arc::clone(&state);
            async move {
                state.pool.delete_context(&input.key);
                Ok(CallToolResult::text("ok"))
            }
        })
        .build()
}

/// **context_list** — List all shared context keys and values
///
/// # Description
/// Retrieve all context key-value pairs currently stored in the pool. Useful for debugging
/// or verifying shared state.
///
/// # Parameters
/// None. This tool takes no parameters.
///
/// # Response Format
/// ```json
/// {
///   "release_version": "0.3.0",
///   "build_status": "stable",
///   "deployment_target": "production"
/// }
/// ```
///
/// # Example
/// Coordinator inspecting all context:
/// ```text
/// await client.call_tool("context_list", {})
/// // Returns: { release_version: "0.3.0", build_status: "stable", ... }
/// ```
///
/// # Error Cases
/// - None (always returns JSON object, possibly empty)
pub fn context_list_tool<S: PoolStore + 'static>(state: Arc<State<S>>) -> Tool {
    ToolBuilder::new("context_list")
        .title("List Context")
        .description("List all shared context keys and values.")
        .read_only()
        .no_params_handler(move || {
            let state = Arc::clone(&state);
            async move {
                let entries = state.pool.list_context();
                let map: serde_json::Map<String, serde_json::Value> = entries
                    .into_iter()
                    .map(|(k, v)| (k, serde_json::Value::String(v)))
                    .collect();
                Ok(CallToolResult::json(serde_json::Value::Object(map)))
            }
        })
        .build()
}

/// **pool_configure_slot** — Set name/role/description for persistent slot identity
///
/// # Description
/// Assign metadata to a slot for identification and filtering. Configured slots persist across runs.
///
/// # Parameters
/// - **slot_id** (string, required) — Slot ID to configure (e.g., "slot-0")
/// - **name** (string, optional) — Human-readable name (e.g., "code-reviewer")
/// - **role** (string, optional) — Role classification (e.g., "coordinator", "worker")
/// - **description** (string, optional) — Purpose description
///
/// # Response Format
/// ```json
/// {
///   "slot_id": "slot-0",
///   "name": "code-reviewer",
///   "role": "worker",
///   "description": "Reviews pull requests for security"
/// }
/// ```
///
/// # Example
/// Coordinator naming a slot for later discovery:
/// ```text
/// await client.call_tool("pool_configure_slot", {
///   "slot_id": "slot-0",
///   "name": "code-reviewer",
///   "role": "worker"
/// })
/// ```
///
/// # Error Cases
/// - **SlotNotFound** — Slot ID doesn't exist
pub fn pool_configure_slot_tool<S: PoolStore + 'static>(state: Arc<State<S>>) -> Tool {
    ToolBuilder::new("pool_configure_slot")
        .title("Configure Slot")
        .description("Set name/role/description for a slot to give it persistent identity")
        .handler(move |input: ConfigureSlotInput| {
            let state = Arc::clone(&state);
            async move {
                let slot_id = claude_pool::SlotId(input.slot_id.clone());

                match state.pool.store().get_slot(&slot_id).await {
                    Ok(Some(mut slot)) => {
                        // Update identity fields
                        if let Some(name) = input.name {
                            slot.config.name = Some(name);
                        }
                        if let Some(role) = input.role {
                            slot.config.role = Some(role);
                        }
                        if let Some(description) = input.description {
                            slot.config.description = Some(description);
                        }

                        // Persist updated slot
                        match state.pool.store().put_slot(slot.clone()).await {
                            Ok(_) => {
                                let response = serde_json::json!({
                                    "slot_id": slot_id.0,
                                    "name": slot.config.name,
                                    "role": slot.config.role,
                                    "description": slot.config.description,
                                });
                                Ok(CallToolResult::json(response))
                            }
                            Err(e) => {
                                Ok(CallToolResult::error(format!("failed to update slot: {e}")))
                            }
                        }
                    }
                    Ok(None) => Ok(CallToolResult::error(format!(
                        "slot not found: {}",
                        input.slot_id
                    ))),
                    Err(e) => Ok(CallToolResult::error(format!("failed to fetch slot: {e}"))),
                }
            }
        })
        .build()
}

// ── Skill + chain tools ──────────────────────────────────────────────

#[derive(Debug, Deserialize, JsonSchema)]
pub struct SkillRunInput {
    /// Name of the skill to run.
    pub skill: String,
    /// Skill arguments as key-value pairs.
    pub arguments: std::collections::HashMap<String, String>,
    /// Model override.
    pub model: Option<String>,
    /// Effort override.
    pub effort: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ChainInput {
    /// Ordered list of chain steps.
    pub steps: Vec<ChainStepInput>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct SubmitChainInput {
    /// Ordered list of chain steps.
    pub steps: Vec<ChainStepInput>,
    /// Tags for grouping/filtering.
    pub tags: Option<Vec<String>>,
    /// Isolation mode: "worktree" for per-chain git worktree, or omit for default (none).
    pub isolation: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct FanOutChainsInput {
    /// List of chains, each a list of steps.
    pub chains: Vec<Vec<ChainStepInput>>,
    /// Tags for grouping/filtering.
    pub tags: Option<Vec<String>>,
    /// Isolation mode: "worktree" for per-chain git worktree, or omit for default (none).
    pub isolation: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ChainStepInput {
    /// Step name.
    pub name: String,
    /// Step type: "prompt" or "skill".
    #[serde(rename = "type")]
    pub step_type: String,
    /// For prompt steps: the prompt text. For skill steps: the skill name.
    pub value: String,
    /// For skill steps: arguments as key-value pairs.
    pub arguments: Option<std::collections::HashMap<String, String>>,
    /// Model override for this step.
    pub model: Option<String>,
    /// Effort override for this step.
    pub effort: Option<String>,
    /// Number of retries on failure (default: 0).
    pub retries: Option<u32>,
    /// Recovery prompt template on exhausted retries. {error} and {previous_output} are substituted.
    pub recovery_prompt: Option<String>,
    /// Extract named values from this step's JSON output for use in later steps.
    /// Key = variable name, Value = dot-path (e.g. "files_changed", "result.summary", ".").
    /// Reference in later prompts as: {steps.STEP_NAME.VAR_NAME}
    pub output_vars: Option<std::collections::HashMap<String, String>>,
}

fn convert_chain_steps(steps: Vec<ChainStepInput>) -> Vec<claude_pool::ChainStep> {
    steps
        .into_iter()
        .map(|s| {
            let action = match s.step_type.as_str() {
                "skill" => claude_pool::StepAction::Skill {
                    skill: s.value,
                    arguments: s.arguments.unwrap_or_default(),
                },
                _ => claude_pool::StepAction::Prompt { prompt: s.value },
            };
            let config = task_config_from(s.model, s.effort, None, None, None, None, None);
            let failure_policy = claude_pool::StepFailurePolicy {
                retries: s.retries.unwrap_or(0),
                recovery_prompt: s.recovery_prompt,
            };
            claude_pool::ChainStep {
                name: s.name,
                action,
                config,
                failure_policy,
                output_vars: s.output_vars.unwrap_or_default(),
            }
        })
        .collect()
}

/// **pool_skill_run** — Run a registered skill by name with arguments (blocks)
///
/// # Description
/// Execute a named skill with provided arguments. Returns result immediately.
/// Skills are reusable prompt templates that encapsulate complex workflows.
///
/// # Parameters
/// - **skill** (string, required) — Skill name to run
/// - **arguments** (object, required) — Key-value pairs matching skill's argument definitions
/// - **model** (string, optional) — Model override for this skill run
/// - **effort** (string, optional) — Effort override: "min", "low", "medium", "high", "max"
///
/// # Response Format
/// ```json
/// {
///   "output": "skill execution result",
///   "tokens_used": 2100,
///   "duration_ms": 4500
/// }
/// ```
///
/// # Example
/// Coordinator running a security review skill:
/// ```text
/// await client.call_tool("pool_skill_run", {
///   "skill": "security_review",
///   "arguments": { "code": "...", "focus": "injection attacks" }
/// })
/// ```
///
/// # Error Cases
/// - **SkillNotFound** — Skill name doesn't exist
/// - **InvalidArguments** — Missing or invalid argument values
/// - **SkillFailed** — Skill execution error
pub fn pool_skill_run_tool<S: PoolStore + 'static>(state: Arc<State<S>>) -> Tool {
    ToolBuilder::new("pool_skill_run")
        .title("Run a Skill")
        .description("Run a registered skill by name with arguments. Blocks until completion.")
        .handler(move |input: SkillRunInput| {
            let state = Arc::clone(&state);
            async move {
                let registry = state.skills.read().await;
                let skill = match registry.get(&input.skill) {
                    Some(s) => s.clone(),
                    None => {
                        return Ok(CallToolResult::error(format!(
                            "skill not found: {}",
                            input.skill
                        )));
                    }
                };
                drop(registry);

                let prompt = match skill.render(&input.arguments) {
                    Ok(p) => p,
                    Err(e) => return Ok(CallToolResult::error(e.to_string())),
                };

                // Merge skill config with per-call overrides.
                let mut config = skill.config.unwrap_or_default();
                if let Some(model) = input.model {
                    config.model = Some(model);
                }
                if let Some(effort) = input.effort {
                    config.effort = parse_effort(&effort);
                }

                match state.pool.run(&prompt).config(config).await {
                    Ok(result) => Ok(CallToolResult::json(serde_json::to_value(&result).unwrap())),
                    Err(e) => Ok(CallToolResult::error(e.to_string())),
                }
            }
        })
        .build()
}

/// **pool_chain** — Chain sequential steps, blocks until all complete
///
/// # Description
/// Execute a pipeline where each step's output feeds into the next as context.
/// Steps can be inline prompts or skill references. For long chains, use `pool_submit_chain` instead.
///
/// # Parameters
/// - **steps** (array, required) — List of step objects with name, type ("prompt" or "skill"), and value
///
/// # Response Format
/// ```json
/// {
///   "steps_executed": 3,
///   "final_output": "pipeline result",
///   "total_tokens": 3200,
///   "execution_path": [
///     { "step": "lint", "duration_ms": 1200 },
///     { "step": "test", "duration_ms": 2100 },
///     { "step": "build", "duration_ms": 800 }
///   ]
/// }
/// ```
///
/// # Example
/// Coordinator chaining lint → test → build:
/// ```text
/// await client.call_tool("pool_chain", {
///   "steps": [
///     { "name": "lint", "type": "prompt", "value": "Lint this code: ..." },
///     { "name": "test", "type": "skill", "value": "run_tests" },
///     { "name": "build", "type": "prompt", "value": "Build the project" }
///   ]
/// })
/// ```
///
/// # Error Cases
/// - **StepFailed** — A step failed; remaining steps are skipped
/// - **InvalidStep** — Step type or name invalid
pub fn pool_chain_tool<S: PoolStore + 'static>(state: Arc<State<S>>) -> Tool {
    ToolBuilder::new("pool_chain")
        .title("Chain Steps")
        .description(
            "Chain a sequential pipeline of steps. Each step's output feeds the next. \
             Steps can be inline prompts or skill references. Blocks until all steps \
             complete. For long chains, fire a chain with pool_submit_chain instead.",
        )
        .handler(move |input: ChainInput| {
            let state = Arc::clone(&state);
            async move {
                let steps = convert_chain_steps(input.steps);
                let skills = state.skills.read().await;
                match claude_pool::execute_chain(&state.pool, &skills, &steps).await {
                    Ok(result) => Ok(CallToolResult::json(serde_json::to_value(&result).unwrap())),
                    Err(e) => Ok(CallToolResult::error(e.to_string())),
                }
            }
        })
        .build()
}

/// **pool_submit_chain** — Fire chain for async execution, returns task_id immediately
///
/// # Description
/// Submit a chain pipeline for asynchronous execution without waiting.
/// Check progress with `pool_chain_result` to see per-step completion.
///
/// # Parameters
/// - **steps** (array, required) — Pipeline steps (same format as pool_chain)
/// - **tags** (array, optional) — Tags for grouping chains
/// - **isolation** (string, optional) — Isolation mode: "none", "clone", or "worktree" (default)
///
/// # Response Format
/// ```json
/// {
///   "task_id": "task-chain-abc1234567"
/// }
/// ```
///
/// # Example
/// Coordinator firing a long chain for background execution:
/// ```text
/// await client.call_tool("pool_submit_chain", {
///   "steps": [
///     { "name": "lint", "type": "prompt", "value": "Lint the code" },
///     { "name": "test", "type": "skill", "value": "run_tests" }
///   ],
///   "tags": ["ci", "release"]
/// })
/// // Returns: { task_id: "task-chain-abc..." }
/// ```
///
/// # Error Cases
/// - **InvalidStep** — Step definition is malformed
/// - **StorageError** — Chain could not be persisted
pub fn pool_submit_chain_tool<S: PoolStore + 'static>(state: Arc<State<S>>) -> Tool {
    ToolBuilder::new("pool_submit_chain")
        .title("Fire a Chain")
        .description(
            "Fire off a chain for async execution. Returns a task_id immediately. \
             Check on it with pool_chain_result for per-step progress.",
        )
        .handler(move |input: SubmitChainInput| {
            let state = Arc::clone(&state);
            async move {
                let steps = convert_chain_steps(input.steps);
                let isolation = parse_isolation(input.isolation.as_deref());
                let options = claude_pool::ChainOptions {
                    tags: input.tags.unwrap_or_default(),
                    isolation,
                };
                let skills = state.skills.read().await;
                match state.pool.submit_chain(steps, &skills, options).await {
                    Ok(task_id) => Ok(CallToolResult::json(
                        serde_json::json!({ "task_id": task_id.0 }),
                    )),
                    Err(e) => Ok(CallToolResult::error(e.to_string())),
                }
            }
        })
        .build()
}

pub fn pool_fan_out_chains_tool<S: PoolStore + 'static>(state: Arc<State<S>>) -> Tool {
    ToolBuilder::new("pool_fan_out_chains")
        .title("Fan Out Chains")
        .description(
            "Fan out multiple chains in parallel, each on its own slot. \
             Returns all task IDs. Check on each with pool_chain_result.",
        )
        .handler(move |input: FanOutChainsInput| {
            let state = Arc::clone(&state);
            async move {
                let chains = input.chains.into_iter().map(convert_chain_steps).collect();
                let isolation = parse_isolation(input.isolation.as_deref());
                let options = claude_pool::ChainOptions {
                    tags: input.tags.unwrap_or_default(),
                    isolation,
                };
                let skills = state.skills.read().await;
                match state.pool.fan_out_chains(chains, &skills, options).await {
                    Ok(task_ids) => Ok(CallToolResult::json(serde_json::json!({
                        "task_ids": task_ids.iter().map(|id| &id.0).collect::<Vec<_>>()
                    }))),
                    Err(e) => Ok(CallToolResult::error(e.to_string())),
                }
            }
        })
        .build()
}

/// **pool_chain_result** — Check on fired chain, shows per-step progress
///
/// # Description
/// Poll a chain for detailed progress. Shows which step is currently running, completed steps,
/// failed steps, and total execution cost.
///
/// # Parameters
/// - **task_id** (string, required) — Chain task ID from `pool_submit_chain`
///
/// # Response Format
/// ```json
/// {
///   "steps": 3,
///   "current_step": 1,
///   "completed": ["lint"],
///   "status": "running",
///   "total_cost": 1500,
///   "step_details": [
///     { "name": "lint", "status": "completed", "duration_ms": 1200 },
///     { "name": "test", "status": "running", "duration_ms": 450 }
///   ]
/// }
/// ```
///
/// # Example
/// Coordinator checking chain progress:
/// ```text
/// await client.call_tool("pool_chain_result", { "task_id": "task-chain-abc123" })
/// // Returns: { steps: 3, current_step: 1, completed: ["lint"], status: "running" }
/// ```
///
/// # Error Cases
/// - **ChainNotFound** — Task ID is not a chain
/// - **TaskNotFound** — Task ID doesn't exist
pub fn pool_chain_result_tool<S: PoolStore + 'static>(state: Arc<State<S>>) -> Tool {
    ToolBuilder::new("pool_chain_result")
        .title("Check on a Chain")
        .description(
            "Check on a fired chain. Shows per-step progress: which step is running, \
             completed steps, and overall status.",
        )
        .read_only()
        .handler(move |input: TaskIdInput| {
            let state = Arc::clone(&state);
            async move {
                let task_id = claude_pool::TaskId(input.task_id.clone());
                match state.pool.chain_progress(&task_id) {
                    Some(progress) => Ok(CallToolResult::json(
                        serde_json::to_value(&progress).unwrap(),
                    )),
                    None => {
                        // Fall back to checking if the task exists at all.
                        match state.pool.result(&task_id).await {
                            Ok(Some(r)) => {
                                Ok(CallToolResult::json(serde_json::to_value(&r).unwrap()))
                            }
                            Ok(None) => Ok(CallToolResult::error(format!(
                                "no chain found for task_id: {}",
                                input.task_id,
                            ))),
                            Err(e) => Ok(CallToolResult::error(e.to_string())),
                        }
                    }
                }
            }
        })
        .build()
}

/// Cancel a running chain, skipping remaining steps.
pub fn pool_cancel_chain_tool<S: PoolStore + 'static>(state: Arc<State<S>>) -> Tool {
    ToolBuilder::new("pool_cancel_chain")
        .title("Cancel a Chain")
        .description(
            "Cancel a running chain that was fired with pool_submit_chain or pool_fan_out_chains. \
             The current step finishes before cancellation takes effect. Remaining steps are \
             skipped. Check on the chain with pool_chain_result to confirm.",
        )
        .handler(move |input: TaskIdInput| {
            let state = Arc::clone(&state);
            async move {
                let task_id = claude_pool::TaskId(input.task_id.clone());
                match state.pool.cancel_chain(&task_id).await {
                    Ok(()) => Ok(CallToolResult::json(serde_json::json!({
                        "status": "cancellation_requested",
                        "task_id": input.task_id,
                    }))),
                    Err(e) => Ok(CallToolResult::error(e.to_string())),
                }
            }
        })
        .build()
}

pub fn pool_invoke_workflow_tool<S: PoolStore + 'static>(state: Arc<State<S>>) -> Tool {
    ToolBuilder::new("pool_invoke_workflow")
        .title("Invoke Workflow")
        .description(
            "Submit a named workflow template with arguments. Returns a task_id immediately. \
             Example workflows: 'issue_to_pr', 'refactor_and_test', 'review_and_fix'.",
        )
        .handler(move |input: InvokeWorkflowInput| {
            let state = Arc::clone(&state);
            async move {
                let skills = state.skills.read().await;
                match state
                    .pool
                    .submit_workflow(
                        &input.workflow,
                        input.arguments,
                        &skills,
                        &state.workflows,
                        input.tags.unwrap_or_default(),
                    )
                    .await
                {
                    Ok(task_id) => Ok(CallToolResult::json(serde_json::json!({
                        "task_id": task_id.0,
                        "workflow": input.workflow,
                    }))),
                    Err(e) => Ok(CallToolResult::error(e.to_string())),
                }
            }
        })
        .build()
}

/// Build all pool tools.
pub fn pool_scale_up_tool<S: PoolStore + 'static>(state: Arc<State<S>>) -> Tool {
    ToolBuilder::new("pool_scale_up")
        .title("Scale Up the Pool")
        .description("Add N new slots to the pool. Returns the new total slot count.")
        .handler(move |input: ScalingInput| {
            let state = Arc::clone(&state);
            async move {
                match state.pool.scale_up(input.count).await {
                    Ok(new_count) => Ok(CallToolResult::json(serde_json::json!({
                        "success": true,
                        "new_slot_count": new_count,
                        "details": format!("Scaled up by {} slots", input.count),
                    }))),
                    Err(e) => Ok(CallToolResult::error(e.to_string())),
                }
            }
        })
        .build()
}

pub fn pool_scale_down_tool<S: PoolStore + 'static>(state: Arc<State<S>>) -> Tool {
    ToolBuilder::new("pool_scale_down")
        .title("Scale Down the Pool")
        .description(
            "Remove N slots from the pool. Removes idle slots first, \
             then waits for busy slots to complete. Returns the new total slot count.",
        )
        .handler(move |input: ScalingInput| {
            let state = Arc::clone(&state);
            async move {
                match state.pool.scale_down(input.count).await {
                    Ok(new_count) => Ok(CallToolResult::json(serde_json::json!({
                        "success": true,
                        "new_slot_count": new_count,
                        "details": format!("Scaled down by {} slots", input.count),
                    }))),
                    Err(e) => Ok(CallToolResult::error(e.to_string())),
                }
            }
        })
        .build()
}

pub fn pool_set_target_slots_tool<S: PoolStore + 'static>(state: Arc<State<S>>) -> Tool {
    ToolBuilder::new("pool_set_target_slots")
        .title("Set Pool Size")
        .description("Set the pool to a specific number of slots, scaling up or down as needed.")
        .handler(move |input: SetTargetSlotsInput| {
            let state = Arc::clone(&state);
            async move {
                match state.pool.set_target_slots(input.target).await {
                    Ok(new_count) => Ok(CallToolResult::json(serde_json::json!({
                        "success": true,
                        "new_slot_count": new_count,
                        "target": input.target,
                    }))),
                    Err(e) => Ok(CallToolResult::error(e.to_string())),
                }
            }
        })
        .build()
}

// ── Skill management tools ──────────────────────────────────────────

/// List registered skills with optional scope/source filters.
pub fn pool_skill_list_tool<S: PoolStore + 'static>(state: Arc<State<S>>) -> Tool {
    ToolBuilder::new("pool_skill_list")
        .title("List Skills")
        .description("List skills available in the pool, with optional scope/source filters. Skills come from builtins, global (~/.claude-pool/skills/), or project (.claude-pool/skills/).")
        .read_only()
        .handler(move |input: SkillListInput| {
            let state = Arc::clone(&state);
            async move {
                let registry = state.skills.read().await;
                let scope_filter = input.scope.as_deref().map(parse_scope);
                let source_filter = input.source.as_deref().and_then(parse_source);

                let mut results: Vec<_> = registry
                    .list_registered()
                    .into_iter()
                    .filter(|rs| {
                        if let Some(scope) = scope_filter
                            && rs.skill.scope != scope
                        {
                            return false;
                        }
                        if let Some(source) = source_filter
                            && rs.source != source
                        {
                            return false;
                        }
                        true
                    })
                    .map(|rs| {
                        let mut entry = serde_json::json!({
                            "name": rs.skill.name,
                            "description": rs.skill.description,
                            "scope": rs.skill.scope.to_string(),
                            "source": rs.source.to_string(),
                        });
                        if let Some(ref hint) = rs.skill.argument_hint {
                            entry["argument_hint"] = serde_json::json!(hint);
                        }
                        entry
                    })
                    .collect();
                results.sort_by(|a, b| a["name"].as_str().cmp(&b["name"].as_str()));
                Ok(CallToolResult::json(serde_json::json!(results)))
            }
        })
        .build()
}

/// Get full details of a skill by name, including prompt template.
pub fn pool_skill_get_tool<S: PoolStore + 'static>(state: Arc<State<S>>) -> Tool {
    ToolBuilder::new("pool_skill_get")
        .title("Get Skill Details")
        .description("Get full details of a skill by name, including prompt template.")
        .read_only()
        .handler(move |input: SkillGetInput| {
            let state = Arc::clone(&state);
            async move {
                let registry = state.skills.read().await;
                match registry.get_registered(&input.name) {
                    Some(rs) => {
                        // A skill is "customized" if it's a builtin name but
                        // loaded from project/global/runtime source.
                        let customized = rs.source != SkillSource::Builtin
                            && claude_pool::skill::builtin_skills()
                                .iter()
                                .any(|b| b.name == rs.skill.name);
                        let response = serde_json::json!({
                            "name": rs.skill.name,
                            "description": rs.skill.description,
                            "prompt": rs.skill.prompt,
                            "arguments": rs.skill.arguments.iter().map(|a| serde_json::json!({
                                "name": a.name,
                                "description": a.description,
                                "required": a.required,
                            })).collect::<Vec<_>>(),
                            "scope": rs.skill.scope.to_string(),
                            "source": rs.source.to_string(),
                            "customized": customized,
                            "config": rs.skill.config,
                        });
                        Ok(CallToolResult::json(response))
                    }
                    None => Ok(CallToolResult::error(format!(
                        "skill not found: {}",
                        input.name
                    ))),
                }
            }
        })
        .build()
}

/// Register a skill at runtime. Ephemeral unless saved with pool_skill_save.
pub fn pool_skill_add_tool<S: PoolStore + 'static>(state: Arc<State<S>>) -> Tool {
    ToolBuilder::new("pool_skill_add")
        .title("Add a Skill")
        .description(
            "Register a skill at runtime. Ephemeral (lost on restart) unless saved \
             with pool_skill_save. Overwrites any existing skill with the same name.",
        )
        .handler(move |input: SkillAddInput| {
            let state = Arc::clone(&state);
            async move {
                let scope = input.scope.as_deref().map(parse_scope).unwrap_or_default();
                let arguments = input
                    .arguments
                    .into_iter()
                    .map(|a| claude_pool::SkillArgument {
                        name: a.name,
                        description: a.description,
                        required: a.required,
                    })
                    .collect();
                let config: Option<TaskOverrides> =
                    input.config.and_then(|v| serde_json::from_value(v).ok());
                let skill = claude_pool::Skill {
                    name: input.name.clone(),
                    description: input.description,
                    prompt: input.prompt,
                    arguments,
                    config,
                    scope,
                    argument_hint: None,
                    skill_dir: None,
                };
                let mut registry = state.skills.write().await;
                let overwritten = registry.get(&input.name).is_some();
                registry.register(skill, SkillSource::Runtime);
                Ok(CallToolResult::json(serde_json::json!({
                    "name": input.name,
                    "overwritten": overwritten,
                    "source": "runtime",
                })))
            }
        })
        .build()
}

/// Remove a skill by name. Runtime-only, does not delete files.
pub fn pool_skill_remove_tool<S: PoolStore + 'static>(state: Arc<State<S>>) -> Tool {
    ToolBuilder::new("pool_skill_remove")
        .title("Remove a Skill")
        .description("Remove a skill by name. Runtime-only, does not delete files on disk.")
        .handler(move |input: SkillRemoveInput| {
            let state = Arc::clone(&state);
            async move {
                let mut registry = state.skills.write().await;
                match registry.remove(&input.name) {
                    Some(_) => Ok(CallToolResult::json(serde_json::json!({
                        "removed": input.name,
                    }))),
                    None => Ok(CallToolResult::error(format!(
                        "skill not found: {}",
                        input.name
                    ))),
                }
            }
        })
        .build()
}

/// Persist a skill to the project skills directory as a SKILL.md folder.
pub fn pool_skill_save_tool<S: PoolStore + 'static>(state: Arc<State<S>>) -> Tool {
    ToolBuilder::new("pool_skill_save")
        .title("Save Skill to Disk")
        .description(
            "Persist a skill to the project skills directory as a SKILL.md folder \
             (Agent Skills standard). Creates/overwrites {dir}/{name}/SKILL.md.",
        )
        .handler(move |input: SkillSaveInput| {
            let state = Arc::clone(&state);
            async move {
                let skill = {
                    let registry = state.skills.read().await;
                    match registry.get(&input.name) {
                        Some(s) => s.clone(),
                        None => {
                            return Ok(CallToolResult::error(format!(
                                "skill not found: {}",
                                input.name
                            )));
                        }
                    }
                };

                let base_dir = input
                    .dir
                    .map(PathBuf::from)
                    .unwrap_or_else(|| state.skills_dir.clone());

                let skill_dir = base_dir.join(&skill.name);
                if let Err(e) = std::fs::create_dir_all(&skill_dir) {
                    return Ok(CallToolResult::error(format!(
                        "failed to create directory {}: {e}",
                        skill_dir.display()
                    )));
                }

                let skill_md = skill_to_skill_md(&skill);
                let path = skill_dir.join("SKILL.md");

                if let Err(e) = std::fs::write(&path, &skill_md) {
                    return Ok(CallToolResult::error(format!(
                        "failed to write {}: {e}",
                        path.display()
                    )));
                }

                // Update source to Project since it's now persisted.
                {
                    let mut registry = state.skills.write().await;
                    if let Some(existing) = registry.get(&input.name).cloned() {
                        registry.register(existing, SkillSource::Project);
                    }
                }

                Ok(CallToolResult::json(serde_json::json!({
                    "saved": input.name,
                    "path": path.display().to_string(),
                    "format": "SKILL.md",
                })))
            }
        })
        .build()
}

/// Convert a Skill to SKILL.md format (YAML frontmatter + markdown body).
fn skill_to_skill_md(skill: &claude_pool::Skill) -> String {
    use std::fmt::Write;

    let mut out = String::new();
    out.push_str("---\n");
    writeln!(out, "name: {}", skill.name).unwrap();
    writeln!(
        out,
        "description: \"{}\"",
        skill.description.replace('"', "\\\"")
    )
    .unwrap();

    let has_metadata = skill.scope != claude_pool::SkillScope::Task
        || !skill.arguments.is_empty()
        || skill.config.is_some();

    if has_metadata {
        out.push_str("metadata:\n");
        if skill.scope != claude_pool::SkillScope::Task {
            writeln!(out, "  scope: {}", skill.scope).unwrap();
        }
        if !skill.arguments.is_empty() {
            out.push_str("  arguments:\n");
            for arg in &skill.arguments {
                writeln!(out, "    - name: {}", arg.name).unwrap();
                writeln!(
                    out,
                    "      description: \"{}\"",
                    arg.description.replace('"', "\\\"")
                )
                .unwrap();
                writeln!(out, "      required: {}", arg.required).unwrap();
            }
        }
    }

    out.push_str("---\n\n");
    out.push_str(&skill.prompt);
    out.push('\n');
    out
}

/// Eject a builtin skill to disk for customization.
pub fn pool_skill_eject_tool<S: PoolStore + 'static>(state: Arc<State<S>>) -> Tool {
    ToolBuilder::new("pool_skill_eject")
        .title("Eject Builtin Skill")
        .description(
            "Write a builtin skill to disk as a SKILL.md folder for customization. \
             The disk version shadows the builtin. Delete the folder to restore the default.",
        )
        .handler(move |input: SkillEjectInput| {
            let state = Arc::clone(&state);
            async move {
                // Find the builtin version of the skill.
                let builtin = claude_pool::skill::builtin_skills()
                    .into_iter()
                    .find(|s| s.name == input.name);

                let skill = match builtin {
                    Some(s) => s,
                    None => {
                        return Ok(CallToolResult::error(format!(
                            "not a builtin skill: {} (only builtins can be ejected)",
                            input.name
                        )));
                    }
                };

                let base_dir = input
                    .dir
                    .map(PathBuf::from)
                    .unwrap_or_else(|| state.skills_dir.clone());

                let skill_dir = base_dir.join(&skill.name);
                if let Err(e) = std::fs::create_dir_all(&skill_dir) {
                    return Ok(CallToolResult::error(format!(
                        "failed to create directory {}: {e}",
                        skill_dir.display()
                    )));
                }

                let skill_md = skill_to_skill_md(&skill);
                let path = skill_dir.join("SKILL.md");

                if let Err(e) = std::fs::write(&path, &skill_md) {
                    return Ok(CallToolResult::error(format!(
                        "failed to write {}: {e}",
                        path.display()
                    )));
                }

                // Re-register as Project source so it shows as customized.
                {
                    let mut registry = state.skills.write().await;
                    registry.register(skill, SkillSource::Project);
                }

                Ok(CallToolResult::json(serde_json::json!({
                    "ejected": input.name,
                    "path": path.display().to_string(),
                    "hint": "edit the SKILL.md to customize, delete the folder to restore builtin",
                })))
            }
        })
        .build()
}

pub fn pool_send_message_tool<S: PoolStore + 'static>(state: Arc<State<S>>) -> Tool {
    ToolBuilder::new("pool_send_message")
        .title("Send Message Between Slots")
        .description("Send a message from one slot to another. Returns the message ID.")
        .handler(move |input: SendMessageInput| {
            let state = Arc::clone(&state);
            async move {
                let from = claude_pool::types::SlotId(input.from);
                let to = claude_pool::types::SlotId(input.to);
                let message_id = state.pool.send_message(from, to, input.content);
                Ok(CallToolResult::json(serde_json::json!({
                    "message_id": message_id,
                })))
            }
        })
        .build()
}

pub fn pool_read_messages_tool<S: PoolStore + 'static>(state: Arc<State<S>>) -> Tool {
    ToolBuilder::new("pool_read_messages")
        .title("Read Messages from Slot")
        .description("Drain and read all messages for a slot, removing them from the inbox.")
        .handler(move |input: ReadMessagesInput| {
            let state = Arc::clone(&state);
            async move {
                let slot_id = claude_pool::types::SlotId(input.slot_id);
                let messages = state.pool.read_messages(&slot_id);
                Ok(CallToolResult::json(
                    serde_json::to_value(&messages).unwrap(),
                ))
            }
        })
        .build()
}

pub fn pool_peek_messages_tool<S: PoolStore + 'static>(state: Arc<State<S>>) -> Tool {
    ToolBuilder::new("pool_peek_messages")
        .title("Peek Messages from Slot")
        .description("Read messages from a slot's inbox without removing them.")
        .read_only()
        .handler(move |input: PeekMessagesInput| {
            let state = Arc::clone(&state);
            async move {
                let slot_id = claude_pool::types::SlotId(input.slot_id);
                let messages = state.pool.peek_messages(&slot_id);
                Ok(CallToolResult::json(
                    serde_json::to_value(&messages).unwrap(),
                ))
            }
        })
        .build()
}

pub fn pool_broadcast_tool<S: PoolStore + 'static>(state: Arc<State<S>>) -> Tool {
    ToolBuilder::new("pool_broadcast")
        .title("Broadcast Message to All Slots")
        .description(
            "Send a message from one slot to all other active slots. Returns the list of message IDs.",
        )
        .handler(move |input: BroadcastInput| {
            let state = Arc::clone(&state);
            async move {
                let from = claude_pool::types::SlotId(input.from);
                match state.pool.broadcast_message(from, input.content).await {
                    Ok(ids) => {
                        let count = ids.len();
                        Ok(CallToolResult::json(serde_json::json!({
                            "message_ids": ids,
                            "recipients": count,
                        })))
                    }
                    Err(e) => Ok(CallToolResult::error(e.to_string())),
                }
            }
        })
        .build()
}

/// **pool_find_slots** — Query slots by name, role, and/or state (all filters optional)
///
/// # Description
/// Find slots matching one or more criteria. All filters are optional; omitted filters match anything.
///
/// # Parameters
/// - **name** (string, optional) — Exact match on slot name (e.g., "code-reviewer")
/// - **role** (string, optional) — Exact match on slot role (e.g., "worker", "coordinator")
/// - **state** (string, optional) — Filter by state: "idle", "busy", "stopped", "errored"
///
/// # Response Format
/// ```json
/// {
///   "slots": [
///     {
///       "id": "slot-0",
///       "state": "idle",
///       "name": "code-reviewer",
///       "role": "worker",
///       "description": "Reviews PRs for quality",
///       "current_task": null,
///       "tasks_completed": 42,
///       "cost_microdollars": 125000
///     }
///   ],
///   "count": 1
/// }
/// ```
///
/// # Example
/// Coordinator finding idle worker slots:
/// ```text
/// await client.call_tool("pool_find_slots", { "state": "idle", "role": "worker" })
/// // Returns: { slots: [{id: "slot-0", state: "idle", ...}], count: 1 }
/// ```
///
/// # Error Cases
/// - **InvalidState** — State filter value not recognized
pub fn pool_find_slots_tool<S: PoolStore + 'static>(state: Arc<State<S>>) -> Tool {
    ToolBuilder::new("pool_find_slots")
        .title("Find Slots by Name, Role, or State")
        .description(
            "Query slots by name, role, and/or state. All filters are optional; omitted filters match everything.",
        )
        .read_only()
        .handler(move |input: FindSlotsInput| {
            let state = Arc::clone(&state);
            async move {
                let slot_state = input.state.as_deref().and_then(|s| match s {
                    "idle" => Some(claude_pool::types::SlotState::Idle),
                    "busy" => Some(claude_pool::types::SlotState::Busy),
                    "stopped" => Some(claude_pool::types::SlotState::Stopped),
                    "errored" => Some(claude_pool::types::SlotState::Errored),
                    _ => None,
                });
                match state
                    .pool
                    .find_slots(input.name.as_deref(), input.role.as_deref(), slot_state)
                    .await
                {
                    Ok(slots) => {
                        let results: Vec<_> = slots
                            .iter()
                            .map(|s| {
                                serde_json::json!({
                                    "id": s.id.0,
                                    "state": s.state,
                                    "name": s.config.name,
                                    "role": s.config.role,
                                    "description": s.config.description,
                                    "current_task": s.current_task.as_ref().map(|t| &t.0),
                                    "tasks_completed": s.tasks_completed,
                                    "cost_microdollars": s.cost_microdollars,
                                })
                            })
                            .collect();
                        Ok(CallToolResult::json(serde_json::json!({
                            "slots": results,
                            "count": results.len(),
                        })))
                    }
                    Err(e) => Ok(CallToolResult::error(e.to_string())),
                }
            }
        })
        .build()
}

/// **pool_claim** — Self-service: idle slot grabs next pending task
///
/// # Description
/// Atomically claim the oldest unassigned pending task. Used by slots to pull work.
/// Returns the task ID or null if queue is empty.
///
/// # Parameters
/// - **slot_id** (string, required) — Slot ID claiming the task
/// - **labels** (array, optional) — Only claim tasks with one of these labels
///
/// # Response Format
/// Task found:
/// ```json
/// {
///   "task_id": "task-abc123",
///   "prompt": "the task prompt",
///   "tags": ["review", "urgent"]
/// }
/// ```
/// No tasks available:
/// ```json
/// {
///   "message": "no pending tasks"
/// }
/// ```
///
/// # Example
/// Slot self-service claiming:
/// ```text
/// await client.call_tool("pool_claim", { "slot_id": "slot-2" })
/// // Returns: { task_id: "task-abc123", ... }
/// ```
///
/// # Error Cases
/// - **SlotNotFound** — Slot ID doesn't exist
pub fn pool_claim_tool<S: PoolStore + 'static>(state: Arc<State<S>>) -> Tool {
    ToolBuilder::new("pool_claim")
        .title("Claim Next Pending Task")
        .description(
            "Self-service task claiming: an idle slot grabs the next pending task from the queue. \
             Returns the claimed task ID, or null if no tasks are waiting. The task executes \
             in the background on the claiming slot.",
        )
        .handler(move |input: ClaimInput| {
            let state = Arc::clone(&state);
            async move {
                let slot_id = claude_pool::types::SlotId(input.slot_id);
                match state.pool.claim(&slot_id).await {
                    Ok(Some(task_id)) => Ok(CallToolResult::json(serde_json::json!({
                        "claimed": true,
                        "task_id": task_id.0,
                    }))),
                    Ok(None) => Ok(CallToolResult::json(serde_json::json!({
                        "claimed": false,
                        "task_id": null,
                        "reason": "no pending tasks or slot not idle",
                    }))),
                    Err(e) => Ok(CallToolResult::error(e.to_string())),
                }
            }
        })
        .build()
}

/// **pool_submit_with_review** — Fire task requiring coordinator approval before completion
///
/// # Description
/// Submit a task that requires human/coordinator approval after execution.
/// When the task completes, it waits in 'pending_review' state until `pool_approve_result` or `pool_reject_result` is called.
///
/// # Parameters
/// - **prompt** (string, required) — The prompt/task to execute
/// - **model** (string, optional) — Model override
/// - **effort** (string, optional) — Effort override
/// - **tags** (array, optional) — Tags for grouping
/// - **max_rejections** (number, optional) — Max rejections before failing (default: 3)
/// - **mcp_servers** (object, optional) — Additional MCP servers
///
/// # Response Format
/// ```json
/// {
///   "task_id": "task-review-abc123",
///   "review_required": true
/// }
/// ```
///
/// # Example
/// Coordinator submitting code change for review:
/// ```text
/// await client.call_tool("pool_submit_with_review", {
///   "prompt": "Refactor this function",
///   "max_rejections": 2
/// })
/// // Returns: { task_id: "task-review-abc123", review_required: true }
/// ```
///
/// # Error Cases
/// - **InvalidInput** — Missing prompt field
pub fn pool_submit_with_review_tool<S: PoolStore + 'static>(state: Arc<State<S>>) -> Tool {
    ToolBuilder::new("pool_submit_with_review")
        .title("Fire a Task with Review Gate")
        .description(
            "Fire off a task that requires coordinator approval before completion. \
             When the task finishes, it enters 'pending_review' state instead of 'completed'. \
             Use pool_approve_result to accept or pool_reject_result to reject with feedback.",
        )
        .handler(move |input: SubmitWithReviewInput| {
            let state = Arc::clone(&state);
            async move {
                let config = task_config_from(
                    input.model,
                    input.effort,
                    input.mcp_servers,
                    input.json_schema,
                    input.disallowed_tools,
                    input.tools,
                    input.max_budget_usd,
                );
                let tags = input.tags.unwrap_or_default();
                match state
                    .pool
                    .submit_with_review(&input.prompt, config, tags, input.max_rejections)
                    .await
                {
                    Ok(task_id) => Ok(CallToolResult::json(
                        serde_json::json!({ "task_id": task_id.0, "review_required": true }),
                    )),
                    Err(e) => Ok(CallToolResult::error(e.to_string())),
                }
            }
        })
        .build()
}

/// **pool_approve_result** — Approve pending review task, mark as completed
///
/// # Description
/// Accept a task result that was waiting for review. Marks it as completed and finalizes.
///
/// # Parameters
/// - **task_id** (string, required) — Task ID in 'pending_review' state
///
/// # Response Format
/// ```json
/// "approved"
/// ```
///
/// # Example
/// Coordinator accepting task result:
/// ```text
/// await client.call_tool("pool_approve_result", { "task_id": "task-review-abc123" })
/// // Returns: "approved"
/// ```
///
/// # Error Cases
/// - **TaskNotFound** — Task ID doesn't exist
/// - **NotPendingReview** — Task is not in pending_review state
pub fn pool_approve_result_tool<S: PoolStore + 'static>(state: Arc<State<S>>) -> Tool {
    ToolBuilder::new("pool_approve_result")
        .title("Approve Task Result")
        .description(
            "Approve a task that is pending review. Transitions the task from \
             'pending_review' to 'completed'.",
        )
        .handler(move |input: ApproveResultInput| {
            let state = Arc::clone(&state);
            async move {
                let task_id = claude_pool::TaskId(input.task_id);
                match state.pool.approve_result(&task_id).await {
                    Ok(()) => Ok(CallToolResult::text("approved")),
                    Err(e) => Ok(CallToolResult::error(e.to_string())),
                }
            }
        })
        .build()
}

/// **pool_reject_result** — Reject task with feedback, re-queue with appended feedback
///
/// # Description
/// Reject a task result and send it back to the queue with feedback appended to the prompt.
/// If the task has been rejected `max_rejections` times, it's marked as failed instead.
///
/// # Parameters
/// - **task_id** (string, required) — Task ID in 'pending_review' state
/// - **feedback** (string, required) — Reason for rejection (appended to prompt on re-queue)
///
/// # Response Format
/// ```json
/// "rejected and re-queued"
/// ```
///
/// # Example
/// Coordinator rejecting with feedback:
/// ```text
/// await client.call_tool("pool_reject_result", {
///   "task_id": "task-review-abc123",
///   "feedback": "Output is incomplete, needs full implementation details"
/// })
/// // Returns: "rejected and re-queued"
/// ```
///
/// # Error Cases
/// - **TaskNotFound** — Task ID doesn't exist
/// - **NotPendingReview** — Task is not in pending_review state
/// - **MaxRejectionsExceeded** — Task has reached max rejection count (marked as failed)
pub fn pool_reject_result_tool<S: PoolStore + 'static>(state: Arc<State<S>>) -> Tool {
    ToolBuilder::new("pool_reject_result")
        .title("Reject Task Result")
        .description(
            "Reject a task that is pending review. The task is re-queued with the \
             original prompt plus rejection feedback appended. If the task has been \
             rejected max_rejections times, it is marked as failed.",
        )
        .handler(move |input: RejectResultInput| {
            let state = Arc::clone(&state);
            async move {
                let task_id = claude_pool::TaskId(input.task_id);
                match state.pool.reject_result(&task_id, &input.feedback).await {
                    Ok(()) => Ok(CallToolResult::text("rejected and re-queued")),
                    Err(e) => Ok(CallToolResult::error(e.to_string())),
                }
            }
        })
        .build()
}

/// **pool_session_metrics** — Get aggregated session metrics: cost, timing, model breakdown
///
/// Returns developer-focused insights for the current pool session including
/// spend tracking, task timing distributions, and per-model breakdowns.
/// Useful for answering questions like "how much did I spend today?" and
/// "how long are my tasks taking on average?".
/// Input for `pool_session_metrics`.
#[derive(Debug, Deserialize, JsonSchema)]
struct SessionMetricsInput {
    /// Only include tasks created after this time (millis since epoch).
    since_ms: Option<u64>,
    /// Only include tasks created before this time (millis since epoch).
    until_ms: Option<u64>,
    /// Only include tasks that ran on this model.
    model: Option<String>,
    /// Only include tasks with this tag.
    tag: Option<String>,
}

pub fn pool_session_metrics_tool<S: PoolStore + 'static>(state: Arc<State<S>>) -> Tool {
    ToolBuilder::new("pool_session_metrics")
        .title("Session Metrics")
        .description(
            "Get aggregated session metrics: spend, timing, model breakdown, task counts. \
             All parameters are optional filters.",
        )
        .read_only()
        .handler(move |input: SessionMetricsInput| {
            let state = Arc::clone(&state);
            async move {
                let filter = claude_pool::types::MetricsFilter {
                    since_ms: input.since_ms,
                    until_ms: input.until_ms,
                    model: input.model,
                    tags: input.tag.map(|t| vec![t]),
                };
                match state.pool.session_metrics(&filter).await {
                    Ok(metrics) => Ok(CallToolResult::text(
                        serde_json::to_string_pretty(&metrics).unwrap(),
                    )),
                    Err(e) => Ok(CallToolResult::error(e.to_string())),
                }
            }
        })
        .build()
}

pub fn all_tools<S: PoolStore + 'static>(state: &Arc<State<S>>) -> Vec<Tool> {
    vec![
        pool_status_tool(Arc::clone(state)),
        pool_session_metrics_tool(Arc::clone(state)),
        pool_run_tool(Arc::clone(state)),
        pool_submit_tool(Arc::clone(state)),
        pool_result_tool(Arc::clone(state)),
        pool_cancel_tool(Arc::clone(state)),
        pool_fan_out_tool(Arc::clone(state)),
        pool_drain_tool(Arc::clone(state)),
        pool_skill_run_tool(Arc::clone(state)),
        pool_chain_tool(Arc::clone(state)),
        pool_submit_chain_tool(Arc::clone(state)),
        pool_fan_out_chains_tool(Arc::clone(state)),
        pool_chain_result_tool(Arc::clone(state)),
        pool_cancel_chain_tool(Arc::clone(state)),
        pool_invoke_workflow_tool(Arc::clone(state)),
        pool_scale_up_tool(Arc::clone(state)),
        pool_scale_down_tool(Arc::clone(state)),
        pool_set_target_slots_tool(Arc::clone(state)),
        context_set_tool(Arc::clone(state)),
        context_get_tool(Arc::clone(state)),
        context_delete_tool(Arc::clone(state)),
        context_list_tool(Arc::clone(state)),
        pool_send_message_tool(Arc::clone(state)),
        pool_read_messages_tool(Arc::clone(state)),
        pool_peek_messages_tool(Arc::clone(state)),
        pool_broadcast_tool(Arc::clone(state)),
        pool_find_slots_tool(Arc::clone(state)),
        pool_configure_slot_tool(Arc::clone(state)),
        pool_skill_list_tool(Arc::clone(state)),
        pool_skill_get_tool(Arc::clone(state)),
        pool_skill_add_tool(Arc::clone(state)),
        pool_skill_remove_tool(Arc::clone(state)),
        pool_skill_save_tool(Arc::clone(state)),
        pool_skill_eject_tool(Arc::clone(state)),
        pool_claim_tool(Arc::clone(state)),
        pool_submit_with_review_tool(Arc::clone(state)),
        pool_approve_result_tool(Arc::clone(state)),
        pool_reject_result_tool(Arc::clone(state)),
    ]
}
