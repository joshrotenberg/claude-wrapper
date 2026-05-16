//! Read-only access to Claude Code's on-disk **background-job**
//! state, exposed as MCP tools and resources. Backed by
//! [`claude_wrapper::jobs::JobsRoot`].
//!
//! Useful for hosts that want to reason about background work
//! launched via the `claude agents` TUI -- those tasks write to
//! the same on-disk state we read here, and each job's session
//! transcript ends up in `~/.claude/projects/` for the
//! [`crate::history`] feature to parse.
//!
//! Tools:
//! - `claude_job_list` -- enumerate every job at the configured root.
//! - `claude_job_get { short_id }` -- one job's full record incl.
//!   parsed `timeline.jsonl` and the raw `state.json` value.
//!
//! Resources:
//! - `claude://jobs` -- same shape as `claude_job_list`.
//!
//! Resource templates:
//! - `claude://jobs/{short_id}` -- full record for one job.
//!
//! Read-only by design. The dispatch protocol is undocumented and
//! version-sensitive; mirroring it would defeat the drift defenses
//! we built. Hosts wanting to fire background work should keep
//! using the agents TUI or the wrapper's DuplexSession machinery.

use std::collections::HashMap;

use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::json;
use tower_mcp::protocol::ReadResourceResult;
use tower_mcp::resource::ResourceTemplate;
use tower_mcp::{
    CallToolResult, Resource, ResourceBuilder, ResourceTemplateBuilder, Tool, ToolBuilder,
};

use claude_wrapper::jobs::JobsRoot;

use crate::state::ServerState;

/// Build the jobs-feature tool list.
pub(crate) fn tools(state: &ServerState) -> Vec<Tool> {
    vec![tool_job_list(state), tool_job_get(state)]
}

/// Build the jobs-feature resource list.
pub(crate) fn resources(state: &ServerState) -> Vec<Resource> {
    vec![resource_jobs(state)]
}

/// Build the jobs-feature resource templates.
pub(crate) fn templates(state: &ServerState) -> Vec<ResourceTemplate> {
    vec![template_job_detail(state)]
}

// -- tool_job_list -------------------------------------------------

#[derive(Debug, Default, Deserialize, JsonSchema)]
struct NoArgs {}

fn tool_job_list(state: &ServerState) -> Tool {
    let state = state.clone();
    ToolBuilder::new("claude_job_list")
        .description(
            "Enumerate background jobs under `~/.claude/jobs/`. Each \
             entry: `short_id` (canonical lookup handle), `state` \
             (`running | done | killed | failed | ...`), `intent` \
             (original prompt), `name` (auto-generated short title), \
             `session_id`, `session_path` (cross-link to the session \
             JSONL the history feature parses), `cwd`, timestamps. \
             Sorted by short_id. Read-only.",
        )
        .read_only()
        .handler(move |_input: NoArgs| {
            let state = state.clone();
            async move {
                let root = jobs_root(&state).map_err(crate::errors::from_wrapper)?;
                let summaries = root.list().map_err(crate::errors::from_wrapper)?;
                Ok(CallToolResult::json(json!({
                    "jobs": summaries
                        .iter()
                        .map(summary_to_json)
                        .collect::<Vec<_>>(),
                })))
            }
        })
        .build()
}

// -- tool_job_get --------------------------------------------------

#[derive(Debug, Deserialize, JsonSchema)]
struct JobGetInput {
    /// Short id (the directory name under `~/.claude/jobs/`,
    /// e.g. `90c961c7`).
    short_id: String,
}

fn tool_job_get(state: &ServerState) -> Tool {
    let state = state.clone();
    ToolBuilder::new("claude_job_get")
        .description(
            "Read one job's full record by `short_id`. Returns summary \
             metadata plus the parsed timeline (state transitions with \
             timestamps and text bodies) and the raw `state.json` value \
             (`raw_state`) for fields not in the typed summary. Errors \
             if no job with that id exists. Read-only.",
        )
        .read_only()
        .handler(move |input: JobGetInput| {
            let state = state.clone();
            async move {
                let root = jobs_root(&state).map_err(crate::errors::from_wrapper)?;
                let job = root
                    .get(&input.short_id)
                    .map_err(crate::errors::from_wrapper)?;
                Ok(CallToolResult::json(job_to_json(&job)))
            }
        })
        .build()
}

// -- resource: claude://jobs ---------------------------------------

fn resource_jobs(state: &ServerState) -> Resource {
    let state = state.clone();
    ResourceBuilder::new("claude://jobs")
        .name("Background jobs")
        .description(
            "Live view of every background job at the configured jobs \
             root. Same shape as the claude_job_list tool.",
        )
        .mime_type("application/json")
        .handler(move || {
            let state = state.clone();
            async move {
                let root = jobs_root(&state).map_err(crate::errors::from_wrapper)?;
                let summaries = root.list().map_err(crate::errors::from_wrapper)?;
                let body = json!({
                    "jobs": summaries
                        .iter()
                        .map(summary_to_json)
                        .collect::<Vec<_>>(),
                });
                let text = serde_json::to_string_pretty(&body).unwrap_or_default();
                Ok(ReadResourceResult::text("claude://jobs", text))
            }
        })
        .build()
}

// -- template: claude://jobs/{short_id} ----------------------------

fn template_job_detail(state: &ServerState) -> ResourceTemplate {
    let state = state.clone();
    ResourceTemplateBuilder::new("claude://jobs/{short_id}")
        .name("Job detail")
        .description(
            "Full job record keyed by short id. Same shape as the \
             claude_job_get tool.",
        )
        .mime_type("application/json")
        .handler(move |uri: String, vars: HashMap<String, String>| {
            let state = state.clone();
            async move {
                let id = vars.get("short_id").cloned().unwrap_or_default();
                let root = jobs_root(&state).map_err(crate::errors::from_wrapper)?;
                let job = root.get(&id).map_err(crate::errors::from_wrapper)?;
                let text = serde_json::to_string_pretty(&job_to_json(&job)).unwrap_or_default();
                Ok(ReadResourceResult::text(uri, text))
            }
        })
}

// -- helpers --------------------------------------------------------

fn jobs_root(state: &ServerState) -> claude_wrapper::error::Result<JobsRoot> {
    match state.config.jobs_root.as_ref() {
        Some(path) => Ok(JobsRoot::at(path.clone())),
        None => JobsRoot::home(),
    }
}

fn summary_to_json(s: &claude_wrapper::jobs::JobSummary) -> serde_json::Value {
    json!({
        "short_id": s.short_id,
        "state": s.state,
        "daemon_short": s.daemon_short,
        "backend": s.backend,
        "name": s.name,
        "detail": s.detail,
        "intent": s.intent,
        "session_id": s.session_id,
        "session_path": s.session_path,
        "cwd": s.cwd,
        "origin_cwd": s.origin_cwd,
        "created_at": s.created_at,
        "updated_at": s.updated_at,
        "first_terminal_at": s.first_terminal_at,
        "cli_version": s.cli_version,
        "state_mtime_secs": s.state_mtime_secs,
    })
}

fn job_to_json(job: &claude_wrapper::jobs::Job) -> serde_json::Value {
    json!({
        "summary": summary_to_json(&job.summary),
        "timeline": job
            .timeline
            .iter()
            .map(|e| json!({
                "at": e.at,
                "state": e.state,
                "detail": e.detail,
                "text": e.text,
                "extra": e.extra,
            }))
            .collect::<Vec<_>>(),
        "raw_state": job.raw_state,
    })
}
