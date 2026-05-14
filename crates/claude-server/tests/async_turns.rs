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
async fn turn_get_unknown_id_errors() {
    let router = build_router(cfg()).expect("router built");
    let mut client = TestClient::from_router(router);
    client.initialize().await;
    let r = client
        .call_tool("turn_get", serde_json::json!({"turn_id": "turn_nope"}))
        .await;
    let text = r.all_text();
    assert!(
        text.contains("no turn with id"),
        "expected unknown-turn error, got {text}"
    );
}

#[tokio::test]
async fn turn_cancel_unknown_id_is_a_noop() {
    let router = build_router(cfg()).expect("router built");
    let mut client = TestClient::from_router(router);
    client.initialize().await;
    let r = client
        .call_tool("turn_cancel", serde_json::json!({"turn_id": "turn_nope"}))
        .await;
    let v: serde_json::Value = serde_json::from_str(&r.all_text()).expect("json");
    assert_eq!(v["ok"], serde_json::json!(true));
    assert_eq!(v["existed"], serde_json::json!(false));
}

#[tokio::test]
async fn turn_list_starts_empty() {
    let router = build_router(cfg()).expect("router built");
    let mut client = TestClient::from_router(router);
    client.initialize().await;
    let r = client.call_tool("turn_list", serde_json::json!({})).await;
    let v: serde_json::Value = serde_json::from_str(&r.all_text()).expect("json");
    assert_eq!(v["turns"].as_array().map(|a| a.len()), Some(0));
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
async fn live_chat_send_then_turn_wait_settles() {
    let router = build_router(cfg()).expect("router built");
    let mut client = TestClient::from_router(router);
    client.initialize().await;

    let open = client
        .call_tool("chat_open", serde_json::json!({"model": "haiku"}))
        .await;
    let body: serde_json::Value = serde_json::from_str(&open.all_text()).expect("open json");
    let chat_id = body["chat_id"].as_str().unwrap().to_string();

    // Fire async.
    let fire = client
        .call_tool(
            "chat_send",
            serde_json::json!({
                "chat_id": chat_id.clone(),
                "prompt": "Reply with exactly the word WAITED and nothing else.",
            }),
        )
        .await;
    let fbody: serde_json::Value = serde_json::from_str(&fire.all_text()).expect("fire json");
    let turn_id = fbody["turn_id"].as_str().unwrap().to_string();

    // turn_get right away: should be running.
    let pre = client
        .call_tool("turn_get", serde_json::json!({"turn_id": turn_id.clone()}))
        .await;
    let pre_v: serde_json::Value = serde_json::from_str(&pre.all_text()).expect("get json");
    assert_eq!(
        pre_v["status"],
        serde_json::json!("running"),
        "pre: {pre_v}"
    );

    // Wait with a generous timeout.
    let waited = client
        .call_tool(
            "turn_wait",
            serde_json::json!({"turn_id": turn_id.clone(), "timeout_secs": 30.0}),
        )
        .await;
    let wv: serde_json::Value = serde_json::from_str(&waited.all_text()).expect("wait json");
    eprintln!("settled: {wv}");
    assert_eq!(wv["status"], serde_json::json!("done"));
    let result_text = wv["result"]["result"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    assert!(
        result_text.contains("WAITED"),
        "expected WAITED, got {result_text:?}"
    );

    // turn_list should show the settled turn.
    let list = client.call_tool("turn_list", serde_json::json!({})).await;
    let lv: serde_json::Value = serde_json::from_str(&list.all_text()).expect("list json");
    let turns = lv["turns"].as_array().expect("array");
    assert!(turns.iter().any(|t| t["turn_id"] == turn_id));

    let _ = client
        .call_tool("chat_close", serde_json::json!({"chat_id": chat_id}))
        .await;
}

#[tokio::test]
#[ignore = "spawns real claude binary"]
async fn live_turn_wait_timeout_then_settle() {
    let router = build_router(cfg()).expect("router built");
    let mut client = TestClient::from_router(router);
    client.initialize().await;

    let open = client
        .call_tool("chat_open", serde_json::json!({"model": "haiku"}))
        .await;
    let chat_id = serde_json::from_str::<serde_json::Value>(&open.all_text()).unwrap()["chat_id"]
        .as_str()
        .unwrap()
        .to_string();

    let fire = client
        .call_tool(
            "chat_send",
            serde_json::json!({
                "chat_id": chat_id.clone(),
                "prompt": "Reply with the word ECHO.",
            }),
        )
        .await;
    let turn_id = serde_json::from_str::<serde_json::Value>(&fire.all_text()).unwrap()["turn_id"]
        .as_str()
        .unwrap()
        .to_string();

    // Tight 50ms timeout -- a real haiku turn is at least ~1s.
    let early = client
        .call_tool(
            "turn_wait",
            serde_json::json!({"turn_id": turn_id.clone(), "timeout_secs": 0.05}),
        )
        .await;
    let ev: serde_json::Value = serde_json::from_str(&early.all_text()).expect("json");
    assert_eq!(ev["status"], serde_json::json!("timeout"), "ev: {ev}");

    // Now wait properly.
    let settled = client
        .call_tool(
            "turn_wait",
            serde_json::json!({"turn_id": turn_id, "timeout_secs": 30.0}),
        )
        .await;
    let sv: serde_json::Value = serde_json::from_str(&settled.all_text()).expect("json");
    assert_eq!(sv["status"], serde_json::json!("done"));

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
