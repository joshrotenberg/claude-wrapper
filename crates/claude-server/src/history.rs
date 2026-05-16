//! Read-only access to Claude Code's on-disk session history,
//! exposed as MCP tools and resources.
//!
//! Backed by [`claude_wrapper::history::HistoryRoot`]. The wrapper
//! owns the JSONL parsing and slug-decoding; this module is a thin
//! MCP surface on top.
//!
//! Tools:
//! - `claude_project_list` -- enumerate project directories with
//!   summary metadata.
//! - `claude_session_list { project_slug? }` -- enumerate sessions,
//!   optionally filtered to one project.
//! - `claude_session_get { session_id }` -- full typed entry log
//!   for one session.
//!
//! Resources:
//! - `claude://projects` -- same shape as `claude_project_list` for
//!   subscribable / cache-friendly UIs.
//!
//! Resource templates:
//! - `claude://projects/{slug}` -- sessions for one project.
//! - `claude://sessions/{id}` -- full session entry log.

use std::collections::HashMap;

use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::json;
use tower_mcp::protocol::ReadResourceResult;
use tower_mcp::resource::ResourceTemplate;
use tower_mcp::{
    CallToolResult, Resource, ResourceBuilder, ResourceTemplateBuilder, Tool, ToolBuilder,
};

use claude_wrapper::history::HistoryRoot;

use crate::state::ServerState;

/// Build the history-feature tool list.
pub(crate) fn tools(state: &ServerState) -> Vec<Tool> {
    vec![
        tool_project_list(state),
        tool_session_list(state),
        tool_session_get(state),
    ]
}

/// Build the history-feature resource list.
pub(crate) fn resources(state: &ServerState) -> Vec<Resource> {
    vec![resource_projects(state)]
}

/// Build the history-feature resource templates.
pub(crate) fn templates(state: &ServerState) -> Vec<ResourceTemplate> {
    vec![
        template_project_detail(state),
        template_session_detail(state),
    ]
}

// -- tool_project_list ----------------------------------------------

#[derive(Debug, Default, Deserialize, JsonSchema)]
struct NoArgs {}

fn tool_project_list(state: &ServerState) -> Tool {
    let state = state.clone();
    ToolBuilder::new("claude_project_list")
        .description(
            "Enumerate Claude Code project directories under `~/.claude/projects/`. \
             Each entry: `slug` (on-disk dir name, the encoded path), \
             `decoded_path` (best-effort decode back to a filesystem path), \
             `session_count`, `last_modified` (Unix-epoch seconds, optional). \
             Read-only.",
        )
        .read_only()
        .handler(move |_input: NoArgs| {
            let state = state.clone();
            async move {
                let root = history_root(&state).map_err(internal)?;
                let projects = root.list_projects().map_err(internal)?;
                Ok(CallToolResult::json(json!({
                    "projects": projects
                        .iter()
                        .map(project_summary_to_json)
                        .collect::<Vec<_>>(),
                })))
            }
        })
        .build()
}

// -- tool_session_list ----------------------------------------------

#[derive(Debug, Default, Deserialize, JsonSchema)]
struct SessionListInput {
    /// Optional project slug to filter on. Omit for the union
    /// across every project.
    #[serde(default)]
    project_slug: Option<String>,
}

fn tool_session_list(state: &ServerState) -> Tool {
    let state = state.clone();
    ToolBuilder::new("claude_session_list")
        .description(
            "Enumerate session JSONL files. With `project_slug`, lists \
             sessions for that one project; without, returns the union \
             across every project. Each entry carries summary metadata: \
             session_id, project_slug, message_count (user + assistant), \
             first/last timestamp, optional ai-generated title, file size. \
             Read-only.",
        )
        .read_only()
        .handler(move |input: SessionListInput| {
            let state = state.clone();
            async move {
                let root = history_root(&state).map_err(internal)?;
                let sessions = root
                    .list_sessions(input.project_slug.as_deref())
                    .map_err(internal)?;
                Ok(CallToolResult::json(json!({
                    "sessions": sessions
                        .iter()
                        .map(session_summary_to_json)
                        .collect::<Vec<_>>(),
                })))
            }
        })
        .build()
}

// -- tool_session_get -----------------------------------------------

#[derive(Debug, Deserialize, JsonSchema)]
struct SessionGetInput {
    /// Session UUID (file basename without `.jsonl`).
    session_id: String,
}

fn tool_session_get(state: &ServerState) -> Tool {
    let state = state.clone();
    ToolBuilder::new("claude_session_get")
        .description(
            "Read one session's full entry log by `session_id`. Returns \
             every parsed entry in arrival order: user, assistant, and \
             other (queue-operation, attachment, ai-title, last-prompt, \
             unknown future types). Errors if no session with that id \
             exists under the configured history root. Read-only.",
        )
        .read_only()
        .handler(move |input: SessionGetInput| {
            let state = state.clone();
            async move {
                let root = history_root(&state).map_err(internal)?;
                let log = root.read_session(&input.session_id).map_err(internal)?;
                Ok(CallToolResult::json(session_log_to_json(&log)))
            }
        })
        .build()
}

// -- resource: claude://projects -----------------------------------

fn resource_projects(state: &ServerState) -> Resource {
    let state = state.clone();
    ResourceBuilder::new("claude://projects")
        .name("Claude projects")
        .description(
            "Live view of every project directory under the configured \
             history root. Same shape as the claude_project_list tool.",
        )
        .mime_type("application/json")
        .handler(move || {
            let state = state.clone();
            async move {
                let root = history_root(&state).map_err(internal)?;
                let projects = root.list_projects().map_err(internal)?;
                let body = json!({
                    "projects": projects
                        .iter()
                        .map(project_summary_to_json)
                        .collect::<Vec<_>>(),
                });
                let text = serde_json::to_string_pretty(&body).unwrap_or_default();
                Ok(ReadResourceResult::text("claude://projects", text))
            }
        })
        .build()
}

// -- template: claude://projects/{slug} -----------------------------

fn template_project_detail(state: &ServerState) -> ResourceTemplate {
    let state = state.clone();
    ResourceTemplateBuilder::new("claude://projects/{slug}")
        .name("Project detail")
        .description(
            "Per-project session list keyed by encoded slug. Same shape \
             as `claude_session_list { project_slug }`.",
        )
        .mime_type("application/json")
        .handler(move |uri: String, vars: HashMap<String, String>| {
            let state = state.clone();
            async move {
                let slug = vars.get("slug").cloned().unwrap_or_default();
                let root = history_root(&state).map_err(internal)?;
                let sessions = root.list_sessions(Some(&slug)).map_err(internal)?;
                let body = json!({
                    "project_slug": slug,
                    "sessions": sessions
                        .iter()
                        .map(session_summary_to_json)
                        .collect::<Vec<_>>(),
                });
                let text = serde_json::to_string_pretty(&body).unwrap_or_default();
                Ok(ReadResourceResult::text(uri, text))
            }
        })
}

// -- template: claude://sessions/{id} -------------------------------

fn template_session_detail(state: &ServerState) -> ResourceTemplate {
    let state = state.clone();
    ResourceTemplateBuilder::new("claude://sessions/{id}")
        .name("Session detail")
        .description(
            "Full parsed entry log for one session, keyed by session_id. \
             Same shape as the claude_session_get tool.",
        )
        .mime_type("application/json")
        .handler(move |uri: String, vars: HashMap<String, String>| {
            let state = state.clone();
            async move {
                let id = vars.get("id").cloned().unwrap_or_default();
                let root = history_root(&state).map_err(internal)?;
                let log = root.read_session(&id).map_err(internal)?;
                let text =
                    serde_json::to_string_pretty(&session_log_to_json(&log)).unwrap_or_default();
                Ok(ReadResourceResult::text(uri, text))
            }
        })
}

// -- helpers --------------------------------------------------------

fn history_root(state: &ServerState) -> claude_wrapper::error::Result<HistoryRoot> {
    match state.config.history_root.as_ref() {
        Some(path) => Ok(HistoryRoot::at(path.clone())),
        None => HistoryRoot::home(),
    }
}

use crate::errors::from_wrapper as internal;

fn project_summary_to_json(p: &claude_wrapper::history::ProjectSummary) -> serde_json::Value {
    json!({
        "slug": p.slug,
        "decoded_path": p.decoded_path,
        "session_count": p.session_count,
        "last_modified_secs": p.last_modified.and_then(|t| {
            t.duration_since(std::time::UNIX_EPOCH).ok().map(|d| d.as_secs())
        }),
    })
}

fn session_summary_to_json(s: &claude_wrapper::history::SessionSummary) -> serde_json::Value {
    json!({
        "session_id": s.session_id,
        "project_slug": s.project_slug,
        "message_count": s.message_count,
        "first_timestamp": s.first_timestamp,
        "last_timestamp": s.last_timestamp,
        "title": s.title,
        "size_bytes": s.size_bytes,
    })
}

fn session_log_to_json(log: &claude_wrapper::history::SessionLog) -> serde_json::Value {
    json!({
        "session_id": log.session_id,
        "project_slug": log.project_slug,
        "entries": log
            .entries
            .iter()
            .map(history_entry_to_json)
            .collect::<Vec<_>>(),
    })
}

fn history_entry_to_json(e: &claude_wrapper::history::HistoryEntry) -> serde_json::Value {
    use claude_wrapper::history::HistoryEntry;
    match e {
        HistoryEntry::User {
            uuid,
            timestamp,
            cwd,
            git_branch,
            message,
            ..
        } => json!({
            "kind": "user",
            "uuid": uuid,
            "timestamp": timestamp,
            "cwd": cwd,
            "git_branch": git_branch,
            "message": message,
        }),
        HistoryEntry::Assistant {
            uuid,
            timestamp,
            message,
            ..
        } => json!({
            "kind": "assistant",
            "uuid": uuid,
            "timestamp": timestamp,
            "message": message,
        }),
        HistoryEntry::Other { type_tag, raw } => json!({
            "kind": "other",
            "type_tag": type_tag,
            "raw": raw,
        }),
    }
}
