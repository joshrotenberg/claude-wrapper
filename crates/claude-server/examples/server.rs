//! Truly-minimal `claude-server` integration recipe.
//!
//! ~50 lines. Build the router, hand it to a stdio transport, run.
//! No subcommands, no flags, no polish. Copy this into your own
//! crate as a starting point when you want to embed.
//!
//! For the polished CLI with subcommands (`tools`, `doctor`,
//! `config`, `install-mcp-config`, `serve-http`), bind safety, log
//! controls, and runtime surface gates, see the `claude-server`
//! binary (`src/bin/claude-server.rs`) instead -- this example is
//! the foundation it grew from.
//!
//! Run with: `cargo run --example server --features full`

use claude_server::{ServerConfig, build_router_with_notification_sender, notification_channel};
use tower::Layer;
use tower_mcp::middleware::McpTracingLayer;
use tower_mcp::transport::stdio::GenericStdioTransport;

#[tokio::main(flavor = "multi_thread", worker_threads = 4)]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // tracing → stderr so stdout stays clean for MCP framing
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_target(false)
        .with_ansi(false)
        .try_init()
        .ok();

    // Notification-aware construction: chat workers fire
    // `claude://chats/{id}` resource updates through `notif_tx`;
    // the stdio transport drains `notif_rx`.
    let (notif_tx, notif_rx) = notification_channel(256);
    let router = build_router_with_notification_sender(ServerConfig::default(), notif_tx)?;
    let service = McpTracingLayer::new().layer(router);
    let mut transport = GenericStdioTransport::with_notifications(service, notif_rx);
    transport.run().await?;
    Ok(())
}
