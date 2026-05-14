//! The canonical "raw, no-frills MCP server CLI" for claude-server.
//!
//! This example exists to demonstrate the library API in its
//! simplest deployable shape. The library does all the work; the
//! example is just `clap + tokio + one library call + a transport`.
//! Copy this file into your own crate and adapt as needed.
//!
//! Run with `cargo run --example server -- serve` (or `tools` /
//! `help`).

use std::path::PathBuf;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};

use claude_server::{ServerConfig, build_router, registered_tools};
use tower_mcp::StdioTransport;

/// Raw MCP server CLI for `claude-server`. Wires the library router
/// to a transport. Reach for this when you want a working binary;
/// reach for [`claude_server::build_router`] directly when you want
/// to embed.
#[derive(Debug, Parser)]
#[command(name = "claude-server", version, about)]
struct Cli {
    /// Path to a TOML ServerConfig. Defaults are used if omitted.
    #[arg(long, short = 'c', global = true)]
    config: Option<PathBuf>,

    #[command(subcommand)]
    command: Option<Cmd>,
}

#[derive(Debug, Subcommand)]
enum Cmd {
    /// Serve over stdio (default).
    Serve,
    /// Print the registered tool surface as JSON and exit.
    Tools,
}

#[tokio::main(flavor = "multi_thread", worker_threads = 4)]
async fn main() -> Result<()> {
    init_tracing();

    let cli = Cli::parse();
    let cfg = load_config(cli.config).await?;

    match cli.command.unwrap_or(Cmd::Serve) {
        Cmd::Serve => {
            let router = build_router(cfg).context("build_router")?;
            let mut transport = StdioTransport::new(router);
            transport.run().await.context("stdio transport")?;
        }
        Cmd::Tools => {
            let tools = registered_tools(cfg).context("registered_tools")?;
            let body: Vec<_> = tools
                .into_iter()
                .map(|t| {
                    serde_json::json!({
                        "name": t.name,
                        "description": t.description,
                    })
                })
                .collect();
            println!("{}", serde_json::to_string_pretty(&body)?);
        }
    }
    Ok(())
}

async fn load_config(path: Option<PathBuf>) -> Result<ServerConfig> {
    let Some(p) = path else {
        return Ok(ServerConfig::default());
    };
    let text = tokio::fs::read_to_string(&p)
        .await
        .with_context(|| format!("reading {}", p.display()))?;
    toml::from_str(&text).with_context(|| format!("parsing {}", p.display()))
}

fn init_tracing() {
    use tracing_subscriber::{EnvFilter, fmt};
    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("info,claude_server=info"));
    let _ = fmt()
        .with_env_filter(filter)
        .with_writer(std::io::stderr)
        .with_target(false)
        .with_ansi(false)
        .try_init();
}
