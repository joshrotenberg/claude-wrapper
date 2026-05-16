//! Read-only access to Claude Code's user-level **agent** definitions
//! (`~/.claude/agents/<stem>.md`), exposed as MCP tools and
//! resources. Backed by [`claude_wrapper::artifacts::AgentsRoot`].
//!
//! Tools:
//! - `agent_list` -- enumerate every `*.md` agent at the configured
//!   root. Returns summary metadata only.
//! - `agent_get { file_stem }` -- one agent's full record including
//!   the prompt body and any unknown frontmatter keys.
//!
//! Resources:
//! - `claude://agents` -- same shape as `agent_list`.
//!
//! Resource templates:
//! - `claude://agents/{file_stem}` -- one agent's full record.
//!
//! Mutations (write / delete) live elsewhere -- they are double-gated
//! by the `mutations` Cargo feature plus `policy.allow_mutations`
//! and are not yet implemented.

use std::collections::HashMap;

use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::json;
use tower_mcp::protocol::ReadResourceResult;
use tower_mcp::resource::ResourceTemplate;
use tower_mcp::{
    CallToolResult, Resource, ResourceBuilder, ResourceTemplateBuilder, Tool, ToolBuilder,
};

use claude_wrapper::artifacts::AgentsRoot;

use crate::state::ServerState;

/// Build the artifacts-feature tool list.
pub(crate) fn tools(state: &ServerState) -> Vec<Tool> {
    vec![tool_agent_list(state), tool_agent_get(state)]
}

/// Build the artifacts-feature resource list.
pub(crate) fn resources(state: &ServerState) -> Vec<Resource> {
    vec![resource_agents(state)]
}

/// Build the artifacts-feature resource templates.
pub(crate) fn templates(state: &ServerState) -> Vec<ResourceTemplate> {
    vec![template_agent_detail(state)]
}

// -- tool_agent_list -------------------------------------------------

#[derive(Debug, Default, Deserialize, JsonSchema)]
struct NoArgs {}

fn tool_agent_list(state: &ServerState) -> Tool {
    let state = state.clone();
    ToolBuilder::new("agent_list")
        .description(
            "Enumerate user-level agents under `~/.claude/agents/<stem>.md`. \
             Each entry: `file_stem` (canonical lookup handle), `name` \
             (frontmatter or stem fallback), `description`, `tools` \
             (parsed comma-list), `model`, `file_path`, `size_bytes`. \
             Read-only.",
        )
        .read_only()
        .handler(move |_input: NoArgs| {
            let state = state.clone();
            async move {
                let root = agents_root(&state).map_err(internal)?;
                let agents = root.list().map_err(internal)?;
                Ok(CallToolResult::json(json!({
                    "agents": agents
                        .iter()
                        .map(agent_summary_to_json)
                        .collect::<Vec<_>>(),
                })))
            }
        })
        .build()
}

// -- tool_agent_get --------------------------------------------------

#[derive(Debug, Deserialize, JsonSchema)]
struct AgentGetInput {
    /// Filename stem (the basename of `<stem>.md` under the agents
    /// root). This is the canonical handle, not the frontmatter
    /// `name` -- those can diverge.
    file_stem: String,
}

fn tool_agent_get(state: &ServerState) -> Tool {
    let state = state.clone();
    ToolBuilder::new("agent_get")
        .description(
            "Read one agent's full record by `file_stem`. Returns the \
             frontmatter metadata plus the prompt body and any unknown \
             frontmatter keys (in `extra`). Errors if no agent with \
             that stem exists under the configured agents root. \
             Read-only.",
        )
        .read_only()
        .handler(move |input: AgentGetInput| {
            let state = state.clone();
            async move {
                let root = agents_root(&state).map_err(internal)?;
                let agent = root.get(&input.file_stem).map_err(internal)?;
                Ok(CallToolResult::json(agent_to_json(&agent)))
            }
        })
        .build()
}

// -- resource: claude://agents --------------------------------------

fn resource_agents(state: &ServerState) -> Resource {
    let state = state.clone();
    ResourceBuilder::new("claude://agents")
        .name("Claude agents")
        .description(
            "Live view of every user-level agent at the configured \
             agents root. Same shape as the agent_list tool.",
        )
        .mime_type("application/json")
        .handler(move || {
            let state = state.clone();
            async move {
                let root = agents_root(&state).map_err(internal)?;
                let agents = root.list().map_err(internal)?;
                let body = json!({
                    "agents": agents
                        .iter()
                        .map(agent_summary_to_json)
                        .collect::<Vec<_>>(),
                });
                let text = serde_json::to_string_pretty(&body).unwrap_or_default();
                Ok(ReadResourceResult::text("claude://agents", text))
            }
        })
        .build()
}

// -- template: claude://agents/{file_stem} --------------------------

fn template_agent_detail(state: &ServerState) -> ResourceTemplate {
    let state = state.clone();
    ResourceTemplateBuilder::new("claude://agents/{file_stem}")
        .name("Agent detail")
        .description(
            "Full agent record keyed by file stem. Same shape as the \
             agent_get tool.",
        )
        .mime_type("application/json")
        .handler(move |uri: String, vars: HashMap<String, String>| {
            let state = state.clone();
            async move {
                let stem = vars.get("file_stem").cloned().unwrap_or_default();
                let root = agents_root(&state).map_err(internal)?;
                let agent = root.get(&stem).map_err(internal)?;
                let text = serde_json::to_string_pretty(&agent_to_json(&agent)).unwrap_or_default();
                Ok(ReadResourceResult::text(uri, text))
            }
        })
}

// -- helpers --------------------------------------------------------

fn agents_root(state: &ServerState) -> claude_wrapper::error::Result<AgentsRoot> {
    match state.config.agents_root.as_ref() {
        Some(path) => Ok(AgentsRoot::at(path.clone())),
        None => AgentsRoot::home(),
    }
}

fn internal(e: impl std::fmt::Display) -> tower_mcp::Error {
    tower_mcp::Error::internal(e.to_string())
}

fn agent_summary_to_json(a: &claude_wrapper::artifacts::AgentSummary) -> serde_json::Value {
    json!({
        "file_stem": a.file_stem,
        "name": a.name,
        "description": a.description,
        "tools": a.tools,
        "model": a.model,
        "file_path": a.file_path,
        "size_bytes": a.size_bytes,
    })
}

fn agent_to_json(a: &claude_wrapper::artifacts::Agent) -> serde_json::Value {
    json!({
        "file_stem": a.file_stem,
        "name": a.name,
        "description": a.description,
        "tools": a.tools,
        "model": a.model,
        "file_path": a.file_path,
        "body": a.body,
        "extra": a.extra,
    })
}
