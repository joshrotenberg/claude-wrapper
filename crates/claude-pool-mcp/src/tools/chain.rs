//! Chain execution tools: chain, submit_chain, chain_result, cancel_chain.

use std::sync::Arc;

use claude_pool::SkillRegistry;
use claude_pool::types::{TaskId, TaskOverrides};
use schemars::JsonSchema;
use serde::Deserialize;
use tower_mcp::ToolBuilder;
use tower_mcp::protocol::CallToolResult;
use tower_mcp::tool::Tool;

use super::{PoolState, json_result};

#[derive(Debug, Deserialize, JsonSchema)]
struct ChainStepInput {
    /// Step name.
    name: String,
    /// Step prompt. Use {previous_output} to reference prior step.
    prompt: String,
    /// Model override for this step.
    model: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct ChainInput {
    /// Ordered steps to execute.
    steps: Vec<ChainStepInput>,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct SubmitChainInput {
    /// Ordered steps to execute.
    steps: Vec<ChainStepInput>,
    /// Tags for grouping/filtering.
    tags: Option<Vec<String>>,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct TaskIdInput {
    /// The task ID.
    task_id: String,
}

fn chain_steps_from_input(steps: Vec<ChainStepInput>) -> Vec<claude_pool::ChainStep> {
    steps
        .into_iter()
        .map(|s| claude_pool::ChainStep {
            name: s.name,
            action: claude_pool::StepAction::Prompt { prompt: s.prompt },
            config: s.model.map(|m| TaskOverrides {
                model: Some(m),
                ..Default::default()
            }),
            failure_policy: claude_pool::StepFailurePolicy::default(),
            output_vars: Default::default(),
        })
        .collect()
}

pub(super) fn tools(state: PoolState) -> Vec<Tool> {
    vec![
        pool_chain(Arc::clone(&state)),
        pool_submit_chain(Arc::clone(&state)),
        pool_chain_result(Arc::clone(&state)),
        pool_cancel_chain(state),
    ]
}

fn pool_chain(state: PoolState) -> Tool {
    ToolBuilder::new("pool_chain")
        .title("Run Chain")
        .description(
            "Run a sequential chain of steps. Each step can reference {previous_output}. \
             Blocks until done.",
        )
        .handler(move |input: ChainInput| {
            let state = Arc::clone(&state);
            async move {
                let steps = chain_steps_from_input(input.steps);
                let skills = SkillRegistry::new();
                match claude_pool::execute_chain(&*state, &skills, &steps).await {
                    Ok(result) => Ok(json_result(&result)),
                    Err(e) => Ok(CallToolResult::error(format!("{e}"))),
                }
            }
        })
        .build()
}

fn pool_submit_chain(state: PoolState) -> Tool {
    ToolBuilder::new("pool_submit_chain")
        .title("Submit Chain")
        .description(
            "Submit a chain for async execution. Returns task_id. Check with pool_chain_result.",
        )
        .handler(move |input: SubmitChainInput| {
            let state = Arc::clone(&state);
            async move {
                let steps = chain_steps_from_input(input.steps);
                let skills = SkillRegistry::new();
                let options = claude_pool::ChainOptions {
                    tags: input.tags.unwrap_or_default(),
                    ..Default::default()
                };
                match state.submit_chain(steps, &skills, options).await {
                    Ok(task_id) => Ok(CallToolResult::json(
                        serde_json::json!({ "task_id": task_id }),
                    )),
                    Err(e) => Ok(CallToolResult::error(format!("{e}"))),
                }
            }
        })
        .build()
}

fn pool_chain_result(state: PoolState) -> Tool {
    ToolBuilder::new("pool_chain_result")
        .title("Get Chain Result")
        .description("Check on an async chain. Returns result and per-step progress.")
        .read_only_safe()
        .handler(move |input: TaskIdInput| {
            let state = Arc::clone(&state);
            async move {
                let tid = TaskId(input.task_id);
                let result = state.result(&tid).await;
                let progress = state.chain_progress(&tid);
                match result {
                    Ok(r) => Ok(CallToolResult::json(serde_json::json!({
                        "result": r,
                        "progress": progress,
                    }))),
                    Err(e) => Ok(CallToolResult::error(format!("{e}"))),
                }
            }
        })
        .build()
}

fn pool_cancel_chain(state: PoolState) -> Tool {
    ToolBuilder::new("pool_cancel_chain")
        .title("Cancel Chain")
        .description("Cancel a running chain. Current step finishes, remaining steps are skipped.")
        .idempotent()
        .handler(move |input: TaskIdInput| {
            let state = Arc::clone(&state);
            async move {
                let tid = TaskId(input.task_id);
                match state.cancel_chain(&tid).await {
                    Ok(()) => Ok(CallToolResult::json(
                        serde_json::json!({ "cancelled": true }),
                    )),
                    Err(e) => Ok(CallToolResult::error(format!("{e}"))),
                }
            }
        })
        .build()
}
