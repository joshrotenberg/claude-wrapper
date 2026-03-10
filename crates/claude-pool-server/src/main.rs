//! claude-pool MCP server binary.
//!
//! Manages a pool of Claude CLI workers, exposed as an MCP server
//! over stdio transport.

mod prompts;
mod resources;
mod tools;

use std::sync::Arc;

use clap::Parser;
use claude_pool::{GlobalWorkerConfig, InMemoryStore, Pool, PoolStore, SkillRegistry};
use tower_mcp::{McpRouter, StdioTransport};

/// Shared state accessible by all tool/resource handlers.
pub struct State<S: PoolStore> {
    pub pool: Pool<S>,
    pub skills: SkillRegistry,
}

/// MCP server for managing a pool of Claude CLI workers.
#[derive(Parser)]
#[command(name = "claude-pool-server", version)]
struct Cli {
    /// Number of workers to spawn.
    #[arg(short = 'n', long, default_value = "2")]
    workers: usize,

    /// Default model for all workers (e.g. "claude-haiku-4-5-20251001").
    #[arg(short, long)]
    model: Option<String>,

    /// Default effort level (min, low, medium, high, max).
    #[arg(short, long)]
    effort: Option<String>,

    /// Total budget cap in USD (e.g. 5.00).
    #[arg(short, long)]
    budget_usd: Option<f64>,

    /// System prompt for all workers.
    #[arg(short, long)]
    system_prompt: Option<String>,

    /// Permission mode for workers (default, acceptEdits, bypassPermissions, plan, auto).
    #[arg(short, long, default_value = "plan")]
    permission_mode: String,

    /// Enable git worktree isolation for workers.
    #[arg(short = 'w', long)]
    worktree: bool,

    /// Disable built-in skills.
    #[arg(long)]
    no_builtins: bool,
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
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::from_default_env()
                .add_directive("claude_pool=info".parse()?)
                .add_directive("claude_pool_server=info".parse()?),
        )
        .with_writer(std::io::stderr)
        .init();

    let cli = Cli::parse();

    let config = GlobalWorkerConfig {
        model: cli.model,
        effort: cli.effort.and_then(|e| parse_effort(&e)),
        budget_microdollars: cli.budget_usd.map(|b| (b * 1_000_000.0) as u64),
        system_prompt: cli.system_prompt,
        permission_mode: Some(parse_permission_mode(&cli.permission_mode)),
        worktree_isolation: cli.worktree,
        ..Default::default()
    };

    let claude = claude_wrapper::Claude::builder().build()?;

    let pool = Pool::builder_with_store(claude, InMemoryStore::new())
        .workers(cli.workers)
        .config(config)
        .build()
        .await
        .map_err(|e| format!("failed to build pool: {e}"))?;

    let skills = if cli.no_builtins {
        SkillRegistry::new()
    } else {
        SkillRegistry::with_builtins()
    };

    let state = Arc::new(State { pool, skills });

    let tool_list = tools::all_tools(&state);
    let resource_list = resources::all_resources(&state);
    let template_list = resources::all_templates(&state);
    let prompt_list = prompts::skill_prompts(&state.skills);

    let mut router = McpRouter::new()
        .server_info("claude-pool", env!("CARGO_PKG_VERSION"))
        .instructions(
            "Claude worker pool. Use pool_run to execute tasks synchronously, \
             pool_submit/pool_result for async. pool_fan_out for parallel execution. \
             pool_chain for synchronous sequential pipelines, pool_submit_chain/pool_chain_result \
             for async chains with per-step progress tracking. context_set/get/list for shared state. \
             pool_configure_worker to set worker identity. \
             Skills are available as prompts (code_review, implement, write_tests, refactor, summarize). \
             To run a worker or chain on a recurring schedule, use Claude Code's /loop command \
             (e.g. `/loop 30m pool_submit_chain ...`). The pool server is stateless and reactive; \
             scheduling is handled by the client.",
        )
        .tools(tool_list)
        .resources(resource_list)
        .prompts(prompt_list);

    for template in template_list {
        router = router.resource_template(template);
    }

    tracing::info!(workers = cli.workers, "claude-pool-server starting");

    let mut transport = StdioTransport::new(router);
    transport.run().await?;

    Ok(())
}
