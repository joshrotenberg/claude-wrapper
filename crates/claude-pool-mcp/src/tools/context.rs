//! Shared context tools: set, get, list, delete.

use std::sync::Arc;

use schemars::JsonSchema;
use serde::Deserialize;
use tower_mcp::ToolBuilder;
use tower_mcp::protocol::CallToolResult;
use tower_mcp::tool::Tool;

use super::{PoolState, json_result};

#[derive(Debug, Deserialize, JsonSchema)]
struct ContextSetInput {
    /// Context key.
    key: String,
    /// Context value.
    value: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct ContextKeyInput {
    /// Context key.
    key: String,
}

pub(super) fn tools(state: PoolState) -> Vec<Tool> {
    vec![
        context_set(Arc::clone(&state)),
        context_get(Arc::clone(&state)),
        context_list(Arc::clone(&state)),
        context_delete(state),
    ]
}

fn context_set(state: PoolState) -> Tool {
    ToolBuilder::new("context_set")
        .title("Set Context")
        .description("Set a shared context key-value pair.")
        .idempotent()
        .handler(move |input: ContextSetInput| {
            let state = Arc::clone(&state);
            async move {
                state.set_context(&input.key, &input.value);
                Ok(CallToolResult::json(serde_json::json!({ "set": true })))
            }
        })
        .build()
}

fn context_get(state: PoolState) -> Tool {
    ToolBuilder::new("context_get")
        .title("Get Context")
        .description("Get a shared context value by key.")
        .read_only_safe()
        .handler(move |input: ContextKeyInput| {
            let state = Arc::clone(&state);
            async move {
                match state.get_context(&input.key) {
                    Some(value) => Ok(CallToolResult::json(serde_json::json!({ "value": value }))),
                    None => Ok(CallToolResult::json(serde_json::Value::Null)),
                }
            }
        })
        .build()
}

fn context_list(state: PoolState) -> Tool {
    ToolBuilder::new("context_list")
        .title("List Context")
        .description("List all shared context keys and values.")
        .read_only_safe()
        .no_params_handler(move || {
            let state = Arc::clone(&state);
            async move {
                let entries = state.list_context();
                Ok(json_result(&serde_json::json!({ "entries": entries })))
            }
        })
        .build()
}

fn context_delete(state: PoolState) -> Tool {
    ToolBuilder::new("context_delete")
        .title("Delete Context")
        .description("Delete a shared context key.")
        .idempotent()
        .handler(move |input: ContextKeyInput| {
            let state = Arc::clone(&state);
            async move {
                let removed = state.delete_context(&input.key);
                Ok(CallToolResult::json(
                    serde_json::json!({ "deleted": removed.is_some() }),
                ))
            }
        })
        .build()
}
