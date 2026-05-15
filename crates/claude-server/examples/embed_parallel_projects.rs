//! Embedded demo: one server, two chats, two projects, in parallel.
//!
//! Demonstrates the "talk to another project" pattern with
//! `chat_open(working_dir: ...)`. A single `claude-server` (here
//! embedded directly, no transport) hosts two chats in different
//! project roots simultaneously. The chats share nothing -- each
//! gets its own duplex `claude` subprocess in its own working
//! directory -- but they're coordinated through one library
//! instance.
//!
//! Use case: a CLI / agent runtime that operates across multiple
//! project trees from a single entrypoint without standing up a
//! separate claude-server per project.
//!
//! Run with: `cargo run --example embed_parallel_projects`
//!
//! Spawns two real `claude` subprocesses (haiku, ~$0.05 each).

use claude_server::{ServerConfig, build_router};
use tower_mcp::TestClient;

#[tokio::main(flavor = "multi_thread", worker_threads = 4)]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let router = build_router(ServerConfig::default())?;
    let mut client = TestClient::from_router(router);
    client.initialize().await;

    // Two project roots: this repo + the parent directory.
    // Substitute any two directories you care about.
    let cwd = std::env::current_dir()?;
    let parent = cwd.parent().unwrap_or(&cwd).to_path_buf();

    println!("opening chat A in {}", cwd.display());
    let chat_a = open_chat(&mut client, &cwd, "haiku").await?;
    println!("opening chat B in {}", parent.display());
    let chat_b = open_chat(&mut client, &parent, "haiku").await?;

    // Fire two turns back-to-back. async-by-default means both
    // return turn_ids immediately and run in parallel -- different
    // chats don't serialize.
    println!("\nfiring two turns in parallel...");
    let fire_a = client
        .call_tool(
            "chat_send",
            serde_json::json!({
                "chat_id": chat_a,
                "prompt": "What's your current working directory? Reply in one short sentence.",
            }),
        )
        .await;
    let fire_b = client
        .call_tool(
            "chat_send",
            serde_json::json!({
                "chat_id": chat_b,
                "prompt": "What's your current working directory? Reply in one short sentence.",
            }),
        )
        .await;
    let turn_a = serde_json::from_str::<serde_json::Value>(&fire_a.all_text())?["turn_id"]
        .as_str()
        .unwrap()
        .to_string();
    let turn_b = serde_json::from_str::<serde_json::Value>(&fire_b.all_text())?["turn_id"]
        .as_str()
        .unwrap()
        .to_string();

    // Both chats run in parallel on the server side -- different
    // duplex subprocesses, no shared state. Waits are sequential
    // here for demo simplicity (TestClient::call_tool needs &mut),
    // but the actual work is concurrent: by the time we get to
    // wait B, it's either already done or about to be.
    println!("waiting for both turns to settle...");
    let settled_a = wait_for(&mut client, &turn_a).await?;
    let settled_b = wait_for(&mut client, &turn_b).await?;

    println!(
        "\nchat A ({}):\n  {}",
        cwd.display(),
        settled_a["result"]["result"].as_str().unwrap_or("")
    );
    println!(
        "\nchat B ({}):\n  {}",
        parent.display(),
        settled_b["result"]["result"].as_str().unwrap_or("")
    );

    // Clean up.
    let _ = client
        .call_tool("chat_close", serde_json::json!({"chat_id": chat_a}))
        .await;
    let _ = client
        .call_tool("chat_close", serde_json::json!({"chat_id": chat_b}))
        .await;

    Ok(())
}

async fn open_chat(
    client: &mut TestClient,
    working_dir: &std::path::Path,
    model: &str,
) -> Result<String, Box<dyn std::error::Error>> {
    let result = client
        .call_tool(
            "chat_open",
            serde_json::json!({
                "model": model,
                "working_dir": working_dir,
                "max_cost_usd": 0.5,
                "system_prompt": "You're being driven by an embedded Rust example. Be brief.",
            }),
        )
        .await;
    let body: serde_json::Value = serde_json::from_str(&result.all_text())?;
    Ok(body["chat_id"].as_str().expect("chat_id").to_string())
}

async fn wait_for(
    client: &mut TestClient,
    turn_id: &str,
) -> Result<serde_json::Value, Box<dyn std::error::Error>> {
    let result = client
        .call_tool(
            "turn_wait",
            serde_json::json!({"turn_id": turn_id, "timeout_secs": 60.0}),
        )
        .await;
    Ok(serde_json::from_str(&result.all_text())?)
}
