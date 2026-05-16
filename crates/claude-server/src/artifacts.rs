//! User-level **agent** definitions (`~/.claude/agents/<stem>.md`)
//! exposed as MCP tools and resources. Backed by
//! [`claude_wrapper::artifacts::AgentsRoot`].
//!
//! Read tools (always-on under the `artifacts` feature):
//! - `agent_list` -- enumerate every `*.md` agent at the configured
//!   root. Returns summary metadata only.
//! - `agent_get { file_stem }` -- one agent's full record including
//!   the prompt body and any unknown frontmatter keys.
//!
//! Mutating tools (gated by `artifacts` + `mutations` Cargo features
//! AND `policy.allow_mutations` at runtime):
//! - `agent_write { file_stem, name?, description?, tools[], model?, body, extra{}, if_not_exists? }`
//!   -- create or overwrite (upsert by default; `if_not_exists: true`
//!   makes it create-only).
//! - `agent_delete { file_stem }` -- remove the file.
//!
//! Resources:
//! - `claude://agents` -- same shape as `agent_list`.
//!
//! Resource templates:
//! - `claude://agents/{file_stem}` -- one agent's full record.

#[cfg(feature = "mutations")]
use std::collections::BTreeMap;
use std::collections::HashMap;

use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::json;
use tower_mcp::protocol::ReadResourceResult;
use tower_mcp::resource::ResourceTemplate;
use tower_mcp::{
    CallToolResult, Resource, ResourceBuilder, ResourceTemplateBuilder, Tool, ToolBuilder,
};

#[cfg(feature = "mutations")]
use claude_wrapper::artifacts::AgentWriteInput;
use claude_wrapper::artifacts::AgentsRoot;

use crate::state::ServerState;

/// Build the artifacts-feature tool list (read-only).
pub(crate) fn tools(state: &ServerState) -> Vec<Tool> {
    vec![tool_agent_list(state), tool_agent_get(state)]
}

/// Build the artifacts-feature mutating tool list. Caller is
/// responsible for runtime gating on `policy.allow_mutations`.
#[cfg(feature = "mutations")]
pub(crate) fn mutating_tools(state: &ServerState) -> Vec<Tool> {
    vec![tool_agent_write(state), tool_agent_delete(state)]
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

// -- tool_agent_write (mutating) ------------------------------------

#[cfg(feature = "mutations")]
#[derive(Debug, Deserialize, JsonSchema)]
struct AgentWriteInputJson {
    /// Filename stem (the basename of `<stem>.md` under the agents
    /// root). Must not be empty, `.`, `..`, or contain `/`, `\`, or
    /// NUL bytes.
    file_stem: String,
    /// Frontmatter `name`. Defaults to `file_stem` if absent.
    #[serde(default)]
    name: Option<String>,
    /// Frontmatter `description`. Omitted when None.
    #[serde(default)]
    description: Option<String>,
    /// Frontmatter `tools` as a list; rendered comma-joined. Empty
    /// list omits the key entirely.
    #[serde(default)]
    tools: Vec<String>,
    /// Frontmatter `model`. Omitted when None.
    #[serde(default)]
    model: Option<String>,
    /// Body of the agent prompt. Trimmed of surrounding whitespace
    /// before write.
    body: String,
    /// Additional frontmatter key/value pairs preserved verbatim.
    #[serde(default)]
    extra: BTreeMap<String, String>,
    /// When true, fail if the agent already exists. Default false
    /// (upsert: create or overwrite).
    #[serde(default)]
    if_not_exists: bool,
}

#[cfg(feature = "mutations")]
fn tool_agent_write(state: &ServerState) -> Tool {
    let state = state.clone();
    ToolBuilder::new("agent_write")
        .description(
            "Create or overwrite an agent at `<file_stem>.md` under the \
             configured agents root. Atomic write (tempfile + rename). \
             Pass `if_not_exists: true` for create-only semantics. \
             Required by both `mutations` Cargo feature and runtime \
             `policy.allow_mutations`. Path-traversal validated.",
        )
        .handler(move |input: AgentWriteInputJson| {
            let state = state.clone();
            async move {
                let root = agents_root(&state).map_err(internal)?;
                let stem = input.file_stem.clone();
                let payload = AgentWriteInput {
                    name: input.name,
                    description: input.description,
                    tools: input.tools,
                    model: input.model,
                    body: input.body,
                    extra: input.extra,
                };
                if input.if_not_exists {
                    root.write_new(&stem, payload).map_err(internal)?;
                } else {
                    root.write(&stem, payload).map_err(internal)?;
                }
                Ok(CallToolResult::json(json!({
                    "file_stem": stem,
                    "status": "written",
                })))
            }
        })
        .build()
}

// -- tool_agent_delete (mutating) -----------------------------------

#[cfg(feature = "mutations")]
#[derive(Debug, Deserialize, JsonSchema)]
struct AgentDeleteInput {
    /// Filename stem to remove (`<stem>.md` under the agents root).
    file_stem: String,
}

#[cfg(feature = "mutations")]
fn tool_agent_delete(state: &ServerState) -> Tool {
    let state = state.clone();
    ToolBuilder::new("agent_delete")
        .description(
            "Remove the `<file_stem>.md` agent. Errors if the file \
             doesn't exist. Required by both `mutations` Cargo feature \
             and runtime `policy.allow_mutations`. Path-traversal \
             validated.",
        )
        .handler(move |input: AgentDeleteInput| {
            let state = state.clone();
            async move {
                let root = agents_root(&state).map_err(internal)?;
                root.delete(&input.file_stem).map_err(internal)?;
                Ok(CallToolResult::json(json!({
                    "file_stem": input.file_stem,
                    "status": "deleted",
                })))
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

use crate::errors::from_wrapper as internal;

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
