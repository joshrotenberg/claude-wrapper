//! Async-turn flow tests.
//!
//! Until step 4 lands the `turn_get` / `turn_wait` / `turn_cancel`
//! tools, these tests exercise the new async `chat_send` over MCP
//! and reach into the library's TurnRegistry via the test client's
//! shared state. The wire test for the full turn lifecycle happens
//! once those tools exist.
//!
//! Live tests gated `#[ignore]`. Run with `--ignored`.

use claude_server::{ServerConfig, build_router};
use tower_mcp::TestClient;

fn cfg() -> ServerConfig {
    ServerConfig::default()
}

#[tokio::test]
async fn chat_send_unknown_chat_errors_synchronously() {
    let router = build_router(cfg()).expect("router built");
    let mut client = TestClient::from_router(router);
    client.initialize().await;

    // We never call chat_open, so this chat_id is invalid. The
    // async tool should reject before promising a turn_id rather
    // than fire-and-forget into a guaranteed-to-fail turn.
    let result = client
        .call_tool(
            "chat_send",
            serde_json::json!({
                "chat_id": "chat_does_not_exist",
                "prompt": "hi",
            }),
        )
        .await;
    let text = result.all_text();
    assert!(
        text.contains("no chat with id"),
        "expected error envelope, got {text}"
    );
}

#[tokio::test]
#[ignore = "spawns real claude binary"]
async fn live_chat_send_returns_turn_id_immediately() {
    use std::time::Instant;

    let router = build_router(cfg()).expect("router built");
    let mut client = TestClient::from_router(router);
    client.initialize().await;

    let open = client
        .call_tool("chat_open", serde_json::json!({"model": "haiku"}))
        .await;
    let body: serde_json::Value = serde_json::from_str(&open.all_text()).expect("open json");
    let chat_id = body["chat_id"].as_str().unwrap().to_string();

    // The async fire should return in well under a second. A real
    // haiku turn takes 1-5+ seconds; if we're seeing that on the
    // fire path, the tool is blocking instead of spawning.
    let started = Instant::now();
    let fire = client
        .call_tool(
            "chat_send",
            serde_json::json!({
                "chat_id": chat_id.clone(),
                "prompt": "Reply with exactly the word ASYNC and nothing else.",
            }),
        )
        .await;
    let fired_in = started.elapsed();
    eprintln!("chat_send returned in {fired_in:?}");
    let fbody: serde_json::Value = serde_json::from_str(&fire.all_text()).expect("fire json");
    let turn_id = fbody["turn_id"].as_str().expect("turn_id").to_string();
    eprintln!("turn_id: {turn_id}");
    assert_eq!(fbody["chat_id"].as_str(), Some(chat_id.as_str()));

    // 500ms is plenty -- spawn + Conversation::lock should be sub-ms.
    // If we see >500ms we're almost certainly blocking on the actual
    // claude turn.
    assert!(
        fired_in < std::time::Duration::from_millis(500),
        "chat_send blocked for {fired_in:?}; should fire-and-return"
    );

    // Once step 4 lands `turn_wait` we'll await completion here.
    // Until then we just let the turn finish in the background by
    // sleeping briefly so the worker can publish before close.
    tokio::time::sleep(std::time::Duration::from_secs(8)).await;

    let _ = client
        .call_tool("chat_close", serde_json::json!({"chat_id": chat_id}))
        .await;
}

#[tokio::test]
#[ignore = "spawns real claude binary"]
async fn live_two_chats_run_in_parallel() {
    use std::time::Instant;

    let router = build_router(cfg()).expect("router built");
    let mut client = TestClient::from_router(router);
    client.initialize().await;

    async fn open_one(client: &mut TestClient) -> String {
        let body = client
            .call_tool("chat_open", serde_json::json!({"model": "haiku"}))
            .await;
        let v: serde_json::Value = serde_json::from_str(&body.all_text()).expect("open json");
        v["chat_id"].as_str().unwrap().to_string()
    }

    let chat_a = open_one(&mut client).await;
    let chat_b = open_one(&mut client).await;

    // Fire two async sends back to back. If they truly run in
    // parallel, the second fire returns almost immediately --
    // it's a different chat with its own Conversation mutex.
    let started = Instant::now();
    let _ = client
        .call_tool(
            "chat_send",
            serde_json::json!({
                "chat_id": chat_a.clone(),
                "prompt": "Reply with exactly the word AAA and nothing else.",
            }),
        )
        .await;
    let after_first = started.elapsed();
    let _ = client
        .call_tool(
            "chat_send",
            serde_json::json!({
                "chat_id": chat_b.clone(),
                "prompt": "Reply with exactly the word BBB and nothing else.",
            }),
        )
        .await;
    let after_second = started.elapsed();
    eprintln!("first fire: {after_first:?}, both fires: {after_second:?}");

    // Both fires combined should still be under a second.
    assert!(
        after_second < std::time::Duration::from_millis(1500),
        "second chat's fire blocked behind first; total {after_second:?}"
    );

    // Let the turns run in the background.
    tokio::time::sleep(std::time::Duration::from_secs(8)).await;

    let _ = client
        .call_tool("chat_close", serde_json::json!({"chat_id": chat_a}))
        .await;
    let _ = client
        .call_tool("chat_close", serde_json::json!({"chat_id": chat_b}))
        .await;
}
