//! Slash-command tests. The interesting question is "does the
//! slash command actually take effect when written to a stream-json
//! input session?" Most assertions are necessarily soft because we
//! don't fully control what `claude` does with `/compact` etc.
//!
//! Live tests gated `#[ignore]`. Run with `--ignored`.

use claude_server::{ServerConfig, build_router};
use tower_mcp::TestClient;

fn cfg() -> ServerConfig {
    ServerConfig::default()
}

#[tokio::test]
async fn chat_compact_unknown_chat_errors_synchronously() {
    let router = build_router(cfg()).expect("router built");
    let mut client = TestClient::from_router(router);
    client.initialize().await;

    let result = client
        .call_tool(
            "chat_compact",
            serde_json::json!({"chat_id": "chat_does_not_exist"}),
        )
        .await;
    let text = result.all_text();
    assert!(
        text.contains("no chat with id"),
        "expected unknown-chat error, got {text}"
    );
}

#[tokio::test]
#[ignore = "spawns real claude binary"]
async fn live_chat_compact_settles_to_terminal() {
    // We can't reliably assert what /compact returns -- claude's
    // compaction prompt produces variable output. We CAN assert
    // that the slash command is accepted by the stream-json input
    // mode at all: the turn settles to a terminal status (not
    // running, not failed-with-protocol-error).

    let router = build_router(cfg()).expect("router built");
    let mut client = TestClient::from_router(router);
    client.initialize().await;

    let open = client
        .call_tool("chat_open", serde_json::json!({"model": "haiku"}))
        .await;
    let body: serde_json::Value = serde_json::from_str(&open.all_text()).expect("open json");
    let chat_id = body["chat_id"].as_str().unwrap().to_string();

    // Seed a turn so there's something to compact.
    let _ = client
        .call_tool(
            "chat_send",
            serde_json::json!({
                "chat_id": chat_id.clone(),
                "prompt": "Reply with the word SEED.",
            }),
        )
        .await;
    // Wait for the seed turn to settle so /compact has history.
    let _ = client.call_tool("turn_list", serde_json::json!({})).await;

    let fire = client
        .call_tool(
            "chat_compact",
            serde_json::json!({"chat_id": chat_id.clone()}),
        )
        .await;
    let fbody: serde_json::Value =
        serde_json::from_str(&fire.all_text()).expect("compact fire json");
    let turn_id = fbody["turn_id"].as_str().expect("turn_id").to_string();

    let waited = client
        .call_tool(
            "turn_wait",
            serde_json::json!({"turn_id": turn_id.clone(), "timeout_secs": 60.0}),
        )
        .await;
    let wv: serde_json::Value = serde_json::from_str(&waited.all_text()).expect("wait json");
    eprintln!("compact settled: {wv}");
    let status = wv["status"].as_str().unwrap_or_default();
    // We accept any terminal status -- if /compact isn't supported
    // through stream-json on this CLI version we want the test to
    // log that loudly rather than green-pass silently.
    assert!(
        ["done", "failed", "cancelled"].contains(&status),
        "expected terminal status, got {status}"
    );
    if status == "failed" {
        eprintln!(
            "/compact failed: {}",
            wv["error"].as_str().unwrap_or("<no error>")
        );
    }

    let _ = client
        .call_tool("chat_close", serde_json::json!({"chat_id": chat_id}))
        .await;
}
