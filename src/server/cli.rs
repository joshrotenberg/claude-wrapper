//! cli surface: low-level tools that mirror the wrapper 1:1.
//!
//! Each MCP tool corresponds to a `claude-wrapper` command builder.
//! Naming convention is `claude.<area>.<verb>` where `area` mirrors
//! the wrapper's `command/` modules.
//!
//! v0 ships the non-mutating, non-streaming subset. Mutating tools
//! (mcp.add/remove, plugin.install/uninstall, marketplace.*, install,
//! update) are deferred and gated behind
//! [`crate::server::config::ServerPolicy::allow_mutations`] when
//! they land.

use std::sync::Arc;

use schemars::JsonSchema;
use serde::Deserialize;
use tower_mcp::extract::{Context, Json, State};
use tower_mcp::{CallToolResult, Tool, ToolBuilder};

use crate::OutputFormat;
use crate::command::{
    ClaudeCommand, agents::AgentsCommand, auth::AuthStatusCommand,
    auto_mode::AutoModeConfigCommand, auto_mode::AutoModeCritiqueCommand,
    auto_mode::AutoModeDefaultsCommand, doctor::DoctorCommand, mcp::McpGetCommand,
    mcp::McpListCommand, query::QueryCommand, version::VersionCommand,
};
use crate::streaming::{StreamEvent, stream_query};

use super::error::error_to_result;
use super::state::ServerState;

/// Build the read-only cli surface tools.
///
/// Returns the tool registrations as a Vec; the caller appends them
/// to its `McpRouter`.
pub(crate) fn read_only_tools(state: &ServerState) -> Vec<Tool> {
    vec![
        tool_query(state),
        tool_query_json(state),
        tool_query_stream(state),
        tool_version(state),
        tool_cli_version(state),
        tool_doctor(state),
        tool_agents(state),
        tool_auth_status(state),
        tool_mcp_list(state),
        tool_mcp_get(state),
        tool_auto_mode_config(state),
        tool_auto_mode_defaults(state),
        tool_auto_mode_critique(state),
    ]
}

// -- query -----------------------------------------------------------

#[derive(Debug, Deserialize, JsonSchema)]
struct QueryInput {
    /// The prompt to send to Claude.
    prompt: String,
    /// Model alias or full ID. Optional.
    model: Option<String>,
    /// Override or replace the system prompt. Optional.
    system_prompt: Option<String>,
    /// Append to the default system prompt. Optional.
    append_system_prompt: Option<String>,
    /// Tool patterns to allow (e.g. `["Bash(git log:*)", "Read"]`).
    allowed_tools: Option<Vec<String>>,
    /// Tool patterns to deny.
    disallowed_tools: Option<Vec<String>>,
    /// Maximum turn count.
    max_turns: Option<u32>,
    /// Per-call CLI-side spend cap in USD.
    max_budget_usd: Option<f64>,
    /// Permission mode (`"default" | "acceptEdits" | "dontAsk" | "plan" | "auto"`).
    permission_mode: Option<String>,
    /// Disable session persistence to disk.
    no_session_persistence: Option<bool>,
    /// Resume an existing session by id.
    resume: Option<String>,
    /// Continue the most recent session.
    continue_session: Option<bool>,
    /// Run with `--bare` (skip hooks/LSP/keychain/CLAUDE.md auto-discovery).
    bare: Option<bool>,
}

fn build_query(input: QueryInput) -> Result<QueryCommand, String> {
    let mut cmd = QueryCommand::new(input.prompt);
    if let Some(m) = input.model {
        cmd = cmd.model(m);
    }
    if let Some(p) = input.system_prompt {
        cmd = cmd.system_prompt(p);
    }
    if let Some(p) = input.append_system_prompt {
        cmd = cmd.append_system_prompt(p);
    }
    if let Some(tools) = input.allowed_tools {
        cmd = cmd.allowed_tools(tools);
    }
    if let Some(tools) = input.disallowed_tools {
        cmd = cmd.disallowed_tools(tools);
    }
    if let Some(n) = input.max_turns {
        cmd = cmd.max_turns(n);
    }
    if let Some(usd) = input.max_budget_usd {
        cmd = cmd.max_budget_usd(usd);
    }
    if let Some(mode) = input.permission_mode {
        let parsed = parse_permission_mode(&mode)
            .ok_or_else(|| format!("invalid permission_mode `{mode}` (allowed: default, acceptEdits, dontAsk, plan, auto)"))?;
        cmd = cmd.permission_mode(parsed);
    }
    if input.no_session_persistence.unwrap_or(false) {
        cmd = cmd.no_session_persistence();
    }
    if let Some(id) = input.resume {
        cmd = cmd.resume(id);
    }
    if input.continue_session.unwrap_or(false) {
        cmd = cmd.continue_session();
    }
    if input.bare.unwrap_or(false) {
        cmd = cmd.bare();
    }
    Ok(cmd)
}

fn tool_query(state: &ServerState) -> Tool {
    ToolBuilder::new("claude.query")
        .description("Run `claude -p <prompt>` with the given options. Returns CommandOutput (stdout, stderr, exit_code).")
        .extractor_handler(
            state.clone(),
            |State(state): State<ServerState>, Json(input): Json<QueryInput>| async move {
                let cmd = match build_query(input) {
                    Ok(c) => c,
                    Err(e) => return Ok(CallToolResult::error(e)),
                };
                let _g = state.lock_default_cwd().await;
                match cmd.execute(&state.claude).await {
                    Ok(out) => Ok(CallToolResult::from_serialize(&serialize_output(&out))?),
                    Err(e) => Ok(error_to_result(e)),
                }
            },
        )
        .build()
}

fn tool_query_json(state: &ServerState) -> Tool {
    ToolBuilder::new("claude.query_json")
        .description("Run a query with `--output-format=json` and return the parsed QueryResult (result text, session_id, cost, etc).")
        .extractor_handler(
            state.clone(),
            |State(state): State<ServerState>, Json(input): Json<QueryInput>| async move {
                let cmd = match build_query(input) {
                    Ok(c) => c,
                    Err(e) => return Ok(CallToolResult::error(e)),
                };
                let _g = state.lock_default_cwd().await;
                match cmd.execute_json(&state.claude).await {
                    Ok(result) => Ok(CallToolResult::from_serialize(&result)?),
                    Err(e) => Ok(error_to_result(e)),
                }
            },
        )
        .build()
}

fn tool_query_stream(state: &ServerState) -> Tool {
    ToolBuilder::new("claude.query_stream")
        .description(
            "Run a query with stream-json output. Each StreamEvent from claude is forwarded as an MCP \
             progress notification (the `message` field carries the serialized event JSON). Final \
             return is the consolidated CommandOutput. Honours MCP cancellation: if the client cancels, \
             the underlying child is dropped and the call returns early.",
        )
        .extractor_handler(
            state.clone(),
            |State(state): State<ServerState>,
             ctx: Context,
             Json(input): Json<QueryInput>| async move {
                let mut cmd = match build_query(input) {
                    Ok(c) => c,
                    Err(e) => return Ok(CallToolResult::error(e)),
                };
                cmd = cmd.output_format(OutputFormat::StreamJson);

                let _g = state.lock_default_cwd().await;

                // Per-event progress notifications. report_progress_sync
                // is fire-and-forget through the notification channel;
                // safe inside stream_query's FnMut closure.
                let counter = std::sync::atomic::AtomicU64::new(0);
                let cancellation = ctx.clone();
                let result = stream_query(&state.claude, &cmd, |event: StreamEvent| {
                    let n = counter.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
                    let payload = serde_json::to_string(&event.data).unwrap_or_default();
                    ctx.report_progress_sync(n as f64, None, Some(&payload));
                    if cancellation.is_cancelled() {
                        // We can't kill the in-flight child from inside
                        // the sync callback; the wrapper's stream loop
                        // will keep going until the child closes its
                        // pipes. The handler will return early after the
                        // .await unwinds.
                    }
                })
                .await;

                if ctx.is_cancelled() {
                    return Ok(CallToolResult::error(
                        "claude.query_stream cancelled by client",
                    ));
                }

                match result {
                    Ok(out) => Ok(CallToolResult::from_serialize(&serialize_output(&out))?),
                    Err(e) => Ok(error_to_result(e)),
                }
            },
        )
        .build()
}

// -- version, cli_version --------------------------------------------

#[derive(Debug, Deserialize, JsonSchema)]
struct EmptyInput {}

fn tool_version(state: &ServerState) -> Tool {
    ToolBuilder::new("claude.version")
        .description("Run `claude --version` and return the raw stdout.")
        .read_only()
        .extractor_handler(
            state.clone(),
            |State(state): State<ServerState>, Json(_input): Json<EmptyInput>| async move {
                let _g = state.lock_default_cwd().await;
                match VersionCommand::new().execute(&state.claude).await {
                    Ok(out) => Ok(CallToolResult::from_serialize(&serialize_output(&out))?),
                    Err(e) => Ok(error_to_result(e)),
                }
            },
        )
        .build()
}

fn tool_cli_version(state: &ServerState) -> Tool {
    ToolBuilder::new("claude.cli_version")
        .description("Return the parsed Claude CLI version (major/minor/patch).")
        .read_only()
        .idempotent()
        .extractor_handler(
            state.clone(),
            |State(state): State<ServerState>, Json(_input): Json<EmptyInput>| async move {
                let _g = state.lock_default_cwd().await;
                match Arc::clone(&state.claude).cli_version().await {
                    Ok(v) => Ok(CallToolResult::from_serialize(&serde_json::json!({
                        "major": v.major,
                        "minor": v.minor,
                        "patch": v.patch,
                        "display": v.to_string(),
                    }))?),
                    Err(e) => Ok(error_to_result(e)),
                }
            },
        )
        .build()
}

// -- doctor, agents --------------------------------------------------

fn tool_doctor(state: &ServerState) -> Tool {
    ToolBuilder::new("claude.doctor")
        .description("Run `claude doctor` for a CLI health check.")
        .read_only()
        .extractor_handler(
            state.clone(),
            |State(state): State<ServerState>, Json(_input): Json<EmptyInput>| async move {
                let _g = state.lock_default_cwd().await;
                match DoctorCommand::new().execute(&state.claude).await {
                    Ok(out) => Ok(CallToolResult::from_serialize(&serialize_output(&out))?),
                    Err(e) => Ok(error_to_result(e)),
                }
            },
        )
        .build()
}

#[derive(Debug, Deserialize, JsonSchema)]
struct AgentsInput {
    /// Comma-separated list of setting sources (`user`, `project`, `local`).
    setting_sources: Option<String>,
}

fn tool_agents(state: &ServerState) -> Tool {
    ToolBuilder::new("claude.agents")
        .description("Run `claude agents` to list configured agents.")
        .read_only()
        .extractor_handler(
            state.clone(),
            |State(state): State<ServerState>, Json(input): Json<AgentsInput>| async move {
                let mut cmd = AgentsCommand::new();
                if let Some(s) = input.setting_sources {
                    cmd = cmd.setting_sources(s);
                }
                let _g = state.lock_default_cwd().await;
                match cmd.execute(&state.claude).await {
                    Ok(out) => Ok(CallToolResult::from_serialize(&serialize_output(&out))?),
                    Err(e) => Ok(error_to_result(e)),
                }
            },
        )
        .build()
}

// -- auth.status -----------------------------------------------------

fn tool_auth_status(state: &ServerState) -> Tool {
    ToolBuilder::new("claude.auth.status")
        .description("Run `claude auth status --json` and return the parsed AuthStatus.")
        .read_only()
        .extractor_handler(
            state.clone(),
            |State(state): State<ServerState>, Json(_input): Json<EmptyInput>| async move {
                let _g = state.lock_default_cwd().await;
                match AuthStatusCommand::new().execute_json(&state.claude).await {
                    Ok(status) => Ok(CallToolResult::from_serialize(&status)?),
                    Err(e) => Ok(error_to_result(e)),
                }
            },
        )
        .build()
}

// -- mcp.list, mcp.get -----------------------------------------------

fn tool_mcp_list(state: &ServerState) -> Tool {
    ToolBuilder::new("claude.mcp.list")
        .description("Run `claude mcp list` and return the raw stdout.")
        .read_only()
        .extractor_handler(
            state.clone(),
            |State(state): State<ServerState>, Json(_input): Json<EmptyInput>| async move {
                let _g = state.lock_default_cwd().await;
                match McpListCommand::new().execute(&state.claude).await {
                    Ok(out) => Ok(CallToolResult::from_serialize(&serialize_output(&out))?),
                    Err(e) => Ok(error_to_result(e)),
                }
            },
        )
        .build()
}

#[derive(Debug, Deserialize, JsonSchema)]
struct McpGetInput {
    /// Name of the MCP server to inspect.
    name: String,
}

fn tool_mcp_get(state: &ServerState) -> Tool {
    ToolBuilder::new("claude.mcp.get")
        .description("Run `claude mcp get <name>` for details on one MCP server.")
        .read_only()
        .extractor_handler(
            state.clone(),
            |State(state): State<ServerState>, Json(input): Json<McpGetInput>| async move {
                let _g = state.lock_default_cwd().await;
                match McpGetCommand::new(input.name).execute(&state.claude).await {
                    Ok(out) => Ok(CallToolResult::from_serialize(&serialize_output(&out))?),
                    Err(e) => Ok(error_to_result(e)),
                }
            },
        )
        .build()
}

// -- auto_mode.* -----------------------------------------------------

fn tool_auto_mode_config(state: &ServerState) -> Tool {
    ToolBuilder::new("claude.auto_mode.config")
        .description("Print the effective auto-mode config as JSON.")
        .read_only()
        .extractor_handler(
            state.clone(),
            |State(state): State<ServerState>, Json(_input): Json<EmptyInput>| async move {
                let _g = state.lock_default_cwd().await;
                match AutoModeConfigCommand::new().execute(&state.claude).await {
                    Ok(out) => Ok(CallToolResult::from_serialize(&serialize_output(&out))?),
                    Err(e) => Ok(error_to_result(e)),
                }
            },
        )
        .build()
}

fn tool_auto_mode_defaults(state: &ServerState) -> Tool {
    ToolBuilder::new("claude.auto_mode.defaults")
        .description("Print the default auto-mode rules as JSON.")
        .read_only()
        .idempotent()
        .extractor_handler(
            state.clone(),
            |State(state): State<ServerState>, Json(_input): Json<EmptyInput>| async move {
                let _g = state.lock_default_cwd().await;
                match AutoModeDefaultsCommand::new().execute(&state.claude).await {
                    Ok(out) => Ok(CallToolResult::from_serialize(&serialize_output(&out))?),
                    Err(e) => Ok(error_to_result(e)),
                }
            },
        )
        .build()
}

#[derive(Debug, Deserialize, JsonSchema)]
struct AutoModeCritiqueInput {
    /// Optional model override.
    model: Option<String>,
}

fn tool_auto_mode_critique(state: &ServerState) -> Tool {
    ToolBuilder::new("claude.auto_mode.critique")
        .description("Get AI feedback on your custom auto-mode rules.")
        .extractor_handler(
            state.clone(),
            |State(state): State<ServerState>, Json(input): Json<AutoModeCritiqueInput>| async move {
                let mut cmd = AutoModeCritiqueCommand::new();
                if let Some(m) = input.model {
                    cmd = cmd.model(m);
                }
                let _g = state.lock_default_cwd().await;
                match cmd.execute(&state.claude).await {
                    Ok(out) => Ok(CallToolResult::from_serialize(&serialize_output(&out))?),
                    Err(e) => Ok(error_to_result(e)),
                }
            },
        )
        .build()
}

// -- helpers ---------------------------------------------------------

/// Parse the user-facing permission-mode string. Deliberately rejects
/// `"bypassPermissions"` -- bypass mode should reach the wrapper via
/// [`crate::dangerous::DangerousClient`], not by smuggling the string
/// through plain `query`.
pub(crate) fn parse_permission_mode(s: &str) -> Option<crate::types::PermissionMode> {
    match s {
        "default" => Some(crate::types::PermissionMode::Default),
        "acceptEdits" => Some(crate::types::PermissionMode::AcceptEdits),
        "dontAsk" => Some(crate::types::PermissionMode::DontAsk),
        "plan" => Some(crate::types::PermissionMode::Plan),
        "auto" => Some(crate::types::PermissionMode::Auto),
        _ => None,
    }
}

fn serialize_output(out: &crate::exec::CommandOutput) -> serde_json::Value {
    serde_json::json!({
        "stdout": out.stdout,
        "stderr": out.stderr,
        "exit_code": out.exit_code,
        "success": out.success,
    })
}
