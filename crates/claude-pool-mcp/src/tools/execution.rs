//! Task execution tools: run, submit, result, cancel, fan_out.

use std::sync::Arc;

use claude_pool::types::{Effort, TaskId, TaskOverrides};
use schemars::JsonSchema;
use serde::Deserialize;
use tower_mcp::ToolBuilder;
use tower_mcp::protocol::CallToolResult;
use tower_mcp::tool::Tool;

use super::{PoolState, json_result};

#[derive(Debug, Deserialize, JsonSchema)]
struct RunInput {
    /// The prompt/task to execute.
    prompt: String,
    /// Model override for this task.
    model: Option<String>,
    /// Effort override (low, medium, high, max).
    effort: Option<String>,
    /// Maximum budget cap for this task in USD.
    max_budget_usd: Option<f64>,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct SubmitInput {
    /// The prompt/task to execute.
    prompt: String,
    /// Model override for this task.
    model: Option<String>,
    /// Effort override (low, medium, high, max).
    effort: Option<String>,
    /// Tags for grouping/filtering.
    tags: Option<Vec<String>>,
    /// Maximum budget cap for this task in USD.
    max_budget_usd: Option<f64>,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct TaskIdInput {
    /// The task ID.
    task_id: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct FanOutInput {
    /// Prompts to execute in parallel (2+).
    prompts: Vec<String>,
}

fn parse_effort(s: &str) -> Option<Effort> {
    match s {
        "low" => Some(Effort::Low),
        "medium" => Some(Effort::Medium),
        "high" => Some(Effort::High),
        "max" => Some(Effort::Max),
        _ => None,
    }
}

pub(super) fn tools(state: PoolState) -> Vec<Tool> {
    vec![
        pool_run(Arc::clone(&state)),
        pool_submit(Arc::clone(&state)),
        pool_result(Arc::clone(&state)),
        pool_cancel(Arc::clone(&state)),
        pool_fan_out(state),
    ]
}

fn pool_run(state: PoolState) -> Tool {
    ToolBuilder::new("pool_run")
        .title("Run Task")
        .description(
            "Run a task synchronously, block until result. Use for single bounded operations.",
        )
        .handler(move |input: RunInput| {
            let state = Arc::clone(&state);
            async move {
                let overrides = TaskOverrides {
                    model: input.model.clone(),
                    effort: input.effort.as_deref().and_then(parse_effort),
                    max_budget_usd: input.max_budget_usd,
                    ..Default::default()
                };
                match state.run(&input.prompt).config(overrides).await {
                    Ok(result) => Ok(json_result(&result)),
                    Err(e) => Ok(CallToolResult::error(format!("{e}"))),
                }
            }
        })
        .build()
}

fn pool_submit(state: PoolState) -> Tool {
    ToolBuilder::new("pool_submit")
        .title("Submit Task")
        .description(
            "Submit a task for async execution. Returns task_id immediately. Check with pool_result.",
        )
        .handler(move |input: SubmitInput| {
            let state = Arc::clone(&state);
            async move {
                let overrides = TaskOverrides {
                    model: input.model,
                    effort: input.effort.as_deref().and_then(parse_effort),
                    max_budget_usd: input.max_budget_usd,
                    ..Default::default()
                };
                let tags = input.tags.unwrap_or_default();
                match state
                    .submit_with_config(&input.prompt, Some(overrides), tags)
                    .await
                {
                    Ok(task_id) => Ok(json_result(&serde_json::json!({ "task_id": task_id }))),
                    Err(e) => Ok(CallToolResult::error(format!("{e}"))),
                }
            }
        })
        .build()
}

fn pool_result(state: PoolState) -> Tool {
    ToolBuilder::new("pool_result")
        .title("Get Task Result")
        .description("Check on an async task. Returns result if complete, null if still running.")
        .read_only_safe()
        .handler(move |input: TaskIdInput| {
            let state = Arc::clone(&state);
            async move {
                let tid = TaskId(input.task_id);
                match state.result(&tid).await {
                    Ok(Some(result)) => Ok(json_result(&result)),
                    Ok(None) => Ok(CallToolResult::json(serde_json::Value::Null)),
                    Err(e) => Ok(CallToolResult::error(format!("{e}"))),
                }
            }
        })
        .build()
}

fn pool_cancel(state: PoolState) -> Tool {
    ToolBuilder::new("pool_cancel")
        .title("Cancel Task")
        .description("Cancel a pending or running task.")
        .idempotent()
        .handler(move |input: TaskIdInput| {
            let state = Arc::clone(&state);
            async move {
                let tid = TaskId(input.task_id);
                match state.cancel(&tid).await {
                    Ok(()) => Ok(CallToolResult::json(
                        serde_json::json!({ "cancelled": true }),
                    )),
                    Err(e) => Ok(CallToolResult::error(format!("{e}"))),
                }
            }
        })
        .build()
}

fn pool_fan_out(state: PoolState) -> Tool {
    ToolBuilder::new("pool_fan_out")
        .title("Fan Out")
        .description("Run N independent prompts in parallel across slots. Returns all results.")
        .handler(move |input: FanOutInput| {
            let state = Arc::clone(&state);
            async move {
                let refs: Vec<&str> = input.prompts.iter().map(|s| s.as_str()).collect();
                match state.fan_out(&refs).await {
                    Ok(results) => Ok(json_result(&results)),
                    Err(e) => Ok(CallToolResult::error(format!("{e}"))),
                }
            }
        })
        .build()
}
