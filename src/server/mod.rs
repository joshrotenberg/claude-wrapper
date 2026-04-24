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

pub mod config;
mod error;
mod state;
mod surface_a;
mod surface_b;

use std::sync::Arc;

use tower_mcp::McpRouter;

pub use self::config::{BudgetConfig, ClaudeConfig, ServerConfig, ServerPolicy, SurfaceBConfig};
pub use self::state::{ChatId, ServerState};

use crate::Claude;

/// Build the MCP router from a server config.
///
/// Constructs the underlying [`Claude`] client from
/// [`ServerConfig::claude`], builds [`ServerState`], and registers
/// every Surface A and Surface B tool the policy permits. The
/// returned router is ready to hand to a transport like
/// [`tower_mcp::StdioTransport`].
pub fn build_router(config: ServerConfig) -> crate::error::Result<McpRouter> {
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

    for tool in surface_a::read_only_tools(&state) {
        router = router.tool(tool);
    }
    for tool in surface_b::agent_tools(&state) {
        router = router.tool(tool);
    }

    Ok(router)
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
