//! Scaling and lifecycle tools: scale_up, scale_down, set_target_slots, drain.

use std::sync::Arc;

use schemars::JsonSchema;
use serde::Deserialize;
use tower_mcp::ToolBuilder;
use tower_mcp::protocol::CallToolResult;
use tower_mcp::tool::Tool;

use super::{PoolState, json_result};

#[derive(Debug, Deserialize, JsonSchema)]
struct ScalingInput {
    /// Number of slots to add or remove.
    count: usize,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct SetTargetInput {
    /// Target number of slots.
    target: usize,
}

pub(super) fn tools(state: PoolState) -> Vec<Tool> {
    vec![
        pool_scale_up(Arc::clone(&state)),
        pool_scale_down(Arc::clone(&state)),
        pool_set_target(Arc::clone(&state)),
        pool_drain(state),
    ]
}

fn pool_scale_up(state: PoolState) -> Tool {
    ToolBuilder::new("pool_scale_up")
        .title("Scale Up")
        .description("Add N new slots to the pool.")
        .handler(move |input: ScalingInput| {
            let state = Arc::clone(&state);
            async move {
                match state.scale_up(input.count).await {
                    Ok(new_count) => Ok(CallToolResult::json(
                        serde_json::json!({ "total_slots": new_count }),
                    )),
                    Err(e) => Ok(CallToolResult::error(format!("{e}"))),
                }
            }
        })
        .build()
}

fn pool_scale_down(state: PoolState) -> Tool {
    ToolBuilder::new("pool_scale_down")
        .title("Scale Down")
        .description("Remove N idle slots from the pool.")
        .handler(move |input: ScalingInput| {
            let state = Arc::clone(&state);
            async move {
                match state.scale_down(input.count).await {
                    Ok(new_count) => Ok(CallToolResult::json(
                        serde_json::json!({ "total_slots": new_count }),
                    )),
                    Err(e) => Ok(CallToolResult::error(format!("{e}"))),
                }
            }
        })
        .build()
}

fn pool_set_target(state: PoolState) -> Tool {
    ToolBuilder::new("pool_set_target_slots")
        .title("Set Target Slots")
        .description("Set pool to an exact number of slots, scaling up or down as needed.")
        .idempotent()
        .handler(move |input: SetTargetInput| {
            let state = Arc::clone(&state);
            async move {
                match state.set_target_slots(input.target).await {
                    Ok(new_count) => Ok(CallToolResult::json(
                        serde_json::json!({ "total_slots": new_count }),
                    )),
                    Err(e) => Ok(CallToolResult::error(format!("{e}"))),
                }
            }
        })
        .build()
}

fn pool_drain(state: PoolState) -> Tool {
    ToolBuilder::new("pool_drain")
        .title("Drain Pool")
        .description("Gracefully shut down the pool. Waits for in-flight tasks to complete.")
        .destructive()
        .no_params_handler(move || {
            let state = Arc::clone(&state);
            async move {
                match state.drain().await {
                    Ok(summary) => Ok(json_result(&summary)),
                    Err(e) => Ok(CallToolResult::error(format!("{e}"))),
                }
            }
        })
        .build()
}
