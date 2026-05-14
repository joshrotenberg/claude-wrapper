//! MCP server layer over [`claude-wrapper`](claude_wrapper).
//!
//! Layer 2 of the claude stack:
//!
//! - **L0** Claude Code CLI (the binary)
//! - **L1** [`claude_wrapper`] -- typed Rust API over the CLI
//! - **L2** this crate -- 1:1ish MCP interface to the wrapper, as a
//!   tower-mcp library. Library is the product; the example CLI
//!   under `examples/server.rs` is one demonstration of wiring it
//!   to a transport.
//!
//! Because [`tower_mcp::McpRouter`] is a `tower::Service`, the
//! returned router IS an async `fn(Request) -> Response`. You can:
//!
//! - Embed it directly in another binary -- `router.call(req).await`
//!   with no transport at all.
//! - Hand it to [`tower_mcp::StdioTransport`] for a stdio MCP server.
//! - Hand it to `tower_mcp::HttpTransport` (when wired) for HTTP/SSE.
//!
//! [`registered_tools`] returns the same tool list without wiring a
//! transport -- useful for `tools` subcommands and diffs.
//!
//! # Example
//!
//! One-liner bootstrap. Pick a transport (or don't):
//!
//! ```no_run
//! # async fn example() -> Result<(), Box<dyn std::error::Error>> {
//! use claude_server::build_router;
//! use tower_mcp::StdioTransport;
//!
//! let router = build_router(Default::default())?;
//! let mut transport = StdioTransport::new(router);
//! transport.run().await?;
//! # Ok(()) }
//! ```
//!
//! See `examples/server.rs` for the canonical "raw MCP CLI" shape
//! (clap + tokio + the one-liner). It's intentionally minimal so
//! it doubles as copy-paste integration documentation.

//! ## Surfaces
//!
//! claude-server exposes three tool surfaces, layered by intent:
//!
//! 1. **Core** ([`core`]) -- 1:1 mirror of the `claude` CLI. Every
//!    subcommand you could shell out to lives here, with the
//!    deliberate exception of interactive (no `-p`) mode. Always on.
//! 2. **Chat** ([`chat`]) -- the duplex sidecar. We hold long-lived
//!    `claude` subprocesses, manage turn ordering, expose cost and
//!    history, and stream events back as MCP progress notifications.
//!    This is where the server earns its keep over a dumb passthrough.
//! 3. **Artifacts** (planned) -- CRUD over `~/.claude/skills/`,
//!    `~/.claude/agents/`, plugin manifests, MCP server configs.
//!    Not yet wired.

mod chat;
pub mod config;
mod core;
mod prompts;
mod resources;
mod state;
mod turns;

use std::sync::Arc;

use tower_mcp::McpRouter;

pub use self::config::{ClaudeConfig, ServerConfig};
pub use self::state::ServerState;

use claude_wrapper::Claude;

/// Build the MCP router from a [`ServerConfig`].
///
/// Constructs a [`Claude`] client from [`ServerConfig::claude`],
/// builds [`ServerState`], and registers the L2 CLI passthrough
/// surface plus resources and prompts. Hand the returned router to a
/// transport (stdio, HTTP, etc.) to serve it.
pub fn build_router(config: ServerConfig) -> claude_wrapper::error::Result<McpRouter> {
    let claude = build_claude(&config.claude)?;
    let state = ServerState::new(Arc::new(claude), Arc::new(config));

    let mut router = McpRouter::new()
        .server_info("claude-server", env!("CARGO_PKG_VERSION"))
        .instructions(
            "MCP server exposing the Claude Code CLI via claude-wrapper. \
             `claude.*` tools are 1:1 passthroughs to the CLI. \
             Read `claude://config` and `claude://tools` resources \
             to discover the active surface.",
        );

    for tool in core::tools(&state) {
        router = router.tool(tool);
    }
    for tool in chat::tools(&state) {
        router = router.tool(tool);
    }
    for resource in resources::resources(&state) {
        router = router.resource(resource);
    }
    for prompt in prompts::prompts(&state) {
        router = router.prompt(prompt);
    }

    Ok(router)
}

/// Light view of a registered tool. Returned by [`registered_tools`]
/// for introspection.
#[derive(Debug, Clone)]
pub struct ToolInfo {
    pub name: String,
    pub description: Option<String>,
}

/// List the tools that would be registered for a given config without
/// assembling a transport. Useful for `claude-server tools`.
pub fn registered_tools(config: ServerConfig) -> claude_wrapper::error::Result<Vec<ToolInfo>> {
    let claude = build_claude(&config.claude)?;
    let state = ServerState::new(Arc::new(claude), Arc::new(config));

    let mut all: Vec<ToolInfo> = core::tools(&state)
        .into_iter()
        .chain(chat::tools(&state))
        .map(|t| ToolInfo {
            name: t.name.clone(),
            description: t.description.clone(),
        })
        .collect();
    all.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(all)
}

/// Default timeout applied to CLI invocations when the config does
/// not set one. Five minutes covers a generous query turn while
/// bounding pathological hangs.
const DEFAULT_TIMEOUT_SECS: u64 = 300;

fn build_claude(cfg: &ClaudeConfig) -> claude_wrapper::error::Result<Claude> {
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
