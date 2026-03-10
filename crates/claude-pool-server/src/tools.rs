//! MCP tool definitions for claude-pool.

use std::sync::Arc;

use claude_pool::PoolStore;
use claude_pool::types::WorkerConfig;
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
pub struct ConfigureWorkerInput {
    /// Worker ID to configure (e.g. "worker-0").
    pub worker_id: String,
    /// Human-readable name for the worker.
    pub name: Option<String>,
    /// Role classification for the worker.
    pub role: Option<String>,
    /// Description of the worker's purpose.
    pub description: Option<String>,
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

fn task_config_from(model: Option<String>, effort: Option<String>) -> Option<WorkerConfig> {
    if model.is_none() && effort.is_none() {
        return None;
    }
    Some(WorkerConfig {
        model,
        effort: effort.and_then(|e| parse_effort(&e)),
        ..Default::default()
    })
}

// ── Tool builders ────────────────────────────────────────────────────

pub fn pool_status_tool<S: PoolStore + 'static>(state: Arc<State<S>>) -> Tool {
    ToolBuilder::new("pool_status")
        .title("Pool Status")
        .description("Get pool status: workers, tasks in flight, budget")
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
            "Run a task synchronously on the next available worker. Blocks until completion.",
        )
        .handler(move |input: RunInput| {
            let state = Arc::clone(&state);
            async move {
                let config = task_config_from(input.model, input.effort);
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
                let config = task_config_from(input.model, input.effort);
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
            "Execute multiple tasks in parallel across available workers. Returns all results.",
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
            "Gracefully shut down the pool. Waits for in-flight tasks, then stops all workers.",
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
        .description("Set a shared context value. Context is injected into worker system prompts.")
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

pub fn pool_configure_worker_tool<S: PoolStore + 'static>(state: Arc<State<S>>) -> Tool {
    ToolBuilder::new("pool_configure_worker")
        .title("Configure Worker")
        .description("Set name/role/description for a worker to give it persistent identity")
        .handler(move |input: ConfigureWorkerInput| {
            let state = Arc::clone(&state);
            async move {
                let worker_id = claude_pool::WorkerId(input.worker_id.clone());

                match state.pool.store().get_worker(&worker_id).await {
                    Ok(Some(mut worker)) => {
                        // Update identity fields
                        if let Some(name) = input.name {
                            worker.config.name = Some(name);
                        }
                        if let Some(role) = input.role {
                            worker.config.role = Some(role);
                        }
                        if let Some(description) = input.description {
                            worker.config.description = Some(description);
                        }

                        // Persist updated worker
                        match state.pool.store().put_worker(worker.clone()).await {
                            Ok(_) => {
                                let response = serde_json::json!({
                                    "worker_id": worker_id.0,
                                    "name": worker.config.name,
                                    "role": worker.config.role,
                                    "description": worker.config.description,
                                });
                                Ok(CallToolResult::json(response))
                            }
                            Err(e) => Ok(CallToolResult::error(format!(
                                "failed to update worker: {e}"
                            ))),
                        }
                    }
                    Ok(None) => Ok(CallToolResult::error(format!(
                        "worker not found: {}",
                        input.worker_id
                    ))),
                    Err(e) => Ok(CallToolResult::error(format!(
                        "failed to fetch worker: {e}"
                    ))),
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
            let config = task_config_from(s.model, s.effort);
            let failure_policy = claude_pool::StepFailurePolicy {
                retries: s.retries.unwrap_or(0),
                recovery_prompt: s.recovery_prompt,
            };
            claude_pool::ChainStep {
                name: s.name,
                action,
                config,
                failure_policy,
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
                let skill = match state.skills.get(&input.skill) {
                    Some(s) => s.clone(),
                    None => {
                        return Ok(CallToolResult::error(format!(
                            "skill not found: {}",
                            input.skill
                        )));
                    }
                };

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
                match claude_pool::execute_chain(&state.pool, &state.skills, &steps).await {
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
                let options = claude_pool::ChainOptions {
                    tags: input.tags.unwrap_or_default(),
                };
                match state.pool.submit_chain(steps, &state.skills, options).await {
                    Ok(task_id) => Ok(CallToolResult::json(
                        serde_json::json!({ "task_id": task_id.0 }),
                    )),
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

/// Build all pool tools.
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
        pool_chain_result_tool(Arc::clone(state)),
        context_set_tool(Arc::clone(state)),
        context_get_tool(Arc::clone(state)),
        context_delete_tool(Arc::clone(state)),
        context_list_tool(Arc::clone(state)),
        pool_configure_worker_tool(Arc::clone(state)),
    ]
}
