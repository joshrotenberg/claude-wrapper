//! `claude-server` -- stdio MCP server exposing the Claude Code CLI.
//!
//! Reads optional config from a TOML file and exposes the
//! `claude.*` and `agent.*` MCP tools defined in
//! [`claude_wrapper::server`]. Suitable for registration as an MCP
//! server in Claude Code's `~/.claude/settings.json` or in any other
//! MCP client.
//!
//! Trace logs go to stderr so they don't collide with the stdio
//! JSON-RPC transport on stdout.
//!
//! # Subcommands
//!
//! - (default, no subcommand): run the stdio MCP server.
//! - `tools`: list every tool the server would register under the
//!   given config, then exit. Useful for debugging "what changes
//!   when I flip `allow_mutations = true`?" style questions without
//!   spinning up a real MCP session.

use std::path::PathBuf;
use std::process::ExitCode;

use claude_wrapper::server::{ServerConfig, build_router, registered_tools};

#[derive(Debug)]
enum Subcommand {
    /// Default: run the stdio MCP server.
    Serve,
    /// Print the registered tool list (name + description) and exit.
    Tools,
}

#[derive(Debug)]
struct Args {
    config: Option<PathBuf>,
    subcommand: Subcommand,
}

fn parse_args() -> Result<Args, String> {
    let mut iter = std::env::args().skip(1);
    let mut config: Option<PathBuf> = None;
    let mut subcommand = Subcommand::Serve;
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--config" => {
                config = Some(PathBuf::from(
                    iter.next()
                        .ok_or_else(|| "expected path after --config".to_string())?,
                ));
            }
            "tools" => subcommand = Subcommand::Tools,
            "-h" | "--help" => {
                eprintln!(
                    "claude-server -- MCP server over the Claude Code CLI\n\n\
                     USAGE:\n  \
                     claude-server [--config <path>]                 # run stdio MCP server\n  \
                     claude-server tools [--config <path>]           # print registered tools and exit\n  \
                     claude-server --help\n\n\
                     With no --config, sensible defaults apply (claude on PATH,\n\
                     no budget cap, all tools registered, no sandbox)."
                );
                std::process::exit(0);
            }
            other => return Err(format!("unknown argument: {other}")),
        }
    }
    Ok(Args { config, subcommand })
}

fn load_config(path: Option<PathBuf>) -> Result<ServerConfig, String> {
    match path {
        Some(p) => ServerConfig::from_path(&p)
            .map_err(|e| format!("failed to load config from {}: {e}", p.display())),
        None => Ok(ServerConfig::default()),
    }
}

#[tokio::main]
async fn main() -> ExitCode {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "claude_wrapper=info,tower_mcp=info".into()),
        )
        .with_writer(std::io::stderr)
        .init();

    let args = match parse_args() {
        Ok(a) => a,
        Err(e) => {
            eprintln!("{e}");
            return ExitCode::from(2);
        }
    };

    let config = match load_config(args.config) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("{e}");
            return ExitCode::from(2);
        }
    };

    match args.subcommand {
        Subcommand::Tools => run_tools(config),
        Subcommand::Serve => run_serve(config).await,
    }
}

fn run_tools(config: ServerConfig) -> ExitCode {
    let tools = match registered_tools(config) {
        Ok(t) => t,
        Err(e) => {
            eprintln!("failed to enumerate tools: {e}");
            return ExitCode::from(1);
        }
    };
    println!("{} tools registered:", tools.len());
    let name_width = tools.iter().map(|t| t.name.len()).max().unwrap_or(0);
    for t in tools {
        let desc = t.description.as_deref().unwrap_or("");
        // Trim long descriptions to one line for readability.
        let first_line = desc.lines().next().unwrap_or("");
        println!("  {:<width$}  {}", t.name, first_line, width = name_width);
    }
    ExitCode::SUCCESS
}

async fn run_serve(config: ServerConfig) -> ExitCode {
    let router = match build_router(config) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("failed to build router: {e}");
            return ExitCode::from(1);
        }
    };

    tracing::info!("claude-server stdio MCP transport ready");
    if let Err(e) = tower_mcp::StdioTransport::new(router).run().await {
        eprintln!("server error: {e}");
        return ExitCode::from(1);
    }

    ExitCode::SUCCESS
}
