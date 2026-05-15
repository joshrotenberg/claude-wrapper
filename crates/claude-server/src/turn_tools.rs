//! MCP tools that drive the async turn registry.
//!
//! Fire a turn with `chat_send` (or `claude_query` once step 5
//! lands), get a `turn_id`, then use these to inspect, wait, cancel,
//! or list:
//!
//! - `turn_get(turn_id)` -- non-blocking status snapshot
//! - `turn_wait(turn_id, timeout_secs?)` -- block until terminal
//!   (or timeout; turn keeps running on timeout)
//! - `turn_cancel(turn_id)` -- cooperative cancel (flips a flag;
//!   the worker observes and short-circuits)
//! - `turn_list(chat_id?)` -- enumerate, optional chat filter

use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{Value, json};
use tower_mcp::{CallToolResult, Tool, ToolBuilder};

use crate::state::ServerState;
use crate::turns::TurnSnapshot;

pub(crate) fn tools(state: &ServerState) -> Vec<Tool> {
    #[cfg_attr(not(feature = "metrics"), allow(unused_mut))]
    let mut out = vec![
        tool_turn_get(state),
        tool_turn_wait(state),
        tool_turn_cancel(state),
        tool_turn_list(state),
    ];
    #[cfg(feature = "metrics")]
    out.push(tool_metrics_summary(state));
    out
}

// -- metrics_summary -------------------------------------------------

#[cfg(feature = "metrics")]
#[derive(Debug, Default, Deserialize, JsonSchema)]
struct NoArgs {}

#[cfg(feature = "metrics")]
fn tool_metrics_summary(state: &ServerState) -> Tool {
    let state = state.clone();
    ToolBuilder::new("metrics_summary")
        .description(
            "Snapshot of process-wide turn counters: turns_fired / done / \
             failed / cancelled, in_flight, total_cost_usd. Lets a \
             coordinator agent introspect its own spend mid-run -- \
             \"before I fire another turn, how much have I spent?\"",
        )
        .read_only()
        .handler(move |_input: NoArgs| {
            let state = state.clone();
            async move {
                let snap = state.turns.metrics().snapshot();
                Ok(CallToolResult::json(json!({
                    "turns_fired": snap.turns_fired,
                    "turns_done": snap.turns_done,
                    "turns_failed": snap.turns_failed,
                    "turns_cancelled": snap.turns_cancelled,
                    "in_flight": snap.in_flight,
                    "total_cost_usd": snap.total_cost_usd,
                })))
            }
        })
        .build()
}

// -- turn_get --------------------------------------------------------

#[derive(Debug, Deserialize, JsonSchema)]
struct TurnIdInput {
    /// Identifier returned by `chat_send` / `claude_query`.
    turn_id: String,
}

fn tool_turn_get(state: &ServerState) -> Tool {
    let state = state.clone();
    ToolBuilder::new("turn_get")
        .description(
            "Non-blocking snapshot of an async turn. Returns the turn's \
             current status (running / done / failed / cancelled), plus \
             the JSON result on `done` or the error string on `failed`. \
             Errors if the turn_id is unknown.",
        )
        .read_only()
        .handler(move |input: TurnIdInput| {
            let state = state.clone();
            async move {
                match state.turns.get(&input.turn_id).await {
                    Some(snap) => Ok(CallToolResult::json(snapshot_to_json(&snap))),
                    None => Err(unknown_turn(&input.turn_id)),
                }
            }
        })
        .build()
}

// -- turn_wait -------------------------------------------------------

#[derive(Debug, Deserialize, JsonSchema)]
struct TurnWaitInput {
    /// Identifier returned by `chat_send` / `claude_query`.
    turn_id: String,
    /// Optional timeout in seconds. If the turn doesn't settle in
    /// this window, returns `{ status: "timeout", turn_id }` with
    /// the turn still running. Omit to wait indefinitely (request
    /// connection stays open).
    #[serde(default)]
    timeout_secs: Option<f64>,
}

fn tool_turn_wait(state: &ServerState) -> Tool {
    let state = state.clone();
    ToolBuilder::new("turn_wait")
        .description(
            "Block until an async turn reaches a terminal status, or until \
             the optional timeout_secs elapses. On settle: returns the same \
             shape as turn_get. On timeout: returns `{ status: \"timeout\", \
             turn_id }` and the turn keeps running -- callers can poll again \
             or cancel.",
        )
        .handler(move |input: TurnWaitInput| {
            let state = state.clone();
            async move {
                let timeout = input.timeout_secs.map(std::time::Duration::from_secs_f64);
                match state.turns.wait(&input.turn_id, timeout).await {
                    Ok(Some(snap)) => Ok(CallToolResult::json(snapshot_to_json(&snap))),
                    Ok(None) => Ok(CallToolResult::json(json!({
                        "turn_id": input.turn_id,
                        "status": "timeout",
                    }))),
                    Err(_) => Err(unknown_turn(&input.turn_id)),
                }
            }
        })
        .build()
}

// -- turn_cancel -----------------------------------------------------

fn tool_turn_cancel(state: &ServerState) -> Tool {
    let state = state.clone();
    ToolBuilder::new("turn_cancel")
        .description(
            "Request cancellation of an async turn. This flips a cooperative \
             flag the worker checks between awaits; an already-in-flight \
             claude turn is not interrupted by this alone (use chat_interrupt \
             at the chat level for that). Returns `{ ok: true, existed }`.",
        )
        .handler(move |input: TurnIdInput| {
            let state = state.clone();
            async move {
                let existed = state.turns.cancel(&input.turn_id).await;
                Ok(CallToolResult::json(json!({
                    "ok": true,
                    "existed": existed,
                    "turn_id": input.turn_id,
                })))
            }
        })
        .build()
}

// -- turn_list -------------------------------------------------------

#[derive(Debug, Default, Deserialize, JsonSchema)]
struct TurnListInput {
    /// Optional chat filter; omit for all turns across all chats
    /// plus single-shot (no chat_id) turns.
    #[serde(default)]
    chat_id: Option<String>,
}

fn tool_turn_list(state: &ServerState) -> Tool {
    let state = state.clone();
    ToolBuilder::new("turn_list")
        .description(
            "Enumerate registered turns -- running and recently-terminal. \
             Optional `chat_id` filters to one chat. Each entry has the same \
             shape as turn_get.",
        )
        .read_only()
        .handler(move |input: TurnListInput| {
            let state = state.clone();
            async move {
                let snaps = state.turns.list(input.chat_id.as_deref()).await;
                let body: Vec<Value> = snaps.iter().map(snapshot_to_json).collect();
                Ok(CallToolResult::json(json!({"turns": body})))
            }
        })
        .build()
}

// -- helpers --------------------------------------------------------

fn snapshot_to_json(snap: &TurnSnapshot) -> Value {
    // Hand-rolled rather than serde_json::to_value(snap) so the
    // wire shape stays an explicit thing we can change in one place.
    json!({
        "turn_id": snap.turn_id,
        "chat_id": snap.chat_id,
        "status": match snap.status {
            crate::turns::TurnStatus::Running => "running",
            crate::turns::TurnStatus::Done => "done",
            crate::turns::TurnStatus::Failed => "failed",
            crate::turns::TurnStatus::Cancelled => "cancelled",
        },
        "started_at_us": snap.started_at_us,
        "finished_at_us": snap.finished_at_us,
        "result": snap.result,
        "error": snap.error,
    })
}

fn unknown_turn(turn_id: &str) -> tower_mcp::Error {
    tower_mcp::Error::internal(format!("no turn with id `{turn_id}`"))
}
