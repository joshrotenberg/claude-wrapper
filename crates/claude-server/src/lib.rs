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
//! claude-server exposes several tool surfaces, layered by intent:
//!
//! 1. **Core** -- 1:1 mirror of the `claude` CLI. Every
//!    subcommand you could shell out to lives here, with the
//!    deliberate exception of interactive (no `-p`) mode. Always on.
//! 2. **Chat** -- the duplex sidecar. We hold long-lived
//!    `claude` subprocesses, manage turn ordering, expose cost and
//!    history, and stream events back as MCP progress notifications.
//!    This is where the server earns its keep over a dumb passthrough.
//! 3. **History** (`history` feature) -- read-only access to
//!    `~/.claude/projects/<slug>/<session_id>.jsonl` files. Tools:
//!    `claude_project_list`, `claude_session_list`,
//!    `claude_session_get`. Resources: `claude://projects`,
//!    `claude://projects/{slug}`, `claude://sessions/{id}`.
//! 4. **Artifacts** (`artifacts` feature) -- read-only access to
//!    `~/.claude/agents/<stem>.md`. Tools: `agent_list`, `agent_get`.
//!    Resources: `claude://agents`, `claude://agents/{file_stem}`.
//!    Skills CRUD and write/delete on agents are planned but not
//!    yet wired.

#[cfg(feature = "artifacts")]
mod artifacts;
#[cfg(feature = "chat")]
mod chat;
pub mod config;
#[cfg(feature = "core")]
mod core;
mod errors;
#[cfg(feature = "history")]
mod history;
#[cfg(feature = "mutations")]
mod mutations;
mod prompts;
mod resources;
mod state;
#[cfg(feature = "chat")]
mod turn_tools;
mod turns;

use std::sync::Arc;

use tower_mcp::McpRouter;
use tower_mcp::context::NotificationSender;

pub use self::config::{ClaudeConfig, ServerConfig, ServerPolicy, TurnConfig};
pub use self::state::ServerState;
pub use tower_mcp::context::{NotificationReceiver, notification_channel};

use claude_wrapper::Claude;

/// Build the MCP router from a [`ServerConfig`] without notification
/// support. Subscriptions to `claude://chats/{id}` (or any other
/// resource) won't fire `notifications/resources/updated` -- callers
/// have to poll. Suitable for stdio-only deployments where the
/// transport doesn't surface notifications anyway.
///
/// For deployments that want subscription updates (UIs, HTTP
/// dashboards), use [`build_router_with_notification_sender`].
pub fn build_router(config: ServerConfig) -> claude_wrapper::error::Result<McpRouter> {
    build_router_inner(config, None)
}

/// Build the MCP router with an attached notification sender.
///
/// Wires `tx` into both [`ServerState`] (so chat workers can fire
/// `notifications/resources/updated` after a turn settles) AND into
/// the underlying [`McpRouter`] (so router-internal events go down
/// the same channel). The corresponding [`NotificationReceiver`]
/// must be plumbed into the transport (e.g.
/// `GenericStdioTransport::with_notifications` or `HttpTransport`'s
/// notification handling).
///
/// Typical wiring:
///
/// ```no_run
/// # async fn example() -> Result<(), Box<dyn std::error::Error>> {
/// use claude_server::{build_router_with_notification_sender, notification_channel};
///
/// let (tx, rx) = notification_channel(256);
/// let router = build_router_with_notification_sender(Default::default(), tx)?;
/// // ... hand `router` and `rx` to a notification-aware transport
/// # let _ = (router, rx);
/// # Ok(()) }
/// ```
pub fn build_router_with_notification_sender(
    config: ServerConfig,
    tx: NotificationSender,
) -> claude_wrapper::error::Result<McpRouter> {
    build_router_inner(config, Some(tx))
}

fn build_router_inner(
    config: ServerConfig,
    notifier: Option<NotificationSender>,
) -> claude_wrapper::error::Result<McpRouter> {
    let claude = build_claude(&config.claude)?;
    let mut state = ServerState::new(Arc::new(claude), Arc::new(config));
    if let Some(ref tx) = notifier {
        state = state.with_notifier(tx.clone());
    }

    // Spawn the turn-registry TTL sweeper. Runs forever until the
    // registry's last Arc is dropped (the JoinHandle is intentionally
    // not retained -- we don't have a graceful-shutdown story today).
    let ttl = std::time::Duration::from_secs(state.config.turns.ttl_secs);
    let interval = std::time::Duration::from_secs(state.config.turns.sweep_interval_secs);
    let _sweeper = state.turns.clone().spawn_sweeper(ttl, interval);

    let mut router = McpRouter::new()
        .server_info("claude-server", env!("CARGO_PKG_VERSION"))
        .instructions(
            "MCP server exposing the Claude Code CLI via claude-wrapper.\n\
             \n\
             THREE SURFACES:\n\
               - claude_*  -- 1:1 mirror of the `claude` CLI\n\
               - chat_*    -- long-lived multi-turn conversations\n\
               - turn_*    -- async lifecycle for in-flight turns\n\
             \n\
             ASYNC BY DEFAULT for agent turns. The bare `chat_send` and \
             `claude_query` return a turn_id immediately and run in the \
             background -- poll with `turn_get`, block with `turn_wait`, \
             cancel with `turn_cancel`. The `*_sync` variants hold your \
             request connection open until the turn completes; reach for \
             them only if you genuinely need to block.\n\
             \n\
             TYPICAL FLOWS:\n\
               - Single-shot:   claude_query(prompt) -> turn_id; turn_wait(id)\n\
               - Conversation:  chat_open -> chat_id; \
             chat_send(id, prompt) -> turn_id; turn_wait; repeat; chat_close\n\
               - Multi-project: chat_open(working_dir: \"/path/to/repo\")\n\
               - Resume:        chat_open(resume: \"<session_id>\")\n\
             \n\
             DISCOVERY:\n\
               - claude://tools         -- the full registered tool surface\n\
               - claude://config        -- server config (env values redacted)\n\
               - claude://chats         -- live chats with cost + turn count\n\
               - claude://chats/{id}    -- one chat's full history (subscribable)\n\
               - claude://metrics       -- process counters; check spend mid-run\n\
               - claude://projects      -- on-disk project history (history feature)\n\
               - claude://sessions/{id} -- full parsed JSONL log for a session\n\
               - claude://agents        -- user-level agents (artifacts feature)\n\
               - claude://agents/{stem} -- one agent's full record + body\n\
             \n\
             For a longer walkthrough call `prompts/get usage_guide`.",
        );

    #[cfg(feature = "core")]
    for tool in core::tools(&state) {
        router = router.tool(tool);
    }
    #[cfg(feature = "chat")]
    for tool in chat::tools(&state) {
        router = router.tool(tool);
    }
    #[cfg(feature = "chat")]
    for tool in turn_tools::tools(&state) {
        router = router.tool(tool);
    }
    #[cfg(feature = "mutations")]
    if state.config.policy.allow_mutations {
        for tool in mutations::tools(&state) {
            router = router.tool(tool);
        }
        tracing::info!("mutating tools registered (policy.allow_mutations = true)");
    }
    #[cfg(feature = "history")]
    {
        for tool in history::tools(&state) {
            router = router.tool(tool);
        }
        for resource in history::resources(&state) {
            router = router.resource(resource);
        }
        for template in history::templates(&state) {
            router = router.resource_template(template);
        }
    }
    #[cfg(feature = "artifacts")]
    {
        for tool in artifacts::tools(&state) {
            router = router.tool(tool);
        }
        for resource in artifacts::resources(&state) {
            router = router.resource(resource);
        }
        for template in artifacts::templates(&state) {
            router = router.resource_template(template);
        }
    }
    for resource in resources::resources(&state) {
        router = router.resource(resource);
    }
    for template in resources::templates(&state) {
        router = router.resource_template(template);
    }
    for prompt in prompts::prompts(&state) {
        router = router.prompt(prompt);
    }

    if let Some(tx) = notifier {
        router = router.with_notification_sender(tx);
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
    #[cfg_attr(
        not(any(
            feature = "core",
            feature = "chat",
            feature = "mutations",
            feature = "history",
            feature = "artifacts"
        )),
        allow(unused_variables)
    )]
    let state = ServerState::new(Arc::new(claude), Arc::new(config));

    #[cfg(feature = "mutations")]
    let muts: Vec<tower_mcp::Tool> = if state.config.policy.allow_mutations {
        mutations::tools(&state)
    } else {
        Vec::new()
    };
    #[cfg(not(feature = "mutations"))]
    let muts: Vec<tower_mcp::Tool> = Vec::new();

    #[cfg(feature = "history")]
    let history_tools = history::tools(&state);
    #[cfg(not(feature = "history"))]
    let history_tools: Vec<tower_mcp::Tool> = Vec::new();

    #[cfg(feature = "artifacts")]
    let artifacts_tools = artifacts::tools(&state);
    #[cfg(not(feature = "artifacts"))]
    let artifacts_tools: Vec<tower_mcp::Tool> = Vec::new();

    #[cfg(feature = "core")]
    let core_tools = core::tools(&state);
    #[cfg(not(feature = "core"))]
    let core_tools: Vec<tower_mcp::Tool> = Vec::new();
    #[cfg(feature = "chat")]
    let chat_tools = chat::tools(&state);
    #[cfg(not(feature = "chat"))]
    let chat_tools: Vec<tower_mcp::Tool> = Vec::new();
    #[cfg(feature = "chat")]
    let turn_tool_list = turn_tools::tools(&state);
    #[cfg(not(feature = "chat"))]
    let turn_tool_list: Vec<tower_mcp::Tool> = Vec::new();

    let mut all: Vec<ToolInfo> = core_tools
        .into_iter()
        .chain(chat_tools)
        .chain(turn_tool_list)
        .chain(muts)
        .chain(history_tools)
        .chain(artifacts_tools)
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
