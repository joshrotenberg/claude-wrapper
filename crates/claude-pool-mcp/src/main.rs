//! claude-pool-mcp: thin MCP server exposing claude-pool as tools.
//!
//! Every tool maps 1:1 to a pool method. No business logic, no planning,
//! no decisioner. The client decides what to run. The server dispatches.

mod tools;

use std::sync::Arc;

use clap::Parser;
use claude_pool::types::{Effort, PermissionMode, ScalingConfig};
use claude_pool::{Pool, PoolConfig};
use claude_wrapper::Claude;
use tower_mcp::McpRouter;
use tower_mcp::transport::StdioTransport;

/// Thin MCP server exposing claude-pool as tools.
#[derive(Parser, Debug)]
#[command(name = "claude-pool-mcp", version)]
struct Cli {
    /// Number of slots to spawn.
    #[arg(short = 'n', long, default_value_t = 2)]
    slots: usize,

    /// Default model for all slots.
    #[arg(short, long)]
    model: Option<String>,

    /// Default effort level (low, medium, high, max).
    #[arg(short, long)]
    effort: Option<String>,

    /// Total budget cap in USD.
    #[arg(short, long)]
    budget_usd: Option<f64>,

    /// System prompt for all slots.
    #[arg(short, long)]
    system_prompt: Option<String>,

    /// Permission mode (plan, auto, default, acceptEdits, bypassPermissions, dontAsk).
    #[arg(short, long, default_value = "plan")]
    permission_mode: String,

    /// Minimum slots floor.
    #[arg(long, default_value_t = 1)]
    min_slots: usize,

    /// Maximum slots ceiling.
    #[arg(long, default_value_t = 16)]
    max_slots: usize,
}

fn parse_effort(s: &str) -> Option<Effort> {
    match s {
        "low" => Some(Effort::Low),
        "medium" => Some(Effort::Medium),
        "high" => Some(Effort::High),
        "max" => Some(Effort::Max),
        _ => None,
    }
}

fn parse_permission_mode(s: &str) -> PermissionMode {
    match s {
        "plan" => PermissionMode::Plan,
        "auto" => PermissionMode::Auto,
        "default" => PermissionMode::Default,
        "acceptEdits" => PermissionMode::AcceptEdits,
        "bypassPermissions" => PermissionMode::BypassPermissions,
        "dontAsk" => PermissionMode::DontAsk,
        _ => PermissionMode::Plan,
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .with_writer(std::io::stderr)
        .init();

    let cli = Cli::parse();

    let claude = Claude::builder().build()?;

    let pool_config = PoolConfig {
        model: cli.model.clone(),
        effort: cli.effort.as_deref().and_then(parse_effort),
        budget_microdollars: cli.budget_usd.map(|usd| (usd * 1_000_000.0) as u64),
        system_prompt: cli.system_prompt.clone(),
        permission_mode: Some(parse_permission_mode(&cli.permission_mode)),
        scaling: ScalingConfig {
            min_slots: cli.min_slots,
            max_slots: cli.max_slots,
        },
        ..Default::default()
    };

    let pool = Pool::builder(claude)
        .slots(cli.slots)
        .config(pool_config)
        .build()
        .await?;

    let state = Arc::new(pool);

    let router = McpRouter::new()
        .server_info("claude-pool-mcp", env!("CARGO_PKG_VERSION"))
        .instructions(
            "Pool orchestration server. Use pool_run for single tasks, \
             pool_fan_out for parallel work, pool_chain for sequential pipelines, \
             pool_auto to let the router decide. All tools map 1:1 to pool methods.",
        )
        .tools(tools::all_tools(Arc::clone(&state)));

    tracing::info!(
        slots = cli.slots,
        model = ?cli.model,
        "claude-pool-mcp starting"
    );

    StdioTransport::new(router).run().await?;

    Ok(())
}
