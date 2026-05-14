//! Slash-command-as-tool family for the chat surface.
//!
//! Claude's interactive CLI exposes commands like `/compact`,
//! `/clear`, `/model` that the user types mid-conversation. In
//! stream-json input mode the same `/foo args` strings are passed
//! through as turns -- the CLI interprets them, the model doesn't
//! see them as user prompts. We can therefore expose them as
//! discrete MCP tools rather than asking the model to remember the
//! `/` prefix and command names.
//!
//! Each tool fires the slash command via the same async-turn
//! machinery as `chat_send` -- registers a turn in the registry,
//! spawns a worker that acquires the Conversation mutex and writes
//! the slash string. Returns immediately with `{ turn_id }`.
//!
//! Today: `chat_compact`. Other commands (`chat_clear`,
//! `chat_model_set`, ...) follow once we've validated the pattern
//! works through stream-json.

use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::json;
use tower_mcp::{CallToolResult, Tool, ToolBuilder};

use crate::state::ServerState;

/// Build the slash-command tool list.
pub(crate) fn tools(state: &ServerState) -> Vec<Tool> {
    vec![tool_chat_compact(state)]
}

// -- chat_compact ---------------------------------------------------

#[derive(Debug, Deserialize, JsonSchema)]
struct ChatCompactInput {
    /// Identifier returned by `chat_open`.
    chat_id: String,
    /// Optional instructions appended to `/compact`. Mirrors the CLI:
    /// `/compact focus on auth bug` becomes the prompt sent. Omit for
    /// a default compaction.
    #[serde(default)]
    instructions: Option<String>,
}

fn tool_chat_compact(state: &ServerState) -> Tool {
    let state = state.clone();
    ToolBuilder::new("chat_compact")
        .description(
            "Fire `/compact` against an open chat to compact the conversation \
             history. Returns immediately with a turn_id; poll with `turn_get` \
             or block with `turn_wait`. Optional `instructions` are passed \
             through to the CLI's compaction prompt.",
        )
        .handler(move |input: ChatCompactInput| {
            let state = state.clone();
            async move {
                fire_slash(
                    &state,
                    &input.chat_id,
                    build_compact_prompt(input.instructions),
                )
                .await
            }
        })
        .build()
}

fn build_compact_prompt(instructions: Option<String>) -> String {
    match instructions {
        Some(s) if !s.trim().is_empty() => format!("/compact {}", s.trim()),
        _ => "/compact".to_string(),
    }
}

/// Common fire-a-slash-command machinery. Same shape as the async
/// `chat_send` body -- validate chat exists, register a turn, spawn
/// a worker that acquires the Conversation mutex and writes the
/// slash string as the turn's prompt.
async fn fire_slash(
    state: &ServerState,
    chat_id: &str,
    prompt: String,
) -> Result<CallToolResult, tower_mcp::Error> {
    if state.get_chat(chat_id).await.is_none() {
        return Err(tower_mcp::Error::internal(format!(
            "no chat with id `{chat_id}` (was it closed?)"
        )));
    }
    let handle = state.turns.register(Some(chat_id.to_string())).await;
    let turn_id = handle.turn_id.clone();

    let state_for_worker = state.clone();
    let chat_id_for_worker = chat_id.to_string();
    tokio::spawn(async move {
        if handle.is_cancelled() {
            handle.cancelled();
            return;
        }
        let conv = match state_for_worker.get_chat(&chat_id_for_worker).await {
            Some(c) => c,
            None => {
                handle.fail(format!(
                    "no chat with id `{chat_id_for_worker}` (was it closed?)"
                ));
                return;
            }
        };
        let mut guard = conv.lock().await;
        if handle.is_cancelled() {
            handle.cancelled();
            return;
        }
        match guard.send(prompt).await {
            Ok(turn) => {
                handle.complete(json!({
                    "result": turn.result_text(),
                    "session_id": turn.session_id(),
                    "turn_cost_usd": turn.total_cost_usd(),
                    "duration_ms": turn.duration_ms(),
                    "cumulative_cost_usd": guard.total_cost_usd(),
                    "total_turns": guard.total_turns(),
                }));
            }
            Err(e) => handle.fail(e),
        }
    });

    Ok(CallToolResult::json(json!({
        "turn_id": turn_id,
        "chat_id": chat_id,
    })))
}

#[cfg(test)]
mod tests {
    use super::build_compact_prompt;

    #[test]
    fn compact_prompt_without_instructions() {
        assert_eq!(build_compact_prompt(None), "/compact");
        assert_eq!(build_compact_prompt(Some(String::new())), "/compact");
        assert_eq!(build_compact_prompt(Some("   ".into())), "/compact");
    }

    #[test]
    fn compact_prompt_with_instructions() {
        assert_eq!(
            build_compact_prompt(Some("focus on auth".into())),
            "/compact focus on auth"
        );
        assert_eq!(
            build_compact_prompt(Some("  trim whitespace  ".into())),
            "/compact trim whitespace"
        );
    }
}
