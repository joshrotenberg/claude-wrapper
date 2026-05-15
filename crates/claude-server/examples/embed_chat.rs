//! Embedded-mode demo: chat lifecycle via direct library calls.
//!
//! No socket. No transport. No subprocess of `claude-server`. Just
//! `build_router(...)` + an in-process JSON-RPC dispatcher
//! (`TestClient`) calling tools directly.
//!
//! This is the shape consuming Rust apps would adopt to embed
//! claude-server -- a CLI subcommand backend, an Elixir NIF
//! sidecar, an agent runtime that wants the chat machinery
//! without standing up another process.
//!
//! Note: this DOES spawn a real `claude` subprocess (the duplex
//! child held open by the chat). It runs against your real
//! `~/.claude/` config and bills your account. We pin haiku to
//! keep cost negligible (this run typically costs <$0.05).
//!
//! Run with: `cargo run --example embed_chat`

use claude_server::{ServerConfig, build_router};
use tower_mcp::TestClient;

#[tokio::main(flavor = "multi_thread", worker_threads = 4)]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Step 1: build the router. This is the only library API call
    // that matters -- everything below is just standard MCP
    // dispatching.
    let router = build_router(ServerConfig::default())?;

    // Step 2: in-process dispatcher. No JSON-RPC over stdio /
    // HTTP -- the request goes straight into the router's tower
    // Service.
    let mut client = TestClient::from_router(router);
    client.initialize().await;

    // Step 3: open a chat. async-by-default returns immediately
    // with a chat_id we use for subsequent calls.
    println!("opening chat...");
    let open = client
        .call_tool(
            "chat_open",
            serde_json::json!({
                "model": "haiku",
                "max_cost_usd": 0.5,
                "system_prompt": "You're being driven by an embedded Rust example. Be brief.",
            }),
        )
        .await;
    let body: serde_json::Value = serde_json::from_str(&open.all_text())?;
    let chat_id = body["chat_id"].as_str().expect("chat_id").to_string();
    println!("chat_id: {chat_id}");

    // Step 4: fire a turn (async). chat_send returns a turn_id
    // immediately; the turn runs in the background.
    println!("firing turn (async)...");
    let fire = client
        .call_tool(
            "chat_send",
            serde_json::json!({
                "chat_id": chat_id,
                "prompt": "In one sentence, what's the deal with embedded MCP servers?",
            }),
        )
        .await;
    let fire_body: serde_json::Value = serde_json::from_str(&fire.all_text())?;
    let turn_id = fire_body["turn_id"].as_str().expect("turn_id").to_string();
    println!("turn_id: {turn_id}");

    // Step 5: block until the turn settles (or timeout).
    println!("waiting for turn to settle...");
    let waited = client
        .call_tool(
            "turn_wait",
            serde_json::json!({"turn_id": turn_id, "timeout_secs": 60.0}),
        )
        .await;
    let wbody: serde_json::Value = serde_json::from_str(&waited.all_text())?;
    println!("status: {}", wbody["status"].as_str().unwrap_or("unknown"));
    if let Some(text) = wbody["result"]["result"].as_str() {
        println!("\nassistant: {text}\n");
    }
    if let Some(cost) = wbody["result"]["turn_cost_usd"].as_f64() {
        println!("turn cost: ${cost:.4}");
    }

    // Step 6: peek at process metrics. We've fired one turn; the
    // counters should reflect it.
    let metrics = client
        .call_tool("metrics_summary", serde_json::json!({}))
        .await;
    println!("\nmetrics: {}", metrics.all_text());

    // Step 7: clean up.
    let _ = client
        .call_tool("chat_close", serde_json::json!({"chat_id": chat_id}))
        .await;
    println!("\nchat closed.");

    Ok(())
}
