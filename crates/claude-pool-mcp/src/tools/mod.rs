//! MCP tool definitions for claude-pool-mcp.
//!
//! Every tool is a thin wrapper: deserialize input, call pool method, serialize output.
//! No business logic lives here. Tools are grouped by category.

mod auto;
mod chain;
mod context;
mod execution;
mod messaging;
mod review;
mod scaling;
mod status;

use std::sync::Arc;

use claude_pool::Pool;
use claude_pool::store::InMemoryStore;
use tower_mcp::protocol::CallToolResult;
use tower_mcp::tool::Tool;

pub(crate) type PoolState = Arc<Pool<InMemoryStore>>;

/// Serialize a value to JSON and wrap in CallToolResult.
pub(crate) fn json_result(value: &impl serde::Serialize) -> CallToolResult {
    match serde_json::to_value(value) {
        Ok(v) => CallToolResult::json(v),
        Err(e) => CallToolResult::error(format!("serialization error: {e}")),
    }
}

/// Build the complete list of MCP tools.
pub fn all_tools(state: PoolState) -> Vec<Tool> {
    let mut tools = Vec::new();
    tools.extend(execution::tools(Arc::clone(&state)));
    tools.extend(auto::tools(Arc::clone(&state)));
    tools.extend(chain::tools(Arc::clone(&state)));
    tools.extend(review::tools(Arc::clone(&state)));
    tools.extend(status::tools(Arc::clone(&state)));
    tools.extend(context::tools(Arc::clone(&state)));
    tools.extend(messaging::tools(Arc::clone(&state)));
    tools.extend(scaling::tools(state));
    tools
}
