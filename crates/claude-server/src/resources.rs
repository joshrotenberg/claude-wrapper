//! MCP resources exposed by the server.
//!
//! Resources are read-only state the client can fetch via
//! `resources/list` and `resources/read`. They complement tools
//! (which take action) and prompts (which template messages).
//!
//! Today: a sanitized view of the server config and a list of the
//! registered tools. Chat resources land with the L2.5 surface.

use std::collections::HashMap;

use serde_json::json;
use tower_mcp::protocol::ReadResourceResult;
use tower_mcp::resource::ResourceTemplate;
use tower_mcp::{Resource, ResourceBuilder, ResourceTemplateBuilder};

use crate::state::ServerState;

pub(crate) fn resources(state: &ServerState) -> Vec<Resource> {
    vec![
        resource_config(state),
        resource_tools(state),
        resource_chats(state),
        resource_metrics(state),
    ]
}

fn resource_metrics(state: &ServerState) -> Resource {
    let state = state.clone();
    ResourceBuilder::new("claude://metrics")
        .name("Process metrics")
        .description(
            "Process-wide turn counters as JSON: turns_fired/done/failed/\
             cancelled, in_flight, total_cost_usd. Same shape as the \
             metrics_summary tool. UIs subscribe here for live dashboards.",
        )
        .mime_type("application/json")
        .handler(move || {
            let state = state.clone();
            async move {
                let snap = state.turns.metrics().snapshot();
                let body = json!({
                    "turns_fired": snap.turns_fired,
                    "turns_done": snap.turns_done,
                    "turns_failed": snap.turns_failed,
                    "turns_cancelled": snap.turns_cancelled,
                    "in_flight": snap.in_flight,
                    "total_cost_usd": snap.total_cost_usd,
                });
                let text = serde_json::to_string_pretty(&body).unwrap_or_default();
                Ok(ReadResourceResult::text("claude://metrics", text))
            }
        })
        .build()
}

/// MCP resource templates -- URI-templated resources that take
/// path parameters (RFC 6570 Level 1, e.g. `{id}`).
pub(crate) fn templates(state: &ServerState) -> Vec<ResourceTemplate> {
    vec![template_chat_detail(state)]
}

fn template_chat_detail(state: &ServerState) -> ResourceTemplate {
    let state = state.clone();
    ResourceTemplateBuilder::new("claude://chats/{id}")
        .name("Chat detail")
        .description(
            "Per-chat snapshot keyed by chat_id: full TurnResult history, \
             cumulative cost, session id, optional budget state. UIs subscribe \
             here instead of polling chat_history.",
        )
        .mime_type("application/json")
        .handler(move |uri: String, vars: HashMap<String, String>| {
            let state = state.clone();
            async move {
                let id = vars.get("id").cloned().unwrap_or_default();
                let conv = state
                    .get_chat(&id)
                    .await
                    .ok_or_else(|| tower_mcp::Error::internal(format!("no chat with id `{id}`")))?;
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
                let budget = guard.budget().map(|b| {
                    json!({
                        "total_usd": b.total_usd(),
                        "max_usd": b.max_usd(),
                        "remaining_usd": b.remaining_usd(),
                        "warn_at_usd": b.warn_at_usd(),
                    })
                });
                let body = json!({
                    "chat_id": id,
                    "session_id": guard.session_id(),
                    "total_turns": guard.total_turns(),
                    "total_cost_usd": guard.total_cost_usd(),
                    "budget": budget,
                    "turns": turns,
                });
                let text = serde_json::to_string_pretty(&body).unwrap_or_default();
                Ok(ReadResourceResult::text(uri, text))
            }
        })
}

fn resource_chats(state: &ServerState) -> Resource {
    let state = state.clone();
    ResourceBuilder::new("claude://chats")
        .name("Open chats")
        .description("Live view of every server-held chat: id, turn count, cumulative cost.")
        .mime_type("application/json")
        .handler(move || {
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
                let text =
                    serde_json::to_string_pretty(&json!({"chats": entries})).unwrap_or_default();
                Ok(ReadResourceResult::text("claude://chats", text))
            }
        })
        .build()
}

fn resource_config(state: &ServerState) -> Resource {
    let cfg = state.config.clone();
    ResourceBuilder::new("claude://config")
        .name("Server config")
        .description("Sanitized view of the active ServerConfig (env values redacted).")
        .mime_type("application/json")
        .handler(move || {
            let cfg = cfg.clone();
            async move {
                let env: serde_json::Map<String, serde_json::Value> = cfg
                    .claude
                    .env
                    .iter()
                    .map(|(k, v)| (k.clone(), json!(redact(k, v))))
                    .collect();
                let body = json!({
                    "claude": {
                        "binary": cfg.claude.binary,
                        "working_dir": cfg.claude.working_dir,
                        "timeout_secs": cfg.claude.timeout_secs,
                        "env": env,
                        "global_args": cfg.claude.global_args,
                    }
                });
                let text = serde_json::to_string_pretty(&body).unwrap_or_default();
                Ok(ReadResourceResult::text("claude://config", text))
            }
        })
        .build()
}

fn resource_tools(state: &ServerState) -> Resource {
    let cfg = state.config.clone();
    ResourceBuilder::new("claude://tools")
        .name("Registered tools")
        .description("List of tools currently registered on this server.")
        .mime_type("application/json")
        .handler(move || {
            let cfg = cfg.clone();
            async move {
                let cfg_clone = (*cfg).clone();
                let tools = crate::registered_tools(cfg_clone)
                    .map_err(|e| tower_mcp::Error::internal(e.to_string()))?;
                let listed: Vec<_> = tools
                    .into_iter()
                    .map(|t| json!({"name": t.name, "description": t.description}))
                    .collect();
                let text = serde_json::to_string_pretty(&json!(listed)).unwrap_or_default();
                Ok(ReadResourceResult::text("claude://tools", text))
            }
        })
        .build()
}

/// Redact env var values for keys that look secret.
fn redact(key: &str, value: &str) -> String {
    let upper = key.to_ascii_uppercase();
    if ["KEY", "TOKEN", "SECRET", "PASSWORD"]
        .iter()
        .any(|needle| upper.contains(needle))
    {
        if value.is_empty() {
            "<unset>".to_string()
        } else {
            "<redacted>".to_string()
        }
    } else {
        value.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::redact;

    #[test]
    fn redacts_obvious_secret_keys() {
        assert_eq!(redact("ANTHROPIC_API_KEY", "sk-abc"), "<redacted>");
        assert_eq!(redact("github_token", "ghp_abc"), "<redacted>");
        assert_eq!(redact("USER_SECRET", "x"), "<redacted>");
    }

    #[test]
    fn passes_non_secret_values_through() {
        assert_eq!(redact("PATH", "/usr/bin"), "/usr/bin");
        assert_eq!(redact("LANG", "en_US"), "en_US");
    }
}
