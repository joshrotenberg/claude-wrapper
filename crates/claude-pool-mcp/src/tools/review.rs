//! Review gate tools: submit_with_review, approve, reject.

use std::sync::Arc;

use claude_pool::types::{TaskId, TaskOverrides};
use schemars::JsonSchema;
use serde::Deserialize;
use tower_mcp::ToolBuilder;
use tower_mcp::protocol::CallToolResult;
use tower_mcp::tool::Tool;

use super::PoolState;

#[derive(Debug, Deserialize, JsonSchema)]
struct SubmitWithReviewInput {
    /// The prompt/task to execute.
    prompt: String,
    /// Model override.
    model: Option<String>,
    /// Maximum rejections before auto-failing (default: 3).
    max_rejections: Option<u32>,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct TaskIdInput {
    /// The task ID.
    task_id: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct RejectInput {
    /// The task ID.
    task_id: String,
    /// Feedback explaining why the result was rejected.
    feedback: String,
}

pub(super) fn tools(state: PoolState) -> Vec<Tool> {
    vec![
        pool_submit_with_review(Arc::clone(&state)),
        pool_approve(Arc::clone(&state)),
        pool_reject(state),
    ]
}

fn pool_submit_with_review(state: PoolState) -> Tool {
    ToolBuilder::new("pool_submit_with_review")
        .title("Submit with Review")
        .description(
            "Submit a task that requires approval before completion. \
             Use pool_approve_result or pool_reject_result on the result.",
        )
        .handler(move |input: SubmitWithReviewInput| {
            let state = Arc::clone(&state);
            async move {
                let overrides = TaskOverrides {
                    model: input.model,
                    ..Default::default()
                };
                match state
                    .submit_with_review(
                        &input.prompt,
                        Some(overrides),
                        vec![],
                        input.max_rejections,
                    )
                    .await
                {
                    Ok(task_id) => Ok(CallToolResult::json(
                        serde_json::json!({ "task_id": task_id }),
                    )),
                    Err(e) => Ok(CallToolResult::error(format!("{e}"))),
                }
            }
        })
        .build()
}

fn pool_approve(state: PoolState) -> Tool {
    ToolBuilder::new("pool_approve_result")
        .title("Approve Result")
        .description("Approve a pending-review task result.")
        .handler(move |input: TaskIdInput| {
            let state = Arc::clone(&state);
            async move {
                let tid = TaskId(input.task_id);
                match state.approve_result(&tid).await {
                    Ok(()) => Ok(CallToolResult::json(
                        serde_json::json!({ "approved": true }),
                    )),
                    Err(e) => Ok(CallToolResult::error(format!("{e}"))),
                }
            }
        })
        .build()
}

fn pool_reject(state: PoolState) -> Tool {
    ToolBuilder::new("pool_reject_result")
        .title("Reject Result")
        .description("Reject a pending-review result with feedback. Task is re-queued.")
        .handler(move |input: RejectInput| {
            let state = Arc::clone(&state);
            async move {
                let tid = TaskId(input.task_id);
                match state.reject_result(&tid, &input.feedback).await {
                    Ok(()) => Ok(CallToolResult::json(
                        serde_json::json!({ "rejected": true }),
                    )),
                    Err(e) => Ok(CallToolResult::error(format!("{e}"))),
                }
            }
        })
        .build()
}
