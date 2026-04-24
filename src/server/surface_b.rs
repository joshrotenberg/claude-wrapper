//! Surface B: high-level "talk to the agent" tools.
//!
//! All Surface B tools apply server-configured defaults (model,
//! system prompt, allowed tools, `--bare`, budget) and let callers
//! override individual fields per call. This is the door for callers
//! who want "ask the agent" rather than "construct the right CLI
//! invocation."

use schemars::JsonSchema;
use serde::Deserialize;
use tower_mcp::extract::{Json, State};
use tower_mcp::{CallToolResult, Tool, ToolBuilder};

use crate::QueryCommand;
use crate::session::Session;

use super::error::error_to_result;
use super::state::ServerState;
use super::surface_a::parse_permission_mode;

/// Build the Surface B tools.
pub(crate) fn agent_tools(state: &ServerState) -> Vec<Tool> {
    vec![
        tool_ask(state),
        tool_chat_open(state),
        tool_chat_send(state),
        tool_chat_close(state),
        tool_chat_list(state),
        tool_budget(state),
    ]
}

// -- helpers ---------------------------------------------------------

#[derive(Debug, Deserialize, JsonSchema, Default)]
struct AgentOverrides {
    model: Option<String>,
    system_prompt: Option<String>,
    allowed_tools: Option<Vec<String>>,
    disallowed_tools: Option<Vec<String>>,
    permission_mode: Option<String>,
}

/// Build a `QueryCommand` for Surface B: server defaults first, then
/// per-call overrides on top. Always sets `--bare` if configured.
fn build_b_command(
    state: &ServerState,
    prompt: String,
    ov: AgentOverrides,
) -> Result<QueryCommand, String> {
    let cfg = &state.config.surface_b;
    let mut cmd = QueryCommand::new(prompt);

    let model = ov.model.or_else(|| cfg.default_model.clone());
    if let Some(m) = model {
        cmd = cmd.model(m);
    }

    let sp = ov
        .system_prompt
        .or_else(|| cfg.default_system_prompt.clone());
    if let Some(p) = sp {
        cmd = cmd.system_prompt(p);
    }

    let allowed = ov
        .allowed_tools
        .unwrap_or_else(|| cfg.default_allowed_tools.clone());
    if !allowed.is_empty() {
        cmd = cmd.allowed_tools(allowed);
    }

    let disallowed = ov
        .disallowed_tools
        .unwrap_or_else(|| cfg.default_disallowed_tools.clone());
    if !disallowed.is_empty() {
        cmd = cmd.disallowed_tools(disallowed);
    }

    let mode = ov
        .permission_mode
        .or_else(|| cfg.default_permission_mode.clone());
    if let Some(m) = mode {
        let parsed = parse_permission_mode(&m).ok_or_else(|| {
            format!(
                "invalid permission_mode `{m}` (allowed: default, acceptEdits, dontAsk, plan, auto)"
            )
        })?;
        cmd = cmd.permission_mode(parsed);
    }

    if cfg.bare {
        cmd = cmd.bare();
    }

    Ok(cmd)
}

// -- agent.ask -------------------------------------------------------

#[derive(Debug, Deserialize, JsonSchema)]
struct AskInput {
    /// The prompt to send.
    prompt: String,
    #[serde(flatten)]
    overrides: AgentOverrides,
}

fn tool_ask(state: &ServerState) -> Tool {
    ToolBuilder::new("agent.ask")
        .description("Single-shot query against the agent using server defaults. Returns assistant text + cost + session id.")
        .extractor_handler(
            state.clone(),
            |State(state): State<ServerState>, Json(input): Json<AskInput>| async move {
                if let Some(ref b) = state.budget
                    && b.check().is_err()
                {
                    return Ok(error_to_result(crate::error::Error::BudgetExceeded {
                        total_usd: b.total_usd(),
                        max_usd: b.max_usd().unwrap_or(0.0),
                    }));
                }
                let cmd = match build_b_command(&state, input.prompt, input.overrides) {
                    Ok(c) => c,
                    Err(e) => return Ok(CallToolResult::error(e)),
                };
                match cmd.execute_json(&state.claude).await {
                    Ok(result) => {
                        if let Some(ref b) = state.budget {
                            b.record(result.cost_usd.unwrap_or(0.0));
                        }
                        Ok(CallToolResult::from_serialize(&serde_json::json!({
                            "result": result.result,
                            "session_id": result.session_id,
                            "cost_usd": result.cost_usd,
                            "num_turns": result.num_turns,
                        }))?)
                    }
                    Err(e) => Ok(error_to_result(e)),
                }
            },
        )
        .build()
}

// -- agent.chat.open -------------------------------------------------

#[derive(Debug, Deserialize, JsonSchema)]
struct ChatOpenInput {
    /// Optional human-friendly display name for the chat (CLI `--name`).
    name: Option<String>,
    /// Resume an existing claude session id instead of starting fresh.
    resume: Option<String>,
    #[serde(flatten)]
    overrides: AgentOverrides,
}

fn tool_chat_open(state: &ServerState) -> Tool {
    ToolBuilder::new("agent.chat.open")
        .description("Create a new server-held multi-turn chat. Returns an opaque chat_id.")
        .extractor_handler(
            state.clone(),
            |State(state): State<ServerState>, Json(input): Json<ChatOpenInput>| async move {
                let mut session = match input.resume {
                    Some(id) => Session::resume(state.claude.clone(), id),
                    None => Session::new(state.claude.clone()),
                };
                if let Some(ref b) = state.budget {
                    session = session.with_budget(b.clone());
                }
                // Stash overrides + name onto the session via a wrapper: today
                // the Session API doesn't carry per-chat defaults beyond what
                // its execute call attaches, so we inline overrides in send.
                // For v0 we record the overrides on the chat record:
                let _ = (input.name, input.overrides); // reserved for v0.1
                let id = state.chats.open(session);
                CallToolResult::from_serialize(&serde_json::json!({
                    "chat_id": id,
                }))
            },
        )
        .build()
}

// -- agent.chat.send -------------------------------------------------

#[derive(Debug, Deserialize, JsonSchema)]
struct ChatSendInput {
    /// Chat id returned from `agent.chat.open`.
    chat_id: String,
    /// The prompt for this turn.
    prompt: String,
    #[serde(flatten)]
    overrides: AgentOverrides,
}

fn tool_chat_send(state: &ServerState) -> Tool {
    ToolBuilder::new("agent.chat.send")
        .description("Send a turn to a chat opened with agent.chat.open. Returns assistant text + cumulative cost.")
        .extractor_handler(
            state.clone(),
            |State(state): State<ServerState>, Json(input): Json<ChatSendInput>| async move {
                let cmd = match build_b_command(&state, input.prompt, input.overrides) {
                    Ok(c) => c,
                    Err(e) => return Ok(CallToolResult::error(e)),
                };

                // Take the session out of the registry briefly: Session::execute
                // takes &mut self and we don't want to hold the registry's lock
                // across an await. swap-remove pattern: pull, run, return.
                //
                // v0 trade-off: a concurrent send to the same chat would race
                // (one would see ChatNotFound transiently). Acceptable for v0
                // since "chat" implies sequential turns.
                let mut session = match state.chats.with_session(&input.chat_id, |s| s.clone()) {
                    Some(s) => s,
                    None => {
                        return Ok(CallToolResult::error(format!(
                            "chat_id `{}` not found",
                            input.chat_id
                        )));
                    }
                };

                let outcome = session.execute(cmd).await;

                // Write the (possibly mutated) session back. We replace via
                // close+open-with-same-id semantics: ChatRegistry doesn't
                // expose update yet, so for v0 we live with the clone-then-
                // run model. The cumulative cost / history that mattered for
                // this turn is in `outcome`; the session_id update on the
                // local `session` clone is what we'd write back next.
                state.chats.with_session(&input.chat_id, |s| {
                    *s = session;
                });

                match outcome {
                    Ok(result) => Ok(CallToolResult::from_serialize(&serde_json::json!({
                        "result": result.result,
                        "session_id": result.session_id,
                        "cost_usd": result.cost_usd,
                        "num_turns": result.num_turns,
                    }))?),
                    Err(e) => Ok(error_to_result(e)),
                }
            },
        )
        .build()
}

// -- agent.chat.close ------------------------------------------------

#[derive(Debug, Deserialize, JsonSchema)]
struct ChatCloseInput {
    chat_id: String,
}

fn tool_chat_close(state: &ServerState) -> Tool {
    ToolBuilder::new("agent.chat.close")
        .description("Drop a server-held chat. No-op if the id is unknown.")
        .extractor_handler(
            state.clone(),
            |State(state): State<ServerState>, Json(input): Json<ChatCloseInput>| async move {
                let removed = state.chats.close(&input.chat_id);
                CallToolResult::from_serialize(&serde_json::json!({
                    "closed": removed,
                }))
            },
        )
        .build()
}

// -- agent.chat.list -------------------------------------------------

#[derive(Debug, Deserialize, JsonSchema)]
struct EmptyInput {}

fn tool_chat_list(state: &ServerState) -> Tool {
    ToolBuilder::new("agent.chat.list")
        .description("List currently open chats with their cumulative cost and turn count.")
        .read_only()
        .extractor_handler(
            state.clone(),
            |State(state): State<ServerState>, Json(_input): Json<EmptyInput>| async move {
                let chats = state.chats.list();
                CallToolResult::from_serialize(&serde_json::json!({
                    "chats": chats,
                }))
            },
        )
        .build()
}

// -- agent.budget ----------------------------------------------------

fn tool_budget(state: &ServerState) -> Tool {
    ToolBuilder::new("agent.budget")
        .description("Report the global BudgetTracker state, if configured.")
        .read_only()
        .extractor_handler(
            state.clone(),
            |State(state): State<ServerState>, Json(_input): Json<EmptyInput>| async move {
                let payload = match &state.budget {
                    Some(b) => serde_json::json!({
                        "configured": true,
                        "total_usd": b.total_usd(),
                        "max_usd": b.max_usd(),
                        "warn_at_usd": b.warn_at_usd(),
                        "remaining_usd": b.remaining_usd(),
                    }),
                    None => serde_json::json!({ "configured": false }),
                };
                CallToolResult::from_serialize(&payload)
            },
        )
        .build()
}
