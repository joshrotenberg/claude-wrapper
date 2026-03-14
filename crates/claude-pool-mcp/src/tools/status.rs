//! Status and inspection tools: status, metrics, list_tasks, find_slots.

use std::sync::Arc;

use claude_pool::PoolStore;
use claude_pool::types::{MetricsFilter, SlotState, TaskFilter};
use schemars::JsonSchema;
use serde::Deserialize;
use tower_mcp::ToolBuilder;
use tower_mcp::protocol::CallToolResult;
use tower_mcp::tool::Tool;

use super::{PoolState, json_result};

#[derive(Debug, Deserialize, JsonSchema)]
struct FindSlotsInput {
    /// Filter by slot name.
    name: Option<String>,
    /// Filter by role.
    role: Option<String>,
    /// Filter by state (idle, busy, stopped, errored).
    state: Option<String>,
}

fn parse_slot_state(s: &str) -> Option<SlotState> {
    match s {
        "idle" => Some(SlotState::Idle),
        "busy" => Some(SlotState::Busy),
        "stopped" => Some(SlotState::Stopped),
        "errored" => Some(SlotState::Errored),
        _ => None,
    }
}

pub(super) fn tools(state: PoolState) -> Vec<Tool> {
    vec![
        pool_status(Arc::clone(&state)),
        pool_metrics(Arc::clone(&state)),
        pool_tasks(Arc::clone(&state)),
        pool_slots(state),
    ]
}

fn pool_status(state: PoolState) -> Tool {
    ToolBuilder::new("pool_status")
        .title("Pool Status")
        .description("Pool snapshot: slots, tasks, spend, budget remaining.")
        .read_only_safe()
        .no_params_handler(move || {
            let state = Arc::clone(&state);
            async move {
                match state.status().await {
                    Ok(status) => Ok(json_result(&status)),
                    Err(e) => Ok(CallToolResult::error(format!("{e}"))),
                }
            }
        })
        .build()
}

fn pool_metrics(state: PoolState) -> Tool {
    ToolBuilder::new("pool_session_metrics")
        .title("Session Metrics")
        .description("Aggregated session metrics: total cost, timing, model breakdown.")
        .read_only_safe()
        .no_params_handler(move || {
            let state = Arc::clone(&state);
            async move {
                let filter = MetricsFilter::default();
                match state.session_metrics(&filter).await {
                    Ok(metrics) => Ok(json_result(&metrics)),
                    Err(e) => Ok(CallToolResult::error(format!("{e}"))),
                }
            }
        })
        .build()
}

fn pool_tasks(state: PoolState) -> Tool {
    ToolBuilder::new("pool_list_tasks")
        .title("List Tasks")
        .description("List tasks. Returns all tasks in the pool store.")
        .read_only_safe()
        .no_params_handler(move || {
            let state = Arc::clone(&state);
            async move {
                let filter = TaskFilter::default();
                match state.store().list_tasks(&filter).await {
                    Ok(tasks) => Ok(json_result(&serde_json::json!({ "tasks": tasks }))),
                    Err(e) => Ok(CallToolResult::error(format!("{e}"))),
                }
            }
        })
        .build()
}

fn pool_slots(state: PoolState) -> Tool {
    ToolBuilder::new("pool_find_slots")
        .title("Find Slots")
        .description("List slots with optional filters (name, role, state).")
        .read_only_safe()
        .handler(move |input: FindSlotsInput| {
            let state = Arc::clone(&state);
            async move {
                let slot_state = input.state.as_deref().and_then(parse_slot_state);
                match state
                    .find_slots(input.name.as_deref(), input.role.as_deref(), slot_state)
                    .await
                {
                    Ok(slots) => Ok(json_result(&serde_json::json!({ "slots": slots }))),
                    Err(e) => Ok(CallToolResult::error(format!("{e}"))),
                }
            }
        })
        .build()
}
