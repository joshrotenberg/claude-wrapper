//! MCP tool definitions for claude-pool.

use std::path::PathBuf;
use std::sync::Arc;

use claude_pool::PoolStore;
use claude_pool::skill::{SkillScope, SkillSource};
use claude_pool::types::SlotConfig;
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
    /// Additional MCP servers for this task (merged with global/slot servers).
    /// Keys are server names, values are server config objects.
    pub mcp_servers: Option<std::collections::HashMap<String, serde_json::Value>>,
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
    /// Additional MCP servers for this task (merged with global/slot servers).
    /// Keys are server names, values are server config objects.
    pub mcp_servers: Option<std::collections::HashMap<String, serde_json::Value>>,
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
) -> Option<SlotConfig> {
    if model.is_none() && effort.is_none() && mcp_servers.is_none() {
        return None;
    }
    Some(SlotConfig {
        model,
        effort: effort.and_then(|e| parse_effort(&e)),
        mcp_servers,
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

pub fn pool_status_tool<S: PoolStore + 'static>(state: Arc<State<S>>) -> Tool {
    ToolBuilder::new("pool_status")
        .title("Pool Status")
        .description("Get pool status: slots, tasks in flight, budget")
        .read_only()
        .no_params_handler(move || {
            let state = Arc::clone(&state);
            async move {
                match state.pool.status().await {
                    Ok(status) => Ok(CallToolResult::json(serde_json::to_value(&status).unwrap())),
                    Err(e) => Ok(CallToolResult::error(e.to_string())),
                }
            }
        })
        .build()
}

pub fn pool_run_tool<S: PoolStore + 'static>(state: Arc<State<S>>) -> Tool {
    ToolBuilder::new("pool_run")
        .title("Run Task (Sync)")
        .description(
            "Run a task synchronously on the next available slot. Blocks until completion.",
        )
        .handler(move |input: RunInput| {
            let state = Arc::clone(&state);
            async move {
                let config = task_config_from(input.model, input.effort, input.mcp_servers);
                match state.pool.run_with_config(&input.prompt, config).await {
                    Ok(result) => Ok(CallToolResult::json(serde_json::to_value(&result).unwrap())),
                    Err(e) => Ok(CallToolResult::error(e.to_string())),
                }
            }
        })
        .build()
}

pub fn pool_submit_tool<S: PoolStore + 'static>(state: Arc<State<S>>) -> Tool {
    ToolBuilder::new("pool_submit")
        .title("Submit Task (Async)")
        .description("Submit a task for async execution. Returns a task_id immediately.")
        .handler(move |input: SubmitInput| {
            let state = Arc::clone(&state);
            async move {
                let config = task_config_from(input.model, input.effort, input.mcp_servers);
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

pub fn pool_result_tool<S: PoolStore + 'static>(state: Arc<State<S>>) -> Tool {
    ToolBuilder::new("pool_result")
        .title("Get Task Result")
        .description("Check/collect result for a submitted task. Returns null if still running.")
        .read_only()
        .handler(move |input: TaskIdInput| {
            let state = Arc::clone(&state);
            async move {
                let task_id = claude_pool::TaskId(input.task_id);
                match state.pool.result(&task_id).await {
                    Ok(Some(r)) => Ok(CallToolResult::json(serde_json::to_value(&r).unwrap())),
                    Ok(None) => Ok(CallToolResult::json(
                        serde_json::json!({ "status": "running" }),
                    )),
                    Err(e) => Ok(CallToolResult::error(e.to_string())),
                }
            }
        })
        .build()
}

pub fn pool_cancel_tool<S: PoolStore + 'static>(state: Arc<State<S>>) -> Tool {
    ToolBuilder::new("pool_cancel")
        .title("Cancel Task")
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

pub fn pool_fan_out_tool<S: PoolStore + 'static>(state: Arc<State<S>>) -> Tool {
    ToolBuilder::new("pool_fan_out")
        .title("Fan Out (Parallel)")
        .description(
            "Execute multiple tasks in parallel across available slots. Returns all results.",
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

pub fn pool_drain_tool<S: PoolStore + 'static>(state: Arc<State<S>>) -> Tool {
    ToolBuilder::new("pool_drain")
        .title("Drain Pool")
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
            let config = task_config_from(s.model, s.effort, None);
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

pub fn pool_skill_run_tool<S: PoolStore + 'static>(state: Arc<State<S>>) -> Tool {
    ToolBuilder::new("pool_skill_run")
        .title("Run Skill")
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

                match state.pool.run_with_config(&prompt, Some(config)).await {
                    Ok(result) => Ok(CallToolResult::json(serde_json::to_value(&result).unwrap())),
                    Err(e) => Ok(CallToolResult::error(e.to_string())),
                }
            }
        })
        .build()
}

pub fn pool_chain_tool<S: PoolStore + 'static>(state: Arc<State<S>>) -> Tool {
    ToolBuilder::new("pool_chain")
        .title("Run Chain (Sync)")
        .description(
            "Execute a sequential pipeline of steps synchronously. Each step's output feeds \
             the next. Steps can be inline prompts or skill references. Blocks until all \
             steps complete. For long chains, use pool_submit_chain instead.",
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

pub fn pool_submit_chain_tool<S: PoolStore + 'static>(state: Arc<State<S>>) -> Tool {
    ToolBuilder::new("pool_submit_chain")
        .title("Submit Chain (Async)")
        .description(
            "Submit a sequential pipeline for async execution. Returns a task_id immediately. \
             Poll with pool_chain_result for per-step progress, or pool_result for final output.",
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
        .title("Fan Out Chains (Parallel Pipelines)")
        .description(
            "Submit multiple sequential chains to run in parallel, each on its own slot. \
             Returns all task IDs for individual progress tracking via pool_chain_result.",
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

pub fn pool_chain_result_tool<S: PoolStore + 'static>(state: Arc<State<S>>) -> Tool {
    ToolBuilder::new("pool_chain_result")
        .title("Get Chain Progress")
        .description(
            "Get per-step progress of an async chain. Shows which step is running, \
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
        .title("Cancel Chain")
        .description(
            "Cancel a running chain submitted with pool_submit_chain or pool_fan_out_chains. \
             The current step finishes before cancellation takes effect. Remaining steps are \
             skipped (marked skipped=true). Use pool_chain_result to confirm, then pool_result \
             to retrieve partial output.",
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
        .title("Scale Up Slots")
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
        .title("Scale Down Slots")
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
        .title("Set Target Slot Count")
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
        .description("List registered skills with optional scope/source filters.")
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
                        serde_json::json!({
                            "name": rs.skill.name,
                            "description": rs.skill.description,
                            "scope": rs.skill.scope.to_string(),
                            "source": rs.source.to_string(),
                        })
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
        .title("Add Skill")
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
                let config: Option<SlotConfig> =
                    input.config.and_then(|v| serde_json::from_value(v).ok());
                let skill = claude_pool::Skill {
                    name: input.name.clone(),
                    description: input.description,
                    prompt: input.prompt,
                    arguments,
                    config,
                    scope,
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
        .title("Remove Skill")
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

/// Persist a skill to the project skills directory as JSON.
pub fn pool_skill_save_tool<S: PoolStore + 'static>(state: Arc<State<S>>) -> Tool {
    ToolBuilder::new("pool_skill_save")
        .title("Save Skill to Disk")
        .description(
            "Persist a skill to the project skills directory as JSON. \
             Creates/overwrites {dir}/{name}.json.",
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

                let dir = input
                    .dir
                    .map(PathBuf::from)
                    .unwrap_or_else(|| state.skills_dir.clone());

                if let Err(e) = std::fs::create_dir_all(&dir) {
                    return Ok(CallToolResult::error(format!(
                        "failed to create directory {}: {e}",
                        dir.display()
                    )));
                }

                let path = dir.join(format!("{}.json", input.name));
                let json = match serde_json::to_string_pretty(&skill) {
                    Ok(j) => j,
                    Err(e) => return Ok(CallToolResult::error(format!("serialize error: {e}"))),
                };

                if let Err(e) = std::fs::write(&path, &json) {
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
                })))
            }
        })
        .build()
}

pub fn all_tools<S: PoolStore + 'static>(state: &Arc<State<S>>) -> Vec<Tool> {
    vec![
        pool_status_tool(Arc::clone(state)),
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
        pool_configure_slot_tool(Arc::clone(state)),
        pool_skill_list_tool(Arc::clone(state)),
        pool_skill_get_tool(Arc::clone(state)),
        pool_skill_add_tool(Arc::clone(state)),
        pool_skill_remove_tool(Arc::clone(state)),
        pool_skill_save_tool(Arc::clone(state)),
    ]
}
