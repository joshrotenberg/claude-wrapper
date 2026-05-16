//! Mutating CLI passthrough tools. Registered only when
//! [`crate::ServerPolicy::allow_mutations`] is true.
//!
//! Each tool wraps a `claude_wrapper` command builder for a CLI
//! subcommand that modifies user/project/local state -- adding MCP
//! servers, installing plugins, adding marketplaces. The tool shape
//! is identical to the read-only core: typed input, JSON output
//! mirroring CommandOutput with ANSI stripped.
//!
//! Naming convention matches the read-only core: `claude_<sub>_<verb>`.
//! `claude mcp add foo` -> `claude_mcp_add`, etc.

use std::collections::BTreeMap;
use std::sync::Arc;

use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::json;
use tower_mcp::{CallToolResult, Tool, ToolBuilder};

use claude_wrapper::Claude;
use claude_wrapper::ClaudeCommand;
use claude_wrapper::command::marketplace::{
    MarketplaceAddCommand, MarketplaceRemoveCommand, MarketplaceUpdateCommand,
};
use claude_wrapper::command::mcp::{McpAddCommand, McpAddJsonCommand, McpRemoveCommand};
use claude_wrapper::command::plugin::{
    PluginDisableCommand, PluginEnableCommand, PluginInstallCommand, PluginPruneCommand,
    PluginUninstallCommand, PluginUpdateCommand,
};
use claude_wrapper::types::Scope;

use crate::state::ServerState;

pub(crate) fn tools(state: &ServerState) -> Vec<Tool> {
    vec![
        tool_mcp_add(state),
        tool_mcp_add_json(state),
        tool_mcp_remove(state),
        tool_plugin_install(state),
        tool_plugin_uninstall(state),
        tool_plugin_prune(state),
        tool_plugin_enable(state),
        tool_plugin_disable(state),
        tool_plugin_update(state),
        tool_marketplace_add(state),
        tool_marketplace_remove(state),
        tool_marketplace_update(state),
    ]
}

fn parse_scope(s: &str) -> Result<Scope, tower_mcp::Error> {
    match s {
        "user" => Ok(Scope::User),
        "project" => Ok(Scope::Project),
        "local" => Ok(Scope::Local),
        "managed" => Ok(Scope::Managed),
        other => Err(tower_mcp::Error::internal(format!(
            "invalid scope `{other}` (expected user / project / local / managed)"
        ))),
    }
}

fn cmd_output_json(out: &claude_wrapper::exec::CommandOutput) -> CallToolResult {
    CallToolResult::json(json!({
        "stdout": crate::core::strip_ansi(&out.stdout),
        "stderr": crate::core::strip_ansi(&out.stderr),
        "exit_code": out.exit_code,
        "success": out.success,
    }))
}

use crate::errors::{from_wrapper, internal};

// -- claude_mcp_add --------------------------------------------------

#[derive(Debug, Deserialize, JsonSchema)]
struct McpAddInput {
    /// Name to register the MCP server under.
    name: String,
    /// Command (for stdio) or URL (for http/sse).
    command_or_url: String,
    /// Optional scope: user, project, or local. Default: CLI default.
    #[serde(default)]
    scope: Option<String>,
    /// Optional transport: stdio, http, sse.
    #[serde(default)]
    transport: Option<String>,
    /// Environment variables to pass to the spawned server.
    #[serde(default)]
    env: BTreeMap<String, String>,
    /// Extra arguments for the server command.
    #[serde(default)]
    server_args: Vec<String>,
}

fn tool_mcp_add(state: &ServerState) -> Tool {
    let claude = state.claude.clone();
    ToolBuilder::new("claude_mcp_add")
        .description("Run `claude mcp add` to register a new MCP server.")
        .handler(move |input: McpAddInput| {
            let claude = Arc::clone(&claude);
            async move { run_mcp_add(claude, input).await }
        })
        .build()
}

async fn run_mcp_add(
    claude: Arc<Claude>,
    input: McpAddInput,
) -> Result<CallToolResult, tower_mcp::Error> {
    let mut cmd = McpAddCommand::new(input.name, input.command_or_url);
    if let Some(s) = input.scope {
        cmd = cmd.scope(parse_scope(&s)?);
    }
    if let Some(t) = input.transport {
        use claude_wrapper::types::Transport;
        let parsed: Transport = t.parse().map_err(internal)?;
        cmd = cmd.transport(parsed);
    }
    for (k, v) in input.env {
        cmd = cmd.env(k, v);
    }
    if !input.server_args.is_empty() {
        cmd = cmd.server_args(input.server_args);
    }
    let out = cmd.execute(&claude).await.map_err(from_wrapper)?;
    Ok(cmd_output_json(&out))
}

// -- claude_mcp_add_json --------------------------------------------

#[derive(Debug, Deserialize, JsonSchema)]
struct McpAddJsonInput {
    /// Name to register the server under.
    name: String,
    /// Full JSON config blob for the server.
    json: String,
    #[serde(default)]
    scope: Option<String>,
}

fn tool_mcp_add_json(state: &ServerState) -> Tool {
    let claude = state.claude.clone();
    ToolBuilder::new("claude_mcp_add_json")
        .description("Run `claude mcp add-json` with a full JSON server config.")
        .handler(move |input: McpAddJsonInput| {
            let claude = Arc::clone(&claude);
            async move {
                let mut cmd = McpAddJsonCommand::new(input.name, input.json);
                if let Some(s) = input.scope {
                    cmd = cmd.scope(parse_scope(&s)?);
                }
                let out = cmd.execute(&claude).await.map_err(from_wrapper)?;
                Ok(cmd_output_json(&out))
            }
        })
        .build()
}

// -- claude_mcp_remove ----------------------------------------------

#[derive(Debug, Deserialize, JsonSchema)]
struct McpRemoveInput {
    name: String,
    #[serde(default)]
    scope: Option<String>,
}

fn tool_mcp_remove(state: &ServerState) -> Tool {
    let claude = state.claude.clone();
    ToolBuilder::new("claude_mcp_remove")
        .description("Run `claude mcp remove` to unregister an MCP server.")
        .handler(move |input: McpRemoveInput| {
            let claude = Arc::clone(&claude);
            async move {
                let mut cmd = McpRemoveCommand::new(input.name);
                if let Some(s) = input.scope {
                    cmd = cmd.scope(parse_scope(&s)?);
                }
                let out = cmd.execute(&claude).await.map_err(from_wrapper)?;
                Ok(cmd_output_json(&out))
            }
        })
        .build()
}

// -- claude_plugin_{install,uninstall,enable,disable,update} ---------

#[derive(Debug, Deserialize, JsonSchema)]
struct PluginScopedInput {
    plugin: String,
    #[serde(default)]
    scope: Option<String>,
}

fn tool_plugin_install(state: &ServerState) -> Tool {
    let claude = state.claude.clone();
    ToolBuilder::new("claude_plugin_install")
        .description("Run `claude plugin install <plugin>`.")
        .handler(move |input: PluginScopedInput| {
            let claude = Arc::clone(&claude);
            async move {
                let mut cmd = PluginInstallCommand::new(input.plugin);
                if let Some(s) = input.scope {
                    cmd = cmd.scope(parse_scope(&s)?);
                }
                let out = cmd.execute(&claude).await.map_err(from_wrapper)?;
                Ok(cmd_output_json(&out))
            }
        })
        .build()
}

#[derive(Debug, Deserialize, JsonSchema)]
struct PluginUninstallInput {
    plugin: String,
    /// Scope: user / project / local / managed.
    #[serde(default)]
    scope: Option<String>,
    /// Preserve the plugin's persistent data directory
    /// (`~/.claude/plugins/data/{id}/`) on uninstall (`--keep-data`).
    /// Default: data is removed alongside the plugin.
    #[serde(default)]
    keep_data: bool,
    /// Also remove auto-installed dependencies that are no longer
    /// needed (`--prune`). Requires `yes: true` in non-interactive
    /// contexts -- which the server always is.
    #[serde(default)]
    prune: bool,
    /// Skip the confirmation prompt (`-y`). **The server is always
    /// non-TTY**, so leaving this off will hang the underlying CLI
    /// on its prompt. Default true here so the common case doesn't
    /// trip; pass `false` if you want the CLI to prompt (only useful
    /// for testing).
    #[serde(default = "default_true")]
    yes: bool,
}

fn default_true() -> bool {
    true
}

fn tool_plugin_uninstall(state: &ServerState) -> Tool {
    let claude = state.claude.clone();
    ToolBuilder::new("claude_plugin_uninstall")
        .description(
            "Run `claude plugin uninstall <plugin>`. Defaults `yes: true` \
             since the server is always non-TTY and the underlying CLI \
             would otherwise hang on its confirmation prompt. Optional \
             flags: `keep_data` (preserve data dir), `prune` (remove \
             auto-installed deps), `scope` (user|project|local|managed).",
        )
        .handler(move |input: PluginUninstallInput| {
            let claude = Arc::clone(&claude);
            async move {
                let mut cmd = PluginUninstallCommand::new(input.plugin);
                if let Some(s) = input.scope {
                    cmd = cmd.scope(parse_scope(&s)?);
                }
                if input.keep_data {
                    cmd = cmd.keep_data();
                }
                if input.prune {
                    cmd = cmd.prune();
                }
                if input.yes {
                    cmd = cmd.yes();
                }
                let out = cmd.execute(&claude).await.map_err(from_wrapper)?;
                Ok(cmd_output_json(&out))
            }
        })
        .build()
}

#[derive(Debug, Default, Deserialize, JsonSchema)]
struct PluginPruneInput {
    /// Print what would be removed without removing anything.
    #[serde(default)]
    dry_run: bool,
    /// Scope: user / project / local / managed.
    #[serde(default)]
    scope: Option<String>,
    /// Skip confirmation (`-y`). Defaults true for non-TTY safety;
    /// pass `false` to let the CLI prompt (only useful for testing).
    #[serde(default = "default_true")]
    yes: bool,
}

fn tool_plugin_prune(state: &ServerState) -> Tool {
    let claude = state.claude.clone();
    ToolBuilder::new("claude_plugin_prune")
        .description(
            "Run `claude plugin prune` (alias `autoremove`) to remove \
             auto-installed dependencies that are no longer needed. \
             Defaults `yes: true` since the server is always non-TTY. \
             Optional `dry_run` previews without removing.",
        )
        .handler(move |input: PluginPruneInput| {
            let claude = Arc::clone(&claude);
            async move {
                let mut cmd = PluginPruneCommand::new();
                if input.dry_run {
                    cmd = cmd.dry_run();
                }
                if let Some(s) = input.scope {
                    cmd = cmd.scope(parse_scope(&s)?);
                }
                if input.yes {
                    cmd = cmd.yes();
                }
                let out = cmd.execute(&claude).await.map_err(from_wrapper)?;
                Ok(cmd_output_json(&out))
            }
        })
        .build()
}

fn tool_plugin_enable(state: &ServerState) -> Tool {
    let claude = state.claude.clone();
    ToolBuilder::new("claude_plugin_enable")
        .description("Run `claude plugin enable <plugin>`.")
        .handler(move |input: PluginScopedInput| {
            let claude = Arc::clone(&claude);
            async move {
                let mut cmd = PluginEnableCommand::new(input.plugin);
                if let Some(s) = input.scope {
                    cmd = cmd.scope(parse_scope(&s)?);
                }
                let out = cmd.execute(&claude).await.map_err(from_wrapper)?;
                Ok(cmd_output_json(&out))
            }
        })
        .build()
}

fn tool_plugin_disable(state: &ServerState) -> Tool {
    let claude = state.claude.clone();
    ToolBuilder::new("claude_plugin_disable")
        .description("Run `claude plugin disable <plugin>`.")
        .handler(move |input: PluginScopedInput| {
            let claude = Arc::clone(&claude);
            async move {
                let mut cmd = PluginDisableCommand::new(input.plugin);
                if let Some(s) = input.scope {
                    cmd = cmd.scope(parse_scope(&s)?);
                }
                let out = cmd.execute(&claude).await.map_err(from_wrapper)?;
                Ok(cmd_output_json(&out))
            }
        })
        .build()
}

fn tool_plugin_update(state: &ServerState) -> Tool {
    let claude = state.claude.clone();
    ToolBuilder::new("claude_plugin_update")
        .description("Run `claude plugin update <plugin>`.")
        .handler(move |input: PluginScopedInput| {
            let claude = Arc::clone(&claude);
            async move {
                let mut cmd = PluginUpdateCommand::new(input.plugin);
                if let Some(s) = input.scope {
                    cmd = cmd.scope(parse_scope(&s)?);
                }
                let out = cmd.execute(&claude).await.map_err(from_wrapper)?;
                Ok(cmd_output_json(&out))
            }
        })
        .build()
}

// -- claude_marketplace_{add,remove,update} -------------------------

#[derive(Debug, Deserialize, JsonSchema)]
struct MarketplaceAddInput {
    /// Source (git URL or filesystem path).
    source: String,
    #[serde(default)]
    scope: Option<String>,
}

fn tool_marketplace_add(state: &ServerState) -> Tool {
    let claude = state.claude.clone();
    ToolBuilder::new("claude_marketplace_add")
        .description("Run `claude plugin marketplace add <source>`.")
        .handler(move |input: MarketplaceAddInput| {
            let claude = Arc::clone(&claude);
            async move {
                let mut cmd = MarketplaceAddCommand::new(input.source);
                if let Some(s) = input.scope {
                    cmd = cmd.scope(parse_scope(&s)?);
                }
                let out = cmd.execute(&claude).await.map_err(from_wrapper)?;
                Ok(cmd_output_json(&out))
            }
        })
        .build()
}

#[derive(Debug, Deserialize, JsonSchema)]
struct MarketplaceNameInput {
    name: String,
}

fn tool_marketplace_remove(state: &ServerState) -> Tool {
    let claude = state.claude.clone();
    ToolBuilder::new("claude_marketplace_remove")
        .description("Run `claude plugin marketplace remove <name>`.")
        .handler(move |input: MarketplaceNameInput| {
            let claude = Arc::clone(&claude);
            async move {
                let cmd = MarketplaceRemoveCommand::new(input.name);
                let out = cmd.execute(&claude).await.map_err(from_wrapper)?;
                Ok(cmd_output_json(&out))
            }
        })
        .build()
}

fn tool_marketplace_update(state: &ServerState) -> Tool {
    let claude = state.claude.clone();
    ToolBuilder::new("claude_marketplace_update")
        .description("Run `claude plugin marketplace update <name>`.")
        .handler(move |input: MarketplaceNameInput| {
            let claude = Arc::clone(&claude);
            async move {
                let cmd = MarketplaceUpdateCommand::new(input.name);
                let out = cmd.execute(&claude).await.map_err(from_wrapper)?;
                Ok(cmd_output_json(&out))
            }
        })
        .build()
}
