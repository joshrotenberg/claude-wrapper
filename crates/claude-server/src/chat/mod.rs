//! L2.5 chat surface: tools that wrap [`DuplexSession`] +
//! [`Conversation`] for multi-turn work.
//!
//! Single-shot prompts go through [`crate::cli::tool_query`]
//! (`claude_query`). For anything that needs accumulated context
//! across turns -- coordinator/SME flows, interactive UIs, anything
//! mid-turn-interruptible -- open a chat, send turns against it,
//! close it when you're done.
//!
//! The wrapper provides:
//! - [`DuplexSession`] -- the long-lived stream-json subprocess.
//! - [`Conversation`] -- host-side accounting (history, cost,
//!   optional budget) on top.
//!
//! We hold one [`Conversation`] per server-side chat, behind a
//! [`tokio::sync::Mutex`] so turns within a chat are serialized.

use std::sync::Arc;

use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::json;
#[cfg(feature = "sync-agent-turns")]
use tower_mcp::extract::{Context, Json, State};
use tower_mcp::{CallToolResult, Tool, ToolBuilder};
use tracing::Instrument;

use claude_wrapper::budget::BudgetTracker;
use claude_wrapper::conversation::Conversation;
#[cfg(feature = "sync-agent-turns")]
use claude_wrapper::duplex::InboundEvent;
use claude_wrapper::duplex::{DuplexOptions, DuplexSession};

use crate::state::ServerState;

mod slash;

/// Build the L2.5 chat tool list.
pub(crate) fn tools(state: &ServerState) -> Vec<Tool> {
    let mut out = vec![
        tool_chat_open(state),
        tool_chat_send(state),
        tool_chat_list(state),
        tool_chat_history(state),
        tool_chat_interrupt(state),
        tool_chat_budget(state),
        tool_chat_close(state),
    ];
    out.extend(slash::tools(state));
    #[cfg(feature = "sync-agent-turns")]
    {
        out.push(tool_chat_send_sync(state));
        out.push(tool_chat_send_stream_sync(state));
    }
    out
}

// -- chat_open ------------------------------------------------------

#[derive(Debug, Default, Deserialize, JsonSchema)]
struct ChatOpenInput {
    /// Optional model (e.g. `sonnet`, `haiku`).
    #[serde(default)]
    model: Option<String>,
    /// Optional system prompt for the session.
    #[serde(default)]
    system_prompt: Option<String>,
    /// Optional appended system prompt.
    #[serde(default)]
    append_system_prompt: Option<String>,
    /// Hard cumulative cost ceiling in USD. When the chat's running
    /// total reaches this, further chat_send calls error before
    /// touching claude.
    #[serde(default)]
    max_cost_usd: Option<f64>,
    /// Soft warning threshold in USD. Logged via tracing when crossed
    /// (no callback wired through MCP today).
    #[serde(default)]
    warn_at_usd: Option<f64>,
    /// Run this chat in a fresh git worktree (`claude --worktree`).
    /// `true` uses a default-named worktree; pass `worktree_name`
    /// to choose a name explicitly.
    #[serde(default)]
    worktree: Option<bool>,
    /// Name for the fresh worktree (implies `worktree = true`).
    /// Useful for "agent runs in isolation" -- the chat's writes
    /// land in a side worktree instead of the current working tree.
    #[serde(default)]
    worktree_name: Option<String>,
    /// Per-chat working directory override. The spawned `claude`
    /// subprocess starts in this directory instead of the
    /// server-default `ServerConfig::claude::working_dir`.
    /// Lets a single server host chats against multiple project
    /// roots simultaneously.
    #[serde(default)]
    working_dir: Option<std::path::PathBuf>,
    /// Resume a prior session by id. Maps to `claude --resume
    /// <session_id>` on the spawned duplex process. Use case:
    /// upgrade a passive on-disk JSONL session to a live duplex
    /// chat. Subsequent `chat_send` turns extend the existing
    /// conversation rather than starting fresh.
    #[serde(default)]
    resume: Option<String>,
    /// Continue the most recent session in the resolved working
    /// directory (`claude --continue`). Mutually exclusive with
    /// `resume` at the CLI level; passing both lets the CLI decide.
    #[serde(default)]
    continue_session: Option<bool>,
}

fn tool_chat_open(state: &ServerState) -> Tool {
    let state = state.clone();
    ToolBuilder::new("chat_open")
        .description(
            "Open a long-lived chat backed by a duplex `claude` subprocess. \
             Returns a chat_id you pass to chat_send / chat_close / etc. \
             Turns within a chat are serialized; multiple chats run in parallel.",
        )
        .handler(move |input: ChatOpenInput| {
            let state = state.clone();
            async move {
                let mut opts = DuplexOptions::default();
                if let Some(m) = input.model {
                    opts = opts.model(m);
                }
                if let Some(s) = input.system_prompt {
                    opts = opts.system_prompt(s);
                }
                if let Some(s) = input.append_system_prompt {
                    opts = opts.append_system_prompt(s);
                }
                if let Some(id) = input.resume {
                    opts = opts.resume(id);
                }
                if input.continue_session.unwrap_or(false) {
                    opts = opts.continue_session();
                }
                // worktree_name implies worktree-enabled.
                let want_worktree =
                    input.worktree_name.is_some() || input.worktree.unwrap_or(false);
                if want_worktree {
                    opts = opts.worktree(input.worktree_name.as_deref());
                }

                // Per-chat working_dir override clones the Claude
                // client; without override we use the server-default.
                let claude = match input.working_dir {
                    Some(dir) => std::sync::Arc::new(state.claude.with_working_dir(dir)),
                    None => state.claude.clone(),
                };
                let session = DuplexSession::spawn(&claude, opts)
                    .await
                    .map_err(super_internal)?;
                let mut conv = Conversation::new(session);
                if let Some(max) = input.max_cost_usd {
                    let mut b = BudgetTracker::builder().max_usd(max);
                    if let Some(w) = input.warn_at_usd {
                        b = b.warn_at_usd(w);
                    }
                    conv = conv.with_budget(b.build());
                }
                let id = state.insert_chat(conv).await;
                state.notify_resources_list_changed();
                Ok(CallToolResult::json(json!({
                    "chat_id": id,
                    "max_cost_usd": input.max_cost_usd,
                })))
            }
        })
        .build()
}

// -- chat_send (async, default) -------------------------------------
//
// Fires the turn into a background task and returns immediately
// with the new turn_id. Within a chat, turns still serialize via
// the Conversation mutex -- the second chat_send to the same chat
// queues behind the first, and starts when the first finishes.
//
// Use turn_get / turn_wait / turn_cancel to drive the turn's
// lifecycle. The agent never blocks on this call.

fn tool_chat_send(state: &ServerState) -> Tool {
    let state = state.clone();
    ToolBuilder::new("chat_send")
        .description(
            "Fire a turn against an open chat and return immediately with a \
             turn_id. The turn runs in the background; poll with `turn_get`, \
             block with `turn_wait`, or cancel with `turn_cancel`. Turns within \
             a single chat still serialize (second turn queues behind first). \
             For the blocking variant, see `chat_send_sync`.",
        )
        .handler(move |input: ChatSendInput| {
            let state = state.clone();
            async move {
                // Confirm chat exists before promising a turn_id.
                if state.get_chat(&input.chat_id).await.is_none() {
                    return Err(tower_mcp::Error::internal(format!(
                        "no chat with id `{}` (was it closed?)",
                        input.chat_id
                    )));
                }
                let handle = state.turns.register(Some(input.chat_id.clone())).await;
                let turn_id = handle.turn_id.clone();
                let span = tracing::info_span!(
                    "chat_send",
                    turn_id = %turn_id,
                    chat_id = %input.chat_id,
                    prompt_len = input.prompt.len(),
                );
                tracing::info!(parent: &span, "fired async turn");

                let state_for_worker = state.clone();
                let chat_id_for_worker = input.chat_id.clone();
                let prompt = input.prompt;
                tokio::spawn(
                    async move {
                        if handle.is_cancelled() {
                            tracing::info!("turn cancelled before start");
                            handle.cancelled();
                            return;
                        }
                        let conv = match state_for_worker.get_chat(&chat_id_for_worker).await {
                            Some(c) => c,
                            None => {
                                tracing::warn!("chat closed before turn could acquire it");
                                handle.fail(format!(
                                    "no chat with id `{chat_id_for_worker}` (was it closed?)"
                                ));
                                return;
                            }
                        };
                        let mut guard = conv.lock().await;
                        if handle.is_cancelled() {
                            tracing::info!("turn cancelled while waiting for mutex");
                            handle.cancelled();
                            return;
                        }
                        match guard.send(prompt).await {
                            Ok(turn) => {
                                let cost = turn.total_cost_usd();
                                let dur = turn.duration_ms();
                                let result_text = turn.result_text().map(str::to_string);
                                let session_id = turn.session_id().map(str::to_string);
                                let cumulative = guard.total_cost_usd();
                                let total_turns = guard.total_turns();
                                drop(guard);
                                tracing::info!(
                                    cost_usd = ?cost,
                                    duration_ms = ?dur,
                                    cumulative_cost_usd = cumulative,
                                    "turn done"
                                );
                                handle.complete(json!({
                                    "result": result_text,
                                    "session_id": session_id,
                                    "turn_cost_usd": cost,
                                    "duration_ms": dur,
                                    "cumulative_cost_usd": cumulative,
                                    "total_turns": total_turns,
                                }));
                            }
                            Err(e) => {
                                drop(guard);
                                tracing::error!(error = %e, "turn failed");
                                handle.fail(e);
                            }
                        }
                        state_for_worker.notify_resource_updated(format!(
                            "claude://chats/{chat_id_for_worker}"
                        ));
                    }
                    .instrument(span),
                );

                Ok(CallToolResult::json(json!({
                    "turn_id": turn_id,
                    "chat_id": input.chat_id,
                })))
            }
        })
        .build()
}

// -- chat_send_sync --------------------------------------------------

#[derive(Debug, Deserialize, JsonSchema)]
struct ChatSendInput {
    /// Identifier returned by `chat_open`.
    chat_id: String,
    /// The user prompt for this turn.
    prompt: String,
}

#[cfg(feature = "sync-agent-turns")]
fn tool_chat_send_sync(state: &ServerState) -> Tool {
    let state = state.clone();
    ToolBuilder::new("chat_send_sync")
        .description(
            "Blocking turn send. Holds the connection open until the assistant \
             finishes the turn, then returns assistant text + cost + turn count. \
             For agent turns prefer `chat_send` (async, returns turn_id); use \
             this when you genuinely want to block.",
        )
        .handler(move |input: ChatSendInput| {
            let state = state.clone();
            async move {
                let conv = state.get_chat(&input.chat_id).await.ok_or_else(|| {
                    tower_mcp::Error::internal(format!(
                        "no chat with id `{}` (was it closed?)",
                        input.chat_id
                    ))
                })?;
                let mut guard = conv.lock().await;
                let turn = guard.send(input.prompt).await.map_err(super_internal)?;
                let body = json!({
                    "result": turn.result_text(),
                    "session_id": turn.session_id(),
                    "turn_cost_usd": turn.total_cost_usd(),
                    "duration_ms": turn.duration_ms(),
                    "cumulative_cost_usd": guard.total_cost_usd(),
                    "total_turns": guard.total_turns(),
                });
                drop(guard);
                state.notify_resource_updated(format!("claude://chats/{}", input.chat_id));
                Ok(CallToolResult::json(body))
            }
        })
        .build()
}

// -- chat_send_stream_sync ------------------------------------------
//
// Streaming variant of chat_send_sync. Holds the connection open and
// forwards every InboundEvent the duplex session emits to the MCP
// client as a `notifications/progress` message so callers see
// assistant text deltas / tool-use blocks / system events as they
// arrive. Sync because the request stays open for the duration of
// the turn -- async streaming is a separate problem (likely solved
// via resource subscriptions on per-turn URIs).
//
// Implementation detail: we subscribe to the broadcast BEFORE calling
// send so we never miss the SystemInit or first Assistant chunk.
// The forwarder runs in a spawned task with a clone of `Context`;
// when send returns we abort it.

#[cfg(feature = "sync-agent-turns")]
fn tool_chat_send_stream_sync(state: &ServerState) -> Tool {
    let state = state.clone();
    ToolBuilder::new("chat_send_stream_sync")
        .description(
            "Blocking streaming turn send. Each event from the underlying \
             duplex session is forwarded as an MCP `notifications/progress` \
             event (assistant deltas, tool-use blocks, system events). \
             The final return is identical to chat_send_sync. Held connection \
             for the duration of the turn -- explicit sync per design rule.",
        )
        .extractor_handler(
            state,
            |State(state): State<ServerState>,
             ctx: Context,
             Json(input): Json<ChatSendInput>| async move {
                let conv = state.get_chat(&input.chat_id).await.ok_or_else(|| {
                    tower_mcp::Error::internal(format!(
                        "no chat with id `{}` (was it closed?)",
                        input.chat_id
                    ))
                })?;
                let mut guard = conv.lock().await;

                let mut rx = guard.session().subscribe();
                let ctx_for_task = ctx.clone();
                let forwarder = tokio::spawn(async move {
                    let mut counter: f64 = 0.0;
                    loop {
                        match rx.recv().await {
                            Ok(event) => {
                                counter += 1.0;
                                let msg = stringify_event(&event);
                                ctx_for_task
                                    .report_progress(counter, None, Some(&msg))
                                    .await;
                            }
                            // Lagged: skip and keep going.
                            Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                            // Closed: session ended.
                            Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                        }
                    }
                });

                let send_result = guard.send(input.prompt).await;
                forwarder.abort();
                let turn = send_result.map_err(super_internal)?;

                let body = json!({
                    "result": turn.result_text(),
                    "session_id": turn.session_id(),
                    "turn_cost_usd": turn.total_cost_usd(),
                    "duration_ms": turn.duration_ms(),
                    "cumulative_cost_usd": guard.total_cost_usd(),
                    "total_turns": guard.total_turns(),
                });
                drop(guard);
                state.notify_resource_updated(format!("claude://chats/{}", input.chat_id));
                Ok(CallToolResult::json(body))
            },
        )
        .build()
}

/// Render an [`InboundEvent`] as a short string suitable for the
/// `message` field of a progress notification. We pull the assistant
/// text where present so consumers can show progressive output;
/// other event types fall back to a type tag.
#[cfg(feature = "sync-agent-turns")]
fn stringify_event(event: &InboundEvent) -> String {
    match event {
        InboundEvent::SystemInit { session_id } => format!("system.init session={session_id}"),
        InboundEvent::Assistant(v) => {
            if let Some(text) = extract_assistant_text(v) {
                format!("assistant {}", truncate(&text, 200))
            } else {
                "assistant".to_string()
            }
        }
        InboundEvent::StreamEvent(_) => "stream_event".to_string(),
        InboundEvent::User(_) => "user".to_string(),
        InboundEvent::Other(v) => v
            .get("type")
            .and_then(|t| t.as_str())
            .map(|s| format!("other.{s}"))
            .unwrap_or_else(|| "other".to_string()),
    }
}

#[cfg(feature = "sync-agent-turns")]
fn extract_assistant_text(v: &serde_json::Value) -> Option<String> {
    let blocks = v
        .get("message")
        .and_then(|m| m.get("content"))?
        .as_array()?;
    let mut buf = String::new();
    for b in blocks {
        if b.get("type").and_then(|t| t.as_str()) == Some("text")
            && let Some(s) = b.get("text").and_then(|t| t.as_str())
        {
            buf.push_str(s);
        }
    }
    if buf.is_empty() { None } else { Some(buf) }
}

#[cfg(feature = "sync-agent-turns")]
fn truncate(s: &str, max_chars: usize) -> String {
    if s.chars().count() <= max_chars {
        s.to_string()
    } else {
        let mut out: String = s.chars().take(max_chars).collect();
        out.push('…');
        out
    }
}

// -- chat_list ------------------------------------------------------

#[derive(Debug, Default, Deserialize, JsonSchema)]
struct NoArgs {}

fn tool_chat_list(state: &ServerState) -> Tool {
    let state = state.clone();
    ToolBuilder::new("chat_list")
        .description("List currently open chats with their cumulative cost and turn count.")
        .read_only()
        .handler(move |_input: NoArgs| {
            let state = state.clone();
            async move {
                let map = state.chats.read().await;
                let mut entries = Vec::with_capacity(map.len());
                for (id, conv) in map.iter() {
                    let guard = conv.lock().await;
                    entries.push(json!({
                        "chat_id": id,
                        "total_turns": guard.total_turns(),
                        "total_cost_usd": guard.total_cost_usd(),
                        "session_id": guard.session_id(),
                    }));
                }
                Ok(CallToolResult::json(json!({"chats": entries})))
            }
        })
        .build()
}

// -- chat_history ---------------------------------------------------

#[derive(Debug, Deserialize, JsonSchema)]
struct ChatHistoryInput {
    /// Identifier of the chat to inspect.
    chat_id: String,
}

fn tool_chat_history(state: &ServerState) -> Tool {
    let state = state.clone();
    ToolBuilder::new("chat_history")
        .description(
            "Return the full per-turn history of a chat: the assistant text, \
             cost, and duration for each turn.",
        )
        .read_only()
        .handler(move |input: ChatHistoryInput| {
            let state = state.clone();
            async move {
                let conv = state.get_chat(&input.chat_id).await.ok_or_else(|| {
                    tower_mcp::Error::internal(format!("no chat with id `{}`", input.chat_id))
                })?;
                let guard = conv.lock().await;
                let turns: Vec<_> = guard
                    .history()
                    .iter()
                    .map(|t| {
                        json!({
                            "result": t.result_text(),
                            "session_id": t.session_id(),
                            "cost_usd": t.total_cost_usd(),
                            "duration_ms": t.duration_ms(),
                        })
                    })
                    .collect();
                Ok(CallToolResult::json(json!({
                    "chat_id": input.chat_id,
                    "turns": turns,
                    "total_cost_usd": guard.total_cost_usd(),
                    "total_turns": guard.total_turns(),
                })))
            }
        })
        .build()
}

// -- chat_interrupt -------------------------------------------------

#[derive(Debug, Deserialize, JsonSchema)]
struct ChatInterruptInput {
    /// Identifier of the chat to interrupt.
    chat_id: String,
}

fn tool_chat_interrupt(state: &ServerState) -> Tool {
    let state = state.clone();
    ToolBuilder::new("chat_interrupt")
        .description(
            "Send a clean mid-turn interrupt to a chat. The in-flight turn \
             (if any) returns with a partial result; the session stays open \
             for further turns.",
        )
        .handler(move |input: ChatInterruptInput| {
            let state = state.clone();
            async move {
                let conv = state.get_chat(&input.chat_id).await.ok_or_else(|| {
                    tower_mcp::Error::internal(format!("no chat with id `{}`", input.chat_id))
                })?;
                // Interrupting needs &DuplexSession access -- Conversation
                // exposes that through `session()`. We don't need to hold
                // the mutex across the interrupt; the in-flight `send`
                // holds it for us.
                let guard = conv.lock().await;
                guard.session().interrupt().await.map_err(super_internal)?;
                Ok(CallToolResult::json(json!({"ok": true})))
            }
        })
        .build()
}

// -- chat_budget ----------------------------------------------------

#[derive(Debug, Deserialize, JsonSchema)]
struct ChatBudgetInput {
    /// Identifier of the chat whose budget to inspect.
    chat_id: String,
}

fn tool_chat_budget(state: &ServerState) -> Tool {
    let state = state.clone();
    ToolBuilder::new("chat_budget")
        .description(
            "Read the BudgetTracker state for a chat: total spent so far, \
             configured ceiling, remaining, and warn threshold. Returns \
             `{ \"budget\": null }` if the chat was opened without a \
             max_cost_usd.",
        )
        .read_only()
        .handler(move |input: ChatBudgetInput| {
            let state = state.clone();
            async move {
                let conv = state.get_chat(&input.chat_id).await.ok_or_else(|| {
                    tower_mcp::Error::internal(format!("no chat with id `{}`", input.chat_id))
                })?;
                let guard = conv.lock().await;
                let body = match guard.budget() {
                    None => json!({"chat_id": input.chat_id, "budget": null}),
                    Some(b) => json!({
                        "chat_id": input.chat_id,
                        "budget": {
                            "total_usd": b.total_usd(),
                            "max_usd": b.max_usd(),
                            "remaining_usd": b.remaining_usd(),
                            "warn_at_usd": b.warn_at_usd(),
                        },
                    }),
                };
                Ok(CallToolResult::json(body))
            }
        })
        .build()
}

// -- chat_close -----------------------------------------------------

#[derive(Debug, Deserialize, JsonSchema)]
struct ChatCloseInput {
    /// Identifier of the chat to close.
    chat_id: String,
}

fn tool_chat_close(state: &ServerState) -> Tool {
    let state = state.clone();
    ToolBuilder::new("chat_close")
        .description("Drop a chat from the server. No-op if the id is unknown.")
        .handler(move |input: ChatCloseInput| {
            let state = state.clone();
            async move {
                let removed = state.remove_chat(&input.chat_id).await;
                let existed = removed.is_some();
                if let Some(arc) = removed {
                    // Try to drain into close(); only proceed if we hold the
                    // last reference, otherwise just drop.
                    if let Ok(mutex) = Arc::try_unwrap(arc) {
                        let conv = mutex.into_inner();
                        let _ = conv.close().await;
                    }
                }
                if existed {
                    state.notify_resources_list_changed();
                }
                Ok(CallToolResult::json(json!({
                    "ok": true,
                    "existed": existed,
                })))
            }
        })
        .build()
}

// -- helpers --------------------------------------------------------

use crate::errors::from_wrapper as super_internal;
