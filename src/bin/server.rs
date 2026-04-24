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

use std::path::PathBuf;
use std::process::ExitCode;

use claude_wrapper::server::{ServerConfig, build_router};

#[derive(Debug)]
struct Args {
    config: Option<PathBuf>,
}

fn parse_args() -> Result<Args, String> {
    let mut iter = std::env::args().skip(1);
    let mut config: Option<PathBuf> = None;
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--config" => {
                config = Some(PathBuf::from(
                    iter.next()
                        .ok_or_else(|| "expected path after --config".to_string())?,
                ));
            }
            "-h" | "--help" => {
                eprintln!(
                    "claude-server -- MCP server over the Claude Code CLI\n\n\
                     USAGE:\n  claude-server [--config <path>]\n\n\
                     With no --config, sensible defaults apply (claude on PATH,\n\
                     no budget cap, all tools registered)."
                );
                std::process::exit(0);
            }
            other => return Err(format!("unknown argument: {other}")),
        }
    }
    Ok(Args { config })
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

    let config = match args.config {
        Some(path) => match ServerConfig::from_path(&path) {
            Ok(c) => c,
            Err(e) => {
                eprintln!("failed to load config from {}: {e}", path.display());
                return ExitCode::from(2);
            }
        },
        None => ServerConfig::default(),
    };

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
