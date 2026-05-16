//! L2 CLI passthrough tools (`claude_*`).
//!
//! Each tool is a thin wrapper around a [`claude_wrapper`] command
//! builder. Inputs map to builder options, outputs are returned as
//! JSON. Single-shot single-prompt work goes through [`tool_query`];
//! conversational/multi-turn work belongs in the `chat_*` family
//! (L2.5, lands later).
//!
//! Convention: tool names use snake_case with `claude_` prefix --
//! `claude_mcp_list`, `claude_plugin_validate`, etc. The CLI command
//! `claude foo bar baz` maps to `claude_foo_bar_baz`.

#[cfg(feature = "sync-agent-turns")]
use std::sync::Arc;

use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::json;
use tower_mcp::{CallToolResult, Tool, ToolBuilder};
use tracing::Instrument;

use claude_wrapper::ClaudeCommand;
use claude_wrapper::OutputFormat;
use claude_wrapper::QueryCommand;
use claude_wrapper::auth;
use claude_wrapper::command::agents::AgentsCommand;
use claude_wrapper::command::auth::AuthStatusCommand;
use claude_wrapper::command::auto_mode::{
    AutoModeConfigCommand, AutoModeCritiqueCommand, AutoModeDefaultsCommand,
};
use claude_wrapper::command::doctor::DoctorCommand;
use claude_wrapper::command::marketplace::MarketplaceListCommand;
use claude_wrapper::command::mcp::{McpGetCommand, McpListCommand};
use claude_wrapper::command::plugin::{PluginListCommand, PluginValidateCommand};
use claude_wrapper::exec::CommandOutput;

use crate::state::ServerState;

/// Build the full L2 passthrough tool list.
pub(crate) fn tools(state: &ServerState) -> Vec<Tool> {
    #[cfg_attr(not(feature = "sync-agent-turns"), allow(unused_mut))]
    let mut out = vec![
        tool_version(),
        tool_cli_version(state),
        tool_query(state),
        tool_agents(state),
        tool_auth_status(state),
        tool_auth_strategy(),
        tool_mcp_list(state),
        tool_mcp_get(state),
        tool_plugin_list(state),
        tool_plugin_validate(state),
        tool_marketplace_list(state),
        tool_auto_mode_config(state),
        tool_auto_mode_defaults(state),
        tool_auto_mode_critique(state),
        tool_doctor(state),
    ];
    #[cfg(feature = "sync-agent-turns")]
    {
        out.push(tool_query_sync(state));
    }
    out
}

// -- shared inputs ---------------------------------------------------

#[derive(Debug, Default, Deserialize, JsonSchema)]
struct NoArgs {}

// -- claude_version --------------------------------------------------

fn tool_version() -> Tool {
    ToolBuilder::new("claude_version")
        .description("Return the claude-server crate version.")
        .read_only()
        .handler(|_input: NoArgs| async move {
            Ok(CallToolResult::json(json!({
                "name": env!("CARGO_PKG_NAME"),
                "version": env!("CARGO_PKG_VERSION"),
            })))
        })
        .build()
}

// -- claude_cli_version ---------------------------------------------

fn tool_cli_version(state: &ServerState) -> Tool {
    let claude = state.claude.clone();
    ToolBuilder::new("claude_cli_version")
        .description("Return the version of the underlying `claude` CLI binary.")
        .read_only()
        .handler(move |_input: NoArgs| {
            let claude = claude.clone();
            async move {
                let v = claude.cli_version().await.map_err(from_wrapper)?;
                Ok(CallToolResult::json(json!({
                    "major": v.major,
                    "minor": v.minor,
                    "patch": v.patch,
                    "raw": v.to_string(),
                })))
            }
        })
        .build()
}

// -- claude_query / claude_query_sync -------------------------------

#[derive(Debug, Deserialize, JsonSchema)]
struct QueryInput {
    /// The prompt to send to claude.
    prompt: String,
    /// Optional model (e.g. `sonnet`, `haiku`).
    #[serde(default)]
    model: Option<String>,
    /// Optional system prompt.
    #[serde(default)]
    system_prompt: Option<String>,
    /// Resume a previous session by id.
    #[serde(default)]
    resume: Option<String>,
    /// Run the query in a fresh git worktree (`claude --worktree`).
    /// Useful for "agent runs in isolation" -- the query's writes
    /// land in a side worktree instead of the current working tree.
    /// Named worktrees aren't yet supported on the single-shot path
    /// (wrapper limitation; see chat_open for the named variant).
    #[serde(default)]
    worktree: Option<bool>,
}

fn build_query(input: &QueryInput) -> QueryCommand {
    // output_format on the builder lands BEFORE the `--` separator,
    // unlike execute_json's late push -- wrapper bug worked around.
    let mut q = QueryCommand::new(input.prompt.clone()).output_format(OutputFormat::Json);
    if let Some(ref m) = input.model {
        q = q.model(m.clone());
    }
    if let Some(ref s) = input.system_prompt {
        q = q.system_prompt(s.clone());
    }
    if let Some(ref r) = input.resume {
        q = q.resume(r.clone());
    }
    if input.worktree.unwrap_or(false) {
        q = q.worktree();
    }
    q
}

fn parse_query_envelope(stdout: &str) -> Result<serde_json::Value, tower_mcp::Error> {
    let parsed: serde_json::Value = serde_json::from_str(stdout)
        .map_err(|e| internal(format!("parse query JSON: {e}; stdout={stdout}")))?;
    Ok(json!({
        "result": parsed.get("result").cloned().unwrap_or(serde_json::Value::Null),
        "session_id": parsed.get("session_id").cloned().unwrap_or(serde_json::Value::Null),
        "total_cost_usd": parsed.get("total_cost_usd").cloned().unwrap_or(serde_json::Value::Null),
        "duration_ms": parsed.get("duration_ms").cloned().unwrap_or(serde_json::Value::Null),
        "num_turns": parsed.get("num_turns").cloned().unwrap_or(serde_json::Value::Null),
        "is_error": parsed.get("is_error").cloned().unwrap_or(serde_json::Value::Null),
    }))
}

fn tool_query(state: &ServerState) -> Tool {
    let state = state.clone();
    ToolBuilder::new("claude_query")
        .description(
            "Fire a single-shot agent query and return immediately with a \
             turn_id. The query runs in the background; poll with `turn_get`, \
             block with `turn_wait`, cancel with `turn_cancel`. For the \
             blocking variant, see `claude_query_sync`.",
        )
        .handler(move |input: QueryInput| {
            let state = state.clone();
            async move {
                let handle = state.turns.register(None).await;
                let turn_id = handle.turn_id.clone();
                let claude = state.claude.clone();
                let span = tracing::info_span!(
                    "claude_query",
                    turn_id = %turn_id,
                    model = input.model.as_deref().unwrap_or("default"),
                    prompt_len = input.prompt.len(),
                );
                tracing::info!(parent: &span, "fired async query");
                tokio::spawn(
                    async move {
                        if handle.is_cancelled() {
                            tracing::info!("query cancelled before start");
                            handle.cancelled();
                            return;
                        }
                        let q = build_query(&input);
                        match q.execute(&claude).await {
                            Ok(out) => match parse_query_envelope(&out.stdout) {
                                Ok(env) => {
                                    let cost = env.get("total_cost_usd").and_then(|v| v.as_f64());
                                    let dur = env.get("duration_ms").and_then(|v| v.as_u64());
                                    tracing::info!(
                                        cost_usd = ?cost,
                                        duration_ms = ?dur,
                                        "query done"
                                    );
                                    handle.complete(env);
                                }
                                Err(e) => {
                                    tracing::error!(error = %e, "parse failed");
                                    handle.fail(e.to_string());
                                }
                            },
                            Err(e) => {
                                tracing::error!(error = %e, "query failed");
                                handle.fail(e);
                            }
                        }
                    }
                    .instrument(span),
                );
                Ok(CallToolResult::json(json!({"turn_id": turn_id})))
            }
        })
        .build()
}

#[cfg(feature = "sync-agent-turns")]
fn tool_query_sync(state: &ServerState) -> Tool {
    let claude = state.claude.clone();
    ToolBuilder::new("claude_query_sync")
        .description(
            "Blocking single-shot query against the claude CLI. Holds the \
             connection open for the duration of the turn. Returns assistant \
             text, session id, and cost. Prefer the async `claude_query` for \
             agent turns; use this when you genuinely want to block.",
        )
        .read_only()
        .handler(move |input: QueryInput| {
            let claude = Arc::clone(&claude);
            async move {
                let q = build_query(&input);
                let out = q.execute(&claude).await.map_err(from_wrapper)?;
                let env = parse_query_envelope(&out.stdout)?;
                Ok(CallToolResult::json(env))
            }
        })
        .build()
}

// -- claude_agents ---------------------------------------------------

fn tool_agents(state: &ServerState) -> Tool {
    let claude = state.claude.clone();
    ToolBuilder::new("claude_agents")
        .description("Run `claude agents` to list configured agents.")
        .read_only()
        .handler(move |_input: NoArgs| {
            let claude = claude.clone();
            async move {
                let out = AgentsCommand::new()
                    .execute(&claude)
                    .await
                    .map_err(from_wrapper)?;
                Ok(command_output_json(&out))
            }
        })
        .build()
}

// -- claude_auth_status ---------------------------------------------

fn tool_auth_status(state: &ServerState) -> Tool {
    let claude = state.claude.clone();
    ToolBuilder::new("claude_auth_status")
        .description("Run `claude auth status --json` and return the parsed status.")
        .read_only()
        .handler(move |_input: NoArgs| {
            let claude = claude.clone();
            async move {
                let parsed = AuthStatusCommand::new()
                    .execute_json(&claude)
                    .await
                    .map_err(from_wrapper)?;
                Ok(CallToolResult::json(
                    serde_json::to_value(parsed).unwrap_or(json!(null)),
                ))
            }
        })
        .build()
}

// -- claude_auth_strategy --------------------------------------------

fn tool_auth_strategy() -> Tool {
    ToolBuilder::new("claude_auth_strategy")
        .description(
            "Report which auth strategy the embedded `claude` CLI will use \
             given the current process environment. Cheap; no subprocess. \
             Returns `{ strategy, has_anthropic_api_key, has_oauth_token, \
             bedrock_enabled, vertex_enabled }`. `strategy` is one of \
             `bedrock | vertex | api_key | oauth_token | subscription`. \
             `subscription` means no env auth set -- the CLI will fall back \
             to credentials stored under `~/.claude/`; for liveness use \
             `claude_auth_status`.",
        )
        .read_only()
        .handler(|_input: NoArgs| async move {
            let summary = auth::detect();
            Ok(CallToolResult::json(
                serde_json::to_value(&summary).unwrap_or(json!(null)),
            ))
        })
        .build()
}

// -- claude_mcp_list -------------------------------------------------

fn tool_mcp_list(state: &ServerState) -> Tool {
    let claude = state.claude.clone();
    ToolBuilder::new("claude_mcp_list")
        .description("Run `claude mcp list` to list configured MCP servers.")
        .read_only()
        .handler(move |_input: NoArgs| {
            let claude = claude.clone();
            async move {
                let out = McpListCommand::new()
                    .execute(&claude)
                    .await
                    .map_err(from_wrapper)?;
                Ok(command_output_json(&out))
            }
        })
        .build()
}

// -- claude_mcp_get --------------------------------------------------

#[derive(Debug, Deserialize, JsonSchema)]
struct McpGetInput {
    /// Name of the configured MCP server to inspect.
    name: String,
}

fn tool_mcp_get(state: &ServerState) -> Tool {
    let claude = state.claude.clone();
    ToolBuilder::new("claude_mcp_get")
        .description("Run `claude mcp get <name>` to inspect a single MCP server.")
        .read_only()
        .handler(move |input: McpGetInput| {
            let claude = claude.clone();
            async move {
                let out = McpGetCommand::new(input.name)
                    .execute(&claude)
                    .await
                    .map_err(from_wrapper)?;
                Ok(command_output_json(&out))
            }
        })
        .build()
}

// -- claude_plugin_list ---------------------------------------------

fn tool_plugin_list(state: &ServerState) -> Tool {
    let claude = state.claude.clone();
    ToolBuilder::new("claude_plugin_list")
        .description("Run `claude plugin list` to list installed plugins.")
        .read_only()
        .handler(move |_input: NoArgs| {
            let claude = claude.clone();
            async move {
                let out = PluginListCommand::new()
                    .execute(&claude)
                    .await
                    .map_err(from_wrapper)?;
                Ok(command_output_json(&out))
            }
        })
        .build()
}

// -- claude_plugin_validate -----------------------------------------

#[derive(Debug, Deserialize, JsonSchema)]
struct PluginValidateInput {
    /// Path to the plugin directory to validate.
    path: String,
}

fn tool_plugin_validate(state: &ServerState) -> Tool {
    let claude = state.claude.clone();
    ToolBuilder::new("claude_plugin_validate")
        .description("Run `claude plugin validate <path>` to check a plugin's manifest.")
        .read_only()
        .handler(move |input: PluginValidateInput| {
            let claude = claude.clone();
            async move {
                let out = PluginValidateCommand::new(input.path)
                    .execute(&claude)
                    .await
                    .map_err(from_wrapper)?;
                Ok(command_output_json(&out))
            }
        })
        .build()
}

// -- claude_marketplace_list -----------------------------------------

fn tool_marketplace_list(state: &ServerState) -> Tool {
    let claude = state.claude.clone();
    ToolBuilder::new("claude_marketplace_list")
        .description("Run `claude plugin marketplace list` to list known plugin marketplaces.")
        .read_only()
        .handler(move |_input: NoArgs| {
            let claude = claude.clone();
            async move {
                let out = MarketplaceListCommand::new()
                    .execute(&claude)
                    .await
                    .map_err(from_wrapper)?;
                Ok(command_output_json(&out))
            }
        })
        .build()
}

// -- claude_auto_mode_config / defaults -----------------------------

fn tool_auto_mode_config(state: &ServerState) -> Tool {
    let claude = state.claude.clone();
    ToolBuilder::new("claude_auto_mode_config")
        .description("Run `claude auto-mode config` to dump the effective auto-mode config.")
        .read_only()
        .handler(move |_input: NoArgs| {
            let claude = claude.clone();
            async move {
                let out = AutoModeConfigCommand::new()
                    .execute(&claude)
                    .await
                    .map_err(from_wrapper)?;
                Ok(command_output_json(&out))
            }
        })
        .build()
}

fn tool_auto_mode_defaults(state: &ServerState) -> Tool {
    let claude = state.claude.clone();
    ToolBuilder::new("claude_auto_mode_defaults")
        .description("Run `claude auto-mode defaults` to dump the built-in auto-mode rules.")
        .read_only()
        .handler(move |_input: NoArgs| {
            let claude = claude.clone();
            async move {
                let out = AutoModeDefaultsCommand::new()
                    .execute(&claude)
                    .await
                    .map_err(from_wrapper)?;
                Ok(command_output_json(&out))
            }
        })
        .build()
}

// -- claude_auto_mode_critique --------------------------------------

#[derive(Debug, Default, Deserialize, JsonSchema)]
struct AutoModeCritiqueInput {
    /// Optional model override (e.g. `sonnet`, `haiku`).
    #[serde(default)]
    model: Option<String>,
}

fn tool_auto_mode_critique(state: &ServerState) -> Tool {
    let state = state.clone();
    ToolBuilder::new("claude_auto_mode_critique")
        .description(
            "Fire `claude auto-mode critique` to get AI feedback on the active \
             auto-mode rules. Async by default -- returns a turn_id; the work \
             runs in the background. Poll with turn_get / turn_wait. Single-shot \
             agent turn (model-backed); same shape as claude_query.",
        )
        .handler(move |input: AutoModeCritiqueInput| {
            let state = state.clone();
            async move {
                let handle = state.turns.register(None).await;
                let turn_id = handle.turn_id.clone();
                let claude = state.claude.clone();
                let span = tracing::info_span!(
                    "claude_auto_mode_critique",
                    turn_id = %turn_id,
                    model = input.model.as_deref().unwrap_or("default"),
                );
                tracing::info!(parent: &span, "fired async critique");
                tokio::spawn(
                    async move {
                        if handle.is_cancelled() {
                            handle.cancelled();
                            return;
                        }
                        let mut cmd = AutoModeCritiqueCommand::new();
                        if let Some(m) = input.model {
                            cmd = cmd.model(m);
                        }
                        match cmd.execute(&claude).await {
                            Ok(out) => {
                                tracing::info!(exit_code = out.exit_code, "critique done");
                                handle.complete(json!({
                                    "stdout": strip_ansi(&out.stdout),
                                    "stderr": strip_ansi(&out.stderr),
                                    "exit_code": out.exit_code,
                                    "success": out.success,
                                }));
                            }
                            Err(e) => {
                                tracing::error!(error = %e, "critique failed");
                                handle.fail(e);
                            }
                        }
                    }
                    .instrument(span),
                );
                Ok(CallToolResult::json(json!({"turn_id": turn_id})))
            }
        })
        .build()
}

// -- claude_doctor --------------------------------------------------

fn tool_doctor(state: &ServerState) -> Tool {
    let claude = state.claude.clone();
    ToolBuilder::new("claude_doctor")
        .description("Run `claude doctor` for a CLI health check. Can be slow (3+ minutes).")
        .read_only()
        .handler(move |_input: NoArgs| {
            let claude = claude.clone();
            async move {
                let out = DoctorCommand::new()
                    .execute(&claude)
                    .await
                    .map_err(from_wrapper)?;
                Ok(command_output_json(&out))
            }
        })
        .build()
}

// -- helpers --------------------------------------------------------

use crate::errors::{from_wrapper, internal};

/// Wrap a [`CommandOutput`] as the standard tool JSON envelope.
/// ANSI escape sequences are stripped from stdout/stderr.
fn command_output_json(out: &CommandOutput) -> CallToolResult {
    CallToolResult::json(json!({
        "stdout": strip_ansi(&out.stdout),
        "stderr": strip_ansi(&out.stderr),
        "exit_code": out.exit_code,
        "success": out.success,
    }))
}

/// Strip ANSI/VT escape sequences from a string.
///
/// Handles the common CSI/OSC/DCS/PM/APC families plus the standalone
/// two-byte sequences (`ESC c`, `ESC =`, etc.). Conservative -- if we
/// see an `ESC` we don't recognise, we still drop it so users don't
/// end up with stray control bytes in their MCP payloads.
pub(crate) fn strip_ansi(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let bytes = input.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        let b = bytes[i];
        if b == 0x1b {
            // ESC. Look at the next byte to decide which family.
            if i + 1 >= bytes.len() {
                i += 1;
                continue;
            }
            match bytes[i + 1] {
                b'[' => {
                    // CSI: ESC [ params final-byte
                    i += 2;
                    while i < bytes.len() && !(0x40..=0x7e).contains(&bytes[i]) {
                        i += 1;
                    }
                    if i < bytes.len() {
                        i += 1;
                    }
                }
                b']' | b'P' | b'^' | b'_' => {
                    // OSC/DCS/PM/APC terminate on BEL or ST (ESC \).
                    i += 2;
                    while i < bytes.len() {
                        if bytes[i] == 0x07 {
                            i += 1;
                            break;
                        }
                        if bytes[i] == 0x1b && i + 1 < bytes.len() && bytes[i + 1] == b'\\' {
                            i += 2;
                            break;
                        }
                        i += 1;
                    }
                }
                _ => {
                    // Two-byte standalone (ESC c, ESC =, etc.).
                    i += 2;
                }
            }
        } else {
            out.push(b as char);
            i += 1;
        }
    }
    out
}

#[cfg(test)]
mod strip_ansi_tests {
    use super::strip_ansi;

    #[test]
    fn strips_csi_color_codes() {
        let s = "\x1b[31mhello\x1b[0m world";
        assert_eq!(strip_ansi(s), "hello world");
    }

    #[test]
    fn strips_osc_title() {
        let s = "\x1b]0;title\x07hello";
        assert_eq!(strip_ansi(s), "hello");
    }

    #[test]
    fn passes_plain_text() {
        assert_eq!(strip_ansi("plain text"), "plain text");
    }

    #[test]
    fn strips_csi_with_params() {
        let s = "\x1b[1;32;40mbright\x1b[m";
        assert_eq!(strip_ansi(s), "bright");
    }

    #[test]
    fn handles_trailing_esc() {
        let s = "ok\x1b";
        assert_eq!(strip_ansi(s), "ok");
    }

    #[test]
    fn strips_st_terminated_osc() {
        let s = "\x1b]8;;https://x\x1b\\link\x1b]8;;\x1b\\done";
        assert_eq!(strip_ansi(s), "linkdone");
    }
}
