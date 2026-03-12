//! claude-pool MCP server binary.
//!
//! Manages a pool of Claude CLI slots, exposed as an MCP server
//! over stdio or HTTP transport.

use std::path::PathBuf;

use std::sync::Arc;

use clap::Parser;
use claude_pool::{InMemoryStore, Pool, PoolConfig, SkillRegistry, WorkflowRegistry};
use claude_pool_server::{State, prompts, resources, tools};
use tokio::sync::RwLock;
use tower_mcp::{McpRouter, StdioTransport};

/// MCP server for managing a pool of Claude CLI slots.
#[derive(Parser)]
#[command(name = "claude-pool-server", version)]
struct Cli {
    /// Number of slots to spawn.
    #[arg(short = 'n', long, default_value = "2")]
    slots: usize,

    /// Default model for all slots (e.g. "claude-haiku-4-5-20251001").
    #[arg(short, long)]
    model: Option<String>,

    /// Default effort level (min, low, medium, high, max).
    #[arg(short, long)]
    effort: Option<String>,

    /// Total budget cap in USD (e.g. 5.00).
    #[arg(short, long)]
    budget_usd: Option<f64>,

    /// System prompt for all slots.
    #[arg(short, long)]
    system_prompt: Option<String>,

    /// Permission mode for slots (default, acceptEdits, bypassPermissions, plan, auto).
    #[arg(short, long, default_value = "plan")]
    permission_mode: String,

    /// Minimum number of slots (floor for scale-down). Default: 1.
    #[arg(long, default_value = "1")]
    min_slots: usize,

    /// Maximum number of slots (ceiling for scale-up). Default: 16.
    #[arg(long, default_value = "16")]
    max_slots: usize,

    /// Enable git worktree isolation for slots.
    #[arg(short = 'w', long)]
    worktree: bool,

    /// Disable built-in skills.
    #[arg(long)]
    no_builtins: bool,

    /// Directory to load project-local skill definitions from.
    #[arg(long, default_value = ".claude-pool/skills")]
    skills_dir: PathBuf,

    /// Skip loading project-local skills.
    #[arg(long)]
    no_project_skills: bool,

    /// Skip loading global user skills from ~/.claude-pool/skills/.
    #[arg(long)]
    no_global_skills: bool,

    /// Disable specific skills by name (comma-separated).
    #[arg(long, value_delimiter = ',')]
    disable_skill: Vec<String>,

    /// Path to an .mcp.json file defining MCP servers available to all slots.
    ///
    /// The file format is `{"mcpServers": {"name": {...}}}`.
    /// Servers defined here are passed to slots via `--mcp-config`.
    #[arg(long, value_name = "PATH")]
    mcp_config: Option<PathBuf>,

    /// Disable strict MCP config mode.
    ///
    /// By default, `--strict-mcp-config` is passed to slots so they only use
    /// the servers defined in the pool config (not the coordinator's .mcp.json).
    /// Pass this flag to allow slots to also inherit the coordinator's servers.
    #[arg(long)]
    no_strict_mcp_config: bool,

    /// Use HTTP transport instead of stdio.
    ///
    /// When enabled, the server listens on an HTTP port for MCP requests
    /// using the Streamable HTTP transport (SSE for notifications).
    /// Multiple coordinators can connect simultaneously.
    #[arg(long)]
    http: bool,

    /// Port to listen on for HTTP transport.
    #[arg(long, default_value = "3100")]
    port: u16,

    /// Bind address for HTTP transport.
    #[arg(long, default_value = "127.0.0.1")]
    bind: String,

    /// Bearer tokens for HTTP authentication (comma-separated).
    ///
    /// When set, all HTTP requests must include an `Authorization: Bearer <token>`
    /// header with a valid token. When omitted, authentication is disabled.
    #[arg(long, value_delimiter = ',')]
    http_token: Vec<String>,

    /// Working directory for pool operations.
    ///
    /// Defaults to the git repository root detected via `git rev-parse --show-toplevel`.
    /// Required for worktree isolation to function correctly.
    #[arg(long)]
    working_dir: Option<PathBuf>,

    /// Enable REST API alongside the primary transport.
    ///
    /// Serves an HTTP REST API for non-MCP clients (CI/CD, dashboards, scripts).
    /// The REST API runs concurrently with the primary MCP transport.
    #[arg(long)]
    rest: bool,

    /// Port for the REST API server.
    #[arg(long, default_value = "3200")]
    rest_port: u16,

    /// Maximum concurrent REST API requests (0 = unlimited).
    #[arg(long, default_value = "0")]
    rest_max_concurrent: usize,
}

fn parse_permission_mode(s: &str) -> claude_pool::PermissionMode {
    match s.to_lowercase().as_str() {
        "default" => claude_pool::PermissionMode::Default,
        "acceptedits" => claude_pool::PermissionMode::AcceptEdits,
        "bypasspermissions" => claude_pool::PermissionMode::BypassPermissions,
        "dontask" => claude_pool::PermissionMode::DontAsk,
        "auto" => claude_pool::PermissionMode::Auto,
        _ => claude_pool::PermissionMode::Plan,
    }
}

fn parse_effort(s: &str) -> Option<claude_pool::Effort> {
    match s.to_lowercase().as_str() {
        "min" | "low" => Some(claude_pool::Effort::Low),
        "medium" => Some(claude_pool::Effort::Medium),
        "high" => Some(claude_pool::Effort::High),
        "max" => Some(claude_pool::Effort::Max),
        _ => None,
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Strip CLAUDECODE so child `claude` processes don't refuse to start.
    // When launched as an MCP server from within Claude Code, the server
    // inherits this variable, causing slots to fail with "Claude Code cannot
    // be launched inside another Claude Code session."
    // SAFETY: called before any threads are spawned.
    unsafe {
        std::env::remove_var("CLAUDECODE");
    }

    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::from_default_env()
                .add_directive("claude_pool=info".parse()?)
                .add_directive("claude_pool_server=info".parse()?),
        )
        .with_writer(std::io::stderr)
        .init();

    let cli = Cli::parse();

    // Load MCP servers from --mcp-config file if provided.
    let mcp_servers = if let Some(ref path) = cli.mcp_config {
        let contents = std::fs::read_to_string(path)
            .map_err(|e| format!("failed to read --mcp-config {}: {e}", path.display()))?;
        let parsed: serde_json::Value = serde_json::from_str(&contents)
            .map_err(|e| format!("invalid JSON in --mcp-config: {e}"))?;
        parsed["mcpServers"]
            .as_object()
            .map(|obj| obj.iter().map(|(k, v)| (k.clone(), v.clone())).collect())
            .unwrap_or_default()
    } else {
        std::collections::HashMap::new()
    };

    let config = PoolConfig {
        model: cli.model.clone(),
        effort: cli.effort.and_then(|e| parse_effort(&e)),
        budget_microdollars: cli.budget_usd.map(|b| (b * 1_000_000.0) as u64),
        system_prompt: cli.system_prompt,
        permission_mode: Some(parse_permission_mode(&cli.permission_mode)),
        worktree_isolation: cli.worktree,
        scaling: claude_pool::ScalingConfig {
            min_slots: cli.min_slots,
            max_slots: cli.max_slots,
        },
        mcp_servers,
        strict_mcp_config: !cli.no_strict_mcp_config,
        ..Default::default()
    };

    // Resolve working directory: explicit flag > git repo root > current dir.
    let working_dir = if let Some(dir) = cli.working_dir {
        dir
    } else {
        match std::process::Command::new("git")
            .args(["rev-parse", "--show-toplevel"])
            .output()
        {
            Ok(output) if output.status.success() => {
                PathBuf::from(String::from_utf8_lossy(&output.stdout).trim())
            }
            _ => {
                tracing::warn!(
                    "not inside a git repository; worktree isolation will not work. \
                     Use --working-dir to set a repo path explicitly."
                );
                std::env::current_dir().unwrap_or_default()
            }
        }
    };
    tracing::info!(working_dir = %working_dir.display(), "resolved working directory");

    let claude = claude_wrapper::Claude::builder()
        .working_dir(&working_dir)
        .build()?;

    let pool = Pool::builder_with_store(claude, InMemoryStore::new())
        .slots(cli.slots)
        .config(config)
        .build()
        .await
        .map_err(|e| format!("failed to build pool: {e}"))?;

    let mut skills = if cli.no_builtins {
        SkillRegistry::new()
    } else {
        SkillRegistry::with_builtins()
    };

    // Load global user skills (~/.claude-pool/skills/) — lower priority than project.
    if !cli.no_global_skills
        && let Some(home) = dirs::home_dir()
    {
        let global_dir = home.join(".claude-pool").join("skills");
        let count =
            skills.load_from_dir_with_source(&global_dir, claude_pool::SkillSource::Global)?;
        if count > 0 {
            tracing::info!(count, dir = %global_dir.display(), "loaded global skills");
        }
    }

    // Load project skills — highest priority, overrides global and builtin.
    if !cli.no_project_skills {
        let count = skills.load_from_dir(&cli.skills_dir)?;
        if count > 0 {
            tracing::info!(count, dir = %cli.skills_dir.display(), "loaded project skills");
        }
    }

    if !cli.disable_skill.is_empty() {
        let names: Vec<&str> = cli.disable_skill.iter().map(|s| s.as_str()).collect();
        skills.remove_many(&names);
        tracing::info!(disabled = ?cli.disable_skill, "disabled skills");
    }

    // Build prompts before wrapping skills in RwLock (prompts are static at startup).
    let prompt_list = prompts::skill_prompts(&skills);

    let workflows = WorkflowRegistry::with_builtins();

    let server_info = claude_pool_server::ServerInfo::new(
        cli.model.clone(),
        cli.permission_mode.clone(),
        cli.slots,
    );

    let state = Arc::new(State {
        pool,
        skills: Arc::new(RwLock::new(skills)),
        workflows,
        skills_dir: cli.skills_dir,
        server_info,
    });

    let tool_list = tools::all_tools(&state);
    let resource_list = resources::all_resources(&state);
    let template_list = resources::all_templates(&state);

    let mut router = McpRouter::new()
        .server_info("claude-pool", env!("CARGO_PKG_VERSION"))
        .instructions(
            "The pool is a system for parallelizing work across Claude CLI slots. \
             \
             Vocabulary — use these verbs consistently: \
             \"run\" = pool_run (sync single task), \
             \"fire\" = pool_submit (async single task, check with pool_result), \
             \"chain\" = pool_chain (sync pipeline), \
             \"fire a chain\" = pool_submit_chain (async pipeline, check with pool_chain_result), \
             \"fan out\" = pool_fan_out (parallel independent tasks), \
             \"run skill\" = pool_skill_run (execute a registered skill), \
             \"claim\" = pool_claim (worker self-service: idle slot grabs next pending task), \
             \"cancel\" = pool_cancel / pool_cancel_chain, \
             \"check\" = pool_result / pool_chain_result, \
             \"fire with review\" = pool_submit_with_review (async task requiring approval), \
             \"approve\" = pool_approve_result (accept a pending-review result), \
             \"reject\" = pool_reject_result (reject with feedback, re-queues), \
             \"inline\" = do the work yourself without the pool. \
             \
             When to use what: Default to inline when uncertain; user can say \"slotize it.\" \
             Run: one clear action with one clear output. If using \"and\" more than once, chain \
             instead. Chain: workflow where steps feed into each other; natural unit is a \
             deliverable (e.g. a PR, report, or resolved issue); each step should be independently \
             verifiable. Fan out: N independent instances of same work; use chain if they depend \
             on each other. Pool slots have CLI tools (git, cargo, gh) but not MCP access; use \
             the Agent tool for research requiring MCP tools. \
             \
             Tools: pool_run (run), pool_submit/pool_result (fire/check), pool_fan_out (fan out), \
             pool_chain (chain), pool_submit_chain/pool_chain_result (fire a chain/check), \
             context_set/get/list (shared state), pool_configure_slot, pool_send_message/ \
             pool_read_messages/pool_peek_messages/pool_broadcast (inter-slot messaging), \
             pool_find_slots (discover slots by name/role/state). \
             Both effort and model can be overridden per-task and per-chain-step. \
             \
             Model guidance: default to the pool's configured model; override per-task/step with \
             the model field when needed. Haiku: bounded single tasks (file a ticket, run checks, \
             rebase, tag), template-driven work, high-volume fan-outs where speed matters. \
             Sonnet: code review needing subtlety, multi-file changes with dependencies, planning \
             steps where quality matters. Opus: large mechanical refactors where one mistake breaks \
             compilation, complex architectural reasoning, tasks where you would want a senior \
             engineer. Rule of thumb: how much does the task benefit from deeper thinking? \
             \
             Skills: use pool_skill_list to discover available skills (SKILL.md format), \
             pool_skill_get to inspect, pool_skill_run to run, pool_skill_eject to customize \
             a builtin, pool_skill_save to persist. Skills are loaded from builtins, \
             ~/.claude-pool/skills/ (global), and .claude-pool/skills/ (project, highest priority). \
             \
             Monitoring: use /loop to watch or check things on a recurring interval \
             (e.g. `/loop 5m check on the chain`). /loop fires while idle (session-only). For \
             unattended scheduling, use cron or systemd timers calling `claude -p` directly. \
             The pool is stateless and reactive; scheduling is handled by the client.",
        )
        .tools(tool_list)
        .resources(resource_list)
        .prompts(prompt_list);

    for template in template_list {
        router = router.resource_template(template);
    }

    // Optionally spawn the REST API server alongside the primary transport.
    #[cfg(feature = "rest")]
    let rest_handle = if cli.rest {
        let rest_addr = format!("{}:{}", cli.bind, cli.rest_port);
        let rest_config = claude_pool_server::rest::RestConfig {
            tokens: claude_pool_server::auth::BearerTokens::new(cli.http_token.clone()),
            max_concurrent_requests: cli.rest_max_concurrent,
        };
        let rest_router = claude_pool_server::rest::router(state.clone(), rest_config);
        let listener = tokio::net::TcpListener::bind(&rest_addr).await?;
        tracing::info!(%rest_addr, "REST API starting");
        Some(tokio::spawn(async move {
            axum::serve(listener, rest_router).await
        }))
    } else {
        None
    };

    #[cfg(not(feature = "rest"))]
    if cli.rest {
        return Err("--rest requires the `rest` feature: install with `cargo install claude-pool-server --features rest`".into());
    }

    if cli.http {
        #[cfg(not(feature = "http"))]
        {
            let _ = router;
            return Err("--http requires the `http` feature: install with `cargo install claude-pool-server --features http`".into());
        }

        #[cfg(feature = "http")]
        {
            let addr = format!("{}:{}", cli.bind, cli.port);

            let tokens = claude_pool_server::auth::BearerTokens::new(cli.http_token);
            if tokens.is_empty() {
                tracing::warn!(
                    "HTTP transport started without authentication -- use --http-token for production"
                );
            }

            let app = build_http_app(router, tokens);

            tracing::info!(slots = cli.slots, %addr, "claude-pool-server starting (HTTP)");
            let listener = tokio::net::TcpListener::bind(&addr).await?;
            axum::serve(listener, app).await?;
        }
    } else {
        tracing::info!(slots = cli.slots, "claude-pool-server starting (stdio)");
        let mut transport = StdioTransport::new(router);
        transport.run().await?;
    }

    // If the REST server was spawned, wait for it to finish.
    #[cfg(feature = "rest")]
    if let Some(handle) = rest_handle {
        handle.await??;
    }

    Ok(())
}

/// Build an axum application with optional bearer token authentication.
#[cfg(feature = "http")]
fn build_http_app(
    router: McpRouter,
    tokens: claude_pool_server::auth::BearerTokens,
) -> axum::Router {
    use tower_mcp::HttpTransport;

    let transport = HttpTransport::new(router).disable_origin_validation();
    let app = transport.into_router();

    if tokens.is_empty() {
        app
    } else {
        app.layer(axum::middleware::from_fn(move |req, next| {
            bearer_auth_middleware(req, next, tokens.clone())
        }))
    }
}

/// Axum middleware that validates `Authorization: Bearer <token>` headers.
#[cfg(feature = "http")]
async fn bearer_auth_middleware(
    req: axum::extract::Request,
    next: axum::middleware::Next,
    tokens: claude_pool_server::auth::BearerTokens,
) -> axum::response::Response {
    use axum::http::StatusCode;
    use axum::response::IntoResponse;

    // Allow unauthenticated access to health endpoint.
    if req.uri().path() == "/health" {
        return next.run(req).await;
    }

    let authorized = req
        .headers()
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .is_some_and(|token| tokens.validate(token));

    if authorized {
        next.run(req).await
    } else {
        StatusCode::UNAUTHORIZED.into_response()
    }
}
