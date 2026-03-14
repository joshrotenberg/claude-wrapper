//! Messaging tools: send_message, broadcast, read_messages, peek_messages.

use std::sync::Arc;

use claude_pool::types::SlotId;
use schemars::JsonSchema;
use serde::Deserialize;
use tower_mcp::ToolBuilder;
use tower_mcp::protocol::CallToolResult;
use tower_mcp::tool::Tool;

use super::{PoolState, json_result};

#[derive(Debug, Deserialize, JsonSchema)]
struct MessageInput {
    /// Sender slot ID.
    from: String,
    /// Recipient slot ID.
    to: String,
    /// Message content.
    content: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct BroadcastInput {
    /// Sender slot ID.
    from: String,
    /// Message content.
    content: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct SlotIdInput {
    /// Slot ID.
    slot_id: String,
}

pub(super) fn tools(state: PoolState) -> Vec<Tool> {
    vec![
        pool_send_message(Arc::clone(&state)),
        pool_broadcast(Arc::clone(&state)),
        pool_read_messages(Arc::clone(&state)),
        pool_peek_messages(state),
    ]
}

fn pool_send_message(state: PoolState) -> Tool {
    ToolBuilder::new("pool_send_message")
        .title("Send Message")
        .description("Send a direct message from one slot to another.")
        .handler(move |input: MessageInput| {
            let state = Arc::clone(&state);
            async move {
                let from = SlotId(input.from);
                let to = SlotId(input.to);
                let msg_id = state.send_message(from, to, input.content);
                Ok(CallToolResult::json(
                    serde_json::json!({ "message_id": msg_id }),
                ))
            }
        })
        .build()
}

fn pool_broadcast(state: PoolState) -> Tool {
    ToolBuilder::new("pool_broadcast")
        .title("Broadcast Message")
        .description("Broadcast a message from one slot to all other active slots.")
        .handler(move |input: BroadcastInput| {
            let state = Arc::clone(&state);
            async move {
                let from = SlotId(input.from);
                match state.broadcast_message(from, input.content).await {
                    Ok(ids) => Ok(CallToolResult::json(
                        serde_json::json!({ "message_ids": ids }),
                    )),
                    Err(e) => Ok(CallToolResult::error(format!("{e}"))),
                }
            }
        })
        .build()
}

fn pool_read_messages(state: PoolState) -> Tool {
    ToolBuilder::new("pool_read_messages")
        .title("Read Messages")
        .description("Drain and read all messages for a slot (removes from inbox).")
        .handler(move |input: SlotIdInput| {
            let state = Arc::clone(&state);
            async move {
                let sid = SlotId(input.slot_id);
                let messages = state.read_messages(&sid);
                Ok(json_result(&serde_json::json!({ "messages": messages })))
            }
        })
        .build()
}

fn pool_peek_messages(state: PoolState) -> Tool {
    ToolBuilder::new("pool_peek_messages")
        .title("Peek Messages")
        .description("Read messages from a slot's inbox without removing them.")
        .read_only()
        .handler(move |input: SlotIdInput| {
            let state = Arc::clone(&state);
            async move {
                let sid = SlotId(input.slot_id);
                let messages = state.peek_messages(&sid);
                Ok(json_result(&serde_json::json!({ "messages": messages })))
            }
        })
        .build()
}
