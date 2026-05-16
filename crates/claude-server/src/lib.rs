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
//! 4. **Artifacts** (`artifacts` feature) -- access to
//!    `~/.claude/agents/<stem>.md`. Read tools: `agent_list`,
//!    `agent_get`. Mutating tools (gated by `mutations` Cargo
//!    feature + runtime `policy.allow_mutations`): `agent_write`,
//!    `agent_delete`. Resources: `claude://agents`,
//!    `claude://agents/{file_stem}`. Skills CRUD is planned.
//! 5. **Worktrees** (`worktrees` feature) -- read-only git worktree
//!    introspection. Tools: `worktree_list`. Resources:
//!    `claude://worktrees`. Useful for hosts orchestrating
//!    `chat_open(worktree=true)` to inspect what they've spawned.
//! 6. **Jobs** (`jobs` feature) -- read-only access to background-job
//!    state Claude Code's `claude agents` daemon writes to
//!    `~/.claude/jobs/`. Tools: `claude_job_list`,
//!    `claude_job_get`. Resources: `claude://jobs`,
//!    `claude://jobs/{short_id}`. Cross-link with the `history`
//!    feature via the job's `session_path`.

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
#[cfg(feature = "jobs")]
mod jobs;
#[cfg(feature = "mutations")]
mod mutations;
mod prompts;
mod resources;
mod state;
#[cfg(feature = "chat")]
mod turn_tools;
mod turns;
#[cfg(feature = "worktrees")]
mod worktrees;

use std::sync::Arc;

use tower_mcp::McpRouter;
use tower_mcp::context::NotificationSender;

pub use self::config::{ClaudeConfig, ServerConfig, ServerPolicy, SurfacesConfig, TurnConfig};
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
               - claude://worktrees     -- git worktrees (worktrees feature)\n\
               - claude://jobs          -- background-job state (jobs feature)\n\
               - claude://jobs/{id}     -- one job's full timeline + state\n\
             \n\
             For a longer walkthrough call `prompts/get usage_guide`.",
        );

    // Each surface block applies BOTH the compile-time Cargo
    // feature gate AND the runtime config.surfaces.enable_X
    // dimmer. Cargo feature is the hard ceiling; runtime config
    // can disable but not enable beyond what was compiled in.
    #[cfg(feature = "core")]
    if state.config.surfaces.enable_core {
        for tool in core::tools(&state) {
            router = router.tool(tool);
        }
    }
    #[cfg(feature = "chat")]
    if state.config.surfaces.enable_chat {
        for tool in chat::tools(&state) {
            router = router.tool(tool);
        }
        for tool in turn_tools::tools(&state) {
            router = router.tool(tool);
        }
    }
    #[cfg(feature = "mutations")]
    if state.config.policy.allow_mutations && state.config.surfaces.enable_mutations {
        for tool in mutations::tools(&state) {
            router = router.tool(tool);
        }
        tracing::info!(
            "mutating tools registered (policy.allow_mutations + surfaces.enable_mutations = true)"
        );
    }
    #[cfg(feature = "history")]
    if state.config.surfaces.enable_history {
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
    if state.config.surfaces.enable_artifacts {
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
    #[cfg(all(feature = "artifacts", feature = "mutations"))]
    if state.config.policy.allow_mutations
        && state.config.surfaces.enable_mutations
        && state.config.surfaces.enable_artifacts
    {
        for tool in artifacts::mutating_tools(&state) {
            router = router.tool(tool);
        }
        tracing::info!("artifacts mutating tools registered");
    }
    #[cfg(feature = "worktrees")]
    if state.config.surfaces.enable_worktrees {
        for tool in worktrees::tools(&state) {
            router = router.tool(tool);
        }
        for resource in worktrees::resources(&state) {
            router = router.resource(resource);
        }
        for template in worktrees::templates(&state) {
            router = router.resource_template(template);
        }
    }
    #[cfg(feature = "jobs")]
    if state.config.surfaces.enable_jobs {
        for tool in jobs::tools(&state) {
            router = router.tool(tool);
        }
        for resource in jobs::resources(&state) {
            router = router.resource(resource);
        }
        for template in jobs::templates(&state) {
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
            feature = "artifacts",
            feature = "worktrees",
            feature = "jobs"
        )),
        allow(unused_variables)
    )]
    let state = ServerState::new(Arc::new(claude), Arc::new(config));

    // Runtime gate dimmer + compile-time feature ceiling. Each
    // surface returns an empty Vec when either the Cargo feature
    // is off OR the runtime surfaces.enable_X flag is false.
    #[cfg(feature = "mutations")]
    let muts: Vec<tower_mcp::Tool> =
        if state.config.policy.allow_mutations && state.config.surfaces.enable_mutations {
            mutations::tools(&state)
        } else {
            Vec::new()
        };
    #[cfg(not(feature = "mutations"))]
    let muts: Vec<tower_mcp::Tool> = Vec::new();

    #[cfg(feature = "history")]
    let history_tools = if state.config.surfaces.enable_history {
        history::tools(&state)
    } else {
        Vec::new()
    };
    #[cfg(not(feature = "history"))]
    let history_tools: Vec<tower_mcp::Tool> = Vec::new();

    #[cfg(feature = "artifacts")]
    let artifacts_tools = if state.config.surfaces.enable_artifacts {
        artifacts::tools(&state)
    } else {
        Vec::new()
    };
    #[cfg(not(feature = "artifacts"))]
    let artifacts_tools: Vec<tower_mcp::Tool> = Vec::new();

    #[cfg(all(feature = "artifacts", feature = "mutations"))]
    let artifacts_mut_tools: Vec<tower_mcp::Tool> = if state.config.policy.allow_mutations
        && state.config.surfaces.enable_mutations
        && state.config.surfaces.enable_artifacts
    {
        artifacts::mutating_tools(&state)
    } else {
        Vec::new()
    };
    #[cfg(not(all(feature = "artifacts", feature = "mutations")))]
    let artifacts_mut_tools: Vec<tower_mcp::Tool> = Vec::new();

    #[cfg(feature = "worktrees")]
    let worktrees_tools = if state.config.surfaces.enable_worktrees {
        worktrees::tools(&state)
    } else {
        Vec::new()
    };
    #[cfg(not(feature = "worktrees"))]
    let worktrees_tools: Vec<tower_mcp::Tool> = Vec::new();

    #[cfg(feature = "jobs")]
    let jobs_tools = if state.config.surfaces.enable_jobs {
        jobs::tools(&state)
    } else {
        Vec::new()
    };
    #[cfg(not(feature = "jobs"))]
    let jobs_tools: Vec<tower_mcp::Tool> = Vec::new();

    #[cfg(feature = "core")]
    let core_tools = if state.config.surfaces.enable_core {
        core::tools(&state)
    } else {
        Vec::new()
    };
    #[cfg(not(feature = "core"))]
    let core_tools: Vec<tower_mcp::Tool> = Vec::new();
    #[cfg(feature = "chat")]
    let chat_tools = if state.config.surfaces.enable_chat {
        chat::tools(&state)
    } else {
        Vec::new()
    };
    #[cfg(not(feature = "chat"))]
    let chat_tools: Vec<tower_mcp::Tool> = Vec::new();
    #[cfg(feature = "chat")]
    let turn_tool_list = if state.config.surfaces.enable_chat {
        turn_tools::tools(&state)
    } else {
        Vec::new()
    };
    #[cfg(not(feature = "chat"))]
    let turn_tool_list: Vec<tower_mcp::Tool> = Vec::new();

    let mut all: Vec<ToolInfo> = core_tools
        .into_iter()
        .chain(chat_tools)
        .chain(turn_tool_list)
        .chain(muts)
        .chain(history_tools)
        .chain(artifacts_tools)
        .chain(artifacts_mut_tools)
        .chain(worktrees_tools)
        .chain(jobs_tools)
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

/// Minimum `claude` CLI version this server has been verified
/// against. Below this we know flags / argument shapes are missing
/// or different. See [`claude_wrapper::Claude::cli_version_status`]
/// for the runtime classifier.
const TESTED_CLI_MIN: claude_wrapper::CliVersion = claude_wrapper::CliVersion {
    major: 2,
    minor: 1,
    patch: 0,
};

/// Maximum `claude` CLI version this server has been verified
/// against. Above this is "untested" -- the wrapper will warn
/// (via tracing) and the typed status surfaces on
/// `claude_doctor` + `claude://config`. Bump as we verify against
/// later releases. Notable past breakage: 2.1.143 repurposed
/// `claude agents` from a list-agents subcommand to a background-
/// session TUI, which we caught and worked around.
const TESTED_CLI_MAX: claude_wrapper::CliVersion = claude_wrapper::CliVersion {
    major: 2,
    minor: 1,
    patch: 143,
};

fn build_claude(cfg: &ClaudeConfig) -> claude_wrapper::error::Result<Claude> {
    let mut builder = Claude::builder().tested_cli_version_range(TESTED_CLI_MIN, TESTED_CLI_MAX);
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
