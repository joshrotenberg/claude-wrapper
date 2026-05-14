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

use std::sync::Arc;

use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::json;
use tower_mcp::{CallToolResult, Tool, ToolBuilder};

use claude_wrapper::ClaudeCommand;
use claude_wrapper::OutputFormat;
use claude_wrapper::QueryCommand;
use claude_wrapper::command::agents::AgentsCommand;
use claude_wrapper::command::auth::AuthStatusCommand;
use claude_wrapper::command::auto_mode::{AutoModeConfigCommand, AutoModeDefaultsCommand};
use claude_wrapper::command::doctor::DoctorCommand;
use claude_wrapper::command::marketplace::MarketplaceListCommand;
use claude_wrapper::command::mcp::{McpGetCommand, McpListCommand};
use claude_wrapper::command::plugin::{PluginListCommand, PluginValidateCommand};
use claude_wrapper::exec::CommandOutput;

use crate::state::ServerState;

/// Build the full L2 passthrough tool list.
pub(crate) fn tools(state: &ServerState) -> Vec<Tool> {
    vec![
        tool_version(),
        tool_cli_version(state),
        tool_query_sync(state),
        tool_agents(state),
        tool_auth_status(state),
        tool_mcp_list(state),
        tool_mcp_get(state),
        tool_plugin_list(state),
        tool_plugin_validate(state),
        tool_marketplace_list(state),
        tool_auto_mode_config(state),
        tool_auto_mode_defaults(state),
        tool_doctor(state),
    ]
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
                let v = claude.cli_version().await.map_err(internal)?;
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

// -- claude_query ----------------------------------------------------

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
}

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
                // Setting output_format on the builder ensures it lands
                // before the `--` separator. (execute_json appends it
                // after, where it gets eaten as prompt text -- wrapper
                // bug, worked around here.)
                let mut q = QueryCommand::new(input.prompt).output_format(OutputFormat::Json);
                if let Some(m) = input.model {
                    q = q.model(m);
                }
                if let Some(s) = input.system_prompt {
                    q = q.system_prompt(s);
                }
                if let Some(r) = input.resume {
                    q = q.resume(r);
                }
                let out = q.execute(&claude).await.map_err(internal)?;
                let parsed: serde_json::Value = serde_json::from_str(&out.stdout)
                    .map_err(|e| internal(format!("parse query JSON: {e}; stdout={}", out.stdout)))?;
                Ok(CallToolResult::json(json!({
                    "result": parsed.get("result").cloned().unwrap_or(serde_json::Value::Null),
                    "session_id": parsed.get("session_id").cloned().unwrap_or(serde_json::Value::Null),
                    "total_cost_usd": parsed.get("total_cost_usd").cloned().unwrap_or(serde_json::Value::Null),
                    "duration_ms": parsed.get("duration_ms").cloned().unwrap_or(serde_json::Value::Null),
                    "num_turns": parsed.get("num_turns").cloned().unwrap_or(serde_json::Value::Null),
                    "is_error": parsed.get("is_error").cloned().unwrap_or(serde_json::Value::Null),
                })))
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
                    .map_err(internal)?;
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
                    .map_err(internal)?;
                Ok(CallToolResult::json(
                    serde_json::to_value(parsed).unwrap_or(json!(null)),
                ))
            }
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
                    .map_err(internal)?;
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
                    .map_err(internal)?;
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
                    .map_err(internal)?;
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
                    .map_err(internal)?;
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
                    .map_err(internal)?;
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
                    .map_err(internal)?;
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
                    .map_err(internal)?;
                Ok(command_output_json(&out))
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
                    .map_err(internal)?;
                Ok(command_output_json(&out))
            }
        })
        .build()
}

// -- helpers --------------------------------------------------------

fn internal(e: impl std::fmt::Display) -> tower_mcp::Error {
    tower_mcp::Error::internal(e.to_string())
}

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
fn strip_ansi(input: &str) -> String {
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
