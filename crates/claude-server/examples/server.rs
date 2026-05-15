//! The canonical "raw, no-frills MCP server CLI" for claude-server.
//!
//! This example exists to demonstrate the library API in its
//! simplest deployable shape. The library does all the work; the
//! example is just `clap + tokio + one library call + a transport`.
//! Copy this file into your own crate and adapt as needed.
//!
//! Run with `cargo run --example server -- serve` (or `tools` /
//! `help`).

use std::net::SocketAddr;
use std::path::PathBuf;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};

use claude_server::{
    ServerConfig, build_router, build_router_with_notification_sender, notification_channel,
    registered_tools,
};
use tower::Layer;
use tower_mcp::HttpTransport;
use tower_mcp::middleware::McpTracingLayer;
use tower_mcp::transport::stdio::GenericStdioTransport;

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
    /// Serve over HTTP via axum. Localhost-only by default.
    ServeHttp {
        /// Bind address. Defaults to 127.0.0.1:7800. Be intentional
        /// before binding to anything other than localhost.
        #[arg(long, default_value = "127.0.0.1:7800")]
        bind: SocketAddr,
        /// Optional bearer token. When set, every request must
        /// include `Authorization: Bearer <token>` or it gets a 401.
        #[arg(long)]
        bearer: Option<String>,
    },
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
            // Notification-aware construction: we own the channel, share
            // its sender with both the library (so chat workers fire
            // claude://chats/{id} resource updates) and the router; the
            // receiver feeds the stdio transport.
            let (notif_tx, notif_rx) = notification_channel(256);
            let router = build_router_with_notification_sender(cfg, notif_tx)
                .context("build_router_with_notification_sender")?;
            let service = McpTracingLayer::new().layer(router);
            let mut transport = GenericStdioTransport::with_notifications(service, notif_rx);
            transport.run().await.context("stdio transport")?;
        }
        Cmd::ServeHttp { bind, bearer } => {
            // HttpTransport currently creates its own notification
            // channel and overrides the router's, so chat workers that
            // fire `claude://chats/{id}` updates against state's channel
            // won't reach HTTP subscribers today. Stdio works; HTTP
            // subscription support needs an upstream tower-mcp change
            // to accept an external sender.
            let router = build_router(cfg).context("build_router")?;
            let mut app = HttpTransport::new(router)
                .layer(McpTracingLayer::new())
                .into_router();
            if let Some(token) = bearer {
                use axum::middleware;
                app = app.layer(middleware::from_fn(move |req, next| {
                    let token = token.clone();
                    bearer_guard(req, next, token)
                }));
                tracing::info!("bearer auth enabled");
            }
            let listener = tokio::net::TcpListener::bind(bind)
                .await
                .with_context(|| format!("binding {bind}"))?;
            tracing::info!(%bind, "claude-server listening on http");
            axum::serve(listener, app).await.context("axum::serve")?;
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
        .unwrap_or_else(|_| EnvFilter::new("info,claude_server=info,tower_mcp=info"));
    let _ = fmt()
        .with_env_filter(filter)
        .with_writer(std::io::stderr)
        .with_target(false)
        .with_ansi(false)
        .try_init();
}

/// Reject any request whose `Authorization: Bearer <token>` doesn't
/// match the configured token. Constant-time string compare would be
/// nicer but we're not handling untrusted input at scale here.
async fn bearer_guard(
    req: axum::extract::Request,
    next: axum::middleware::Next,
    expected: String,
) -> Result<axum::response::Response, axum::http::StatusCode> {
    let header = req
        .headers()
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.strip_prefix("Bearer "));
    match header {
        Some(t) if t == expected => Ok(next.run(req).await),
        _ => Err(axum::http::StatusCode::UNAUTHORIZED),
    }
}
