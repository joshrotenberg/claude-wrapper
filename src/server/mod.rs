//! MCP server layer over `claude-wrapper`.
//!
//! This module exposes the wrapper's command builders and a
//! high-level "talk to the agent" interface as MCP tools via
//! [`tower-mcp`](https://crates.io/crates/tower-mcp). Two namespaces:
//!
//! - `claude.*` -- low-level passthrough (1:1 with the wrapper).
//!   Used by management UIs, scripts, and agents wanting fine-grained
//!   control.
//! - `agent.*` -- opinionated work-shaped interface. Server-configured
//!   defaults apply; per-call overrides allowed. Used by callers that
//!   just want to ask the agent.
//!
//! Both surfaces live on the same [`tower_mcp::McpRouter`] returned
//! by [`build_router`]. Pick a transport in your binary (stdio is the
//! conventional first choice for local tools).
//!
//! # Example
//!
//! ```no_run
//! # #[cfg(feature = "server")] {
//! use claude_wrapper::server::{ServerConfig, build_router};
//! use tower_mcp::StdioTransport;
//!
//! # async fn example() -> Result<(), tower_mcp::BoxError> {
//! let config = ServerConfig::default();
//! let router = build_router(config)?;
//! StdioTransport::new(router).run().await?;
//! # Ok(()) }
//! # }
//! ```

mod agent;
mod cli;
pub mod config;
mod error;
pub mod sandbox;
mod state;

use std::sync::Arc;

use tower_mcp::McpRouter;

pub use self::config::{
    AgentConfig, BudgetConfig, ClaudeConfig, SandboxConfig, SandboxMode, ServerConfig, ServerPolicy,
};
pub use self::sandbox::Sandbox;
pub use self::state::{ChatId, ServerState};

use crate::Claude;

/// Build the MCP router from a server config.
///
/// Constructs the underlying [`Claude`] client from
/// [`ServerConfig::claude`], builds [`ServerState`], and registers
/// every cli surface and agent surface tool the policy permits. The
/// returned router is ready to hand to a transport like
/// [`tower_mcp::StdioTransport`].
///
/// If [`ServerConfig::sandbox`] is enabled, the sandbox is created
/// (or reused if already on disk) and its overrides are layered onto
/// the resolved [`ClaudeConfig`] before the [`Claude`] client is
/// built. Caller-supplied env / working_dir wins over sandbox
/// defaults so explicit user overrides are never silently shadowed.
pub fn build_router(config: ServerConfig) -> crate::error::Result<McpRouter> {
    let mut config = config;

    let sandbox = sandbox::maybe_create(&config.sandbox)?;
    if let Some(ref s) = sandbox {
        s.apply_to(&mut config.claude);
        tracing::info!(
            sandbox_home = %s.home().display(),
            sandbox_workspace = %s.workspace().display(),
            "sandbox active",
        );
    }

    let claude = build_claude(&config.claude)?;
    let state = ServerState::new(Arc::new(claude), Arc::new(config));

    let mut router = McpRouter::new()
        .server_info("claude-wrapper-mcp", env!("CARGO_PKG_VERSION"))
        .auto_instructions_with(
            Some(
                "MCP server exposing the Claude Code CLI. \
                 Use the `agent.*` tools to talk to the agent with server defaults; \
                 use the `claude.*` tools for low-level CLI passthrough.",
            ),
            None::<String>,
        );

    for tool in cli::read_only_tools(&state) {
        router = router.tool(tool);
    }
    for tool in agent::agent_tools(&state) {
        router = router.tool(tool);
    }

    Ok(router)
}

/// Light view of a registered tool. Returned by [`registered_tools`]
/// for introspection (e.g. the `claude-server tools` subcommand).
#[derive(Debug, Clone)]
pub struct ToolInfo {
    pub name: String,
    pub description: Option<String>,
}

/// List the tools that would be registered for the given config,
/// without assembling a full [`McpRouter`] or starting any transport.
///
/// Useful for `claude-server tools` and for diffing tool surfaces
/// across configs (e.g. "what changes when I flip
/// `allow_mutations = true`?").
pub fn registered_tools(config: ServerConfig) -> crate::error::Result<Vec<ToolInfo>> {
    let mut config = config;
    let sandbox = sandbox::maybe_create(&config.sandbox)?;
    if let Some(ref s) = sandbox {
        s.apply_to(&mut config.claude);
    }
    let claude = build_claude(&config.claude)?;
    let state = ServerState::new(Arc::new(claude), Arc::new(config));

    let mut all = Vec::new();
    for t in cli::read_only_tools(&state) {
        all.push(ToolInfo {
            name: t.name.clone(),
            description: t.description.clone(),
        });
    }
    for t in agent::agent_tools(&state) {
        all.push(ToolInfo {
            name: t.name.clone(),
            description: t.description.clone(),
        });
    }
    all.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(all)
}

/// Default timeout applied to CLI invocations when the config does
/// not set one. Five minutes covers a generous query turn while
/// still bounding pathological hangs (e.g. `claude doctor` has been
/// observed running 3+ minutes in normal use).
const DEFAULT_TIMEOUT_SECS: u64 = 300;

fn build_claude(cfg: &ClaudeConfig) -> crate::error::Result<Claude> {
    let mut builder = Claude::builder();
    if let Some(ref bin) = cfg.binary {
        builder = builder.binary(bin);
    }
    if let Some(ref dir) = cfg.working_dir {
        builder = builder.working_dir(dir);
    }
    builder = builder.timeout_secs(cfg.timeout_secs.unwrap_or(DEFAULT_TIMEOUT_SECS));
    for (k, v) in &cfg.env {
        builder = builder.env(k, v);
    }
    for arg in &cfg.global_args {
        builder = builder.arg(arg);
    }
    builder.build()
}
