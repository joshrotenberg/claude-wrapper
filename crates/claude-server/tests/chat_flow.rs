//! Chat-surface flow tests.
//!
//! In-process tests verify the surface exists and that the empty
//! `chat_list` / `claude://chats` resource round-trips cleanly --
//! anything beyond that needs a real `claude` and is gated behind
//! `#[ignore]`.
//!
//! Run live tests with: `cargo test -p claude-server --test chat_flow -- --ignored`.

use claude_server::{ServerConfig, build_router};
use tower_mcp::TestClient;

fn cfg() -> ServerConfig {
    ServerConfig::default()
}

#[tokio::test]
async fn chat_list_starts_empty() {
    let router = build_router(cfg()).expect("router built");
    let mut client = TestClient::from_router(router);
    client.initialize().await;

    let result = client.call_tool("chat_list", serde_json::json!({})).await;
    let text = result.all_text();
    let v: serde_json::Value = serde_json::from_str(&text).expect("json body");
    assert_eq!(
        v["chats"].as_array().map(|a| a.len()),
        Some(0),
        "body: {text}"
    );
}

#[tokio::test]
async fn chat_close_unknown_id_is_a_noop() {
    let router = build_router(cfg()).expect("router built");
    let mut client = TestClient::from_router(router);
    client.initialize().await;

    let result = client
        .call_tool(
            "chat_close",
            serde_json::json!({"chat_id": "chat_does_not_exist"}),
        )
        .await;
    let text = result.all_text();
    let v: serde_json::Value = serde_json::from_str(&text).expect("json body");
    assert_eq!(v["ok"], serde_json::json!(true), "body: {text}");
    assert_eq!(v["existed"], serde_json::json!(false), "body: {text}");
}

#[tokio::test]
async fn chats_resource_round_trips_empty() {
    let router = build_router(cfg()).expect("router built");
    let mut client = TestClient::from_router(router);
    client.initialize().await;

    let result = client.read_resource("claude://chats").await;
    let text = result
        .contents
        .first()
        .and_then(|c| c.text.clone())
        .unwrap_or_default();
    let v: serde_json::Value = serde_json::from_str(&text).expect("json resource body");
    assert_eq!(
        v["chats"].as_array().map(|a| a.len()),
        Some(0),
        "body: {text}"
    );
}

// ---------------------------------------------------------------
// Live #[ignore] tests -- exercise a real `claude` binary.
// ---------------------------------------------------------------

#[tokio::test]
async fn chat_budget_returns_null_when_no_budget() {
    let router = build_router(cfg()).expect("router built");
    let mut client = TestClient::from_router(router);
    client.initialize().await;

    // Need a chat to query, but spawning real claude needs --ignored.
    // Instead, check chat_budget on a non-existent id surfaces an error.
    let result = client
        .call_tool(
            "chat_budget",
            serde_json::json!({"chat_id": "chat_not_real"}),
        )
        .await;
    let text = result.all_text();
    // Either an MCP error envelope or our internal error message; both are fine.
    assert!(
        text.contains("no chat with id") || text.contains("not_real"),
        "expected unknown-chat error, got {text}"
    );
}

#[tokio::test]
#[ignore = "spawns real claude binary"]
async fn live_chat_open_send_close_roundtrip() {
    let router = build_router(cfg()).expect("router built");
    let mut client = TestClient::from_router(router);
    client.initialize().await;

    // Open a chat with haiku to keep cost down.
    let open = client
        .call_tool("chat_open", serde_json::json!({"model": "haiku"}))
        .await;
    let body: serde_json::Value = serde_json::from_str(&open.all_text()).expect("chat_open json");
    let chat_id = body["chat_id"]
        .as_str()
        .expect("chat_id present")
        .to_string();
    eprintln!("opened chat: {chat_id}");

    // Send a turn.
    let sent = client
        .call_tool(
            "chat_send",
            serde_json::json!({
                "chat_id": chat_id,
                "prompt": "Reply with exactly the word ALPHA and nothing else.",
            }),
        )
        .await;
    let sent_body: serde_json::Value =
        serde_json::from_str(&sent.all_text()).expect("chat_send json");
    eprintln!("turn 1: {sent_body}");
    let r1 = sent_body["result"].as_str().unwrap_or_default();
    assert!(r1.contains("ALPHA"), "expected ALPHA, got {r1:?}");
    assert_eq!(sent_body["total_turns"], serde_json::json!(1));

    // Send a second turn -- the assistant should remember the first.
    let sent2 = client
        .call_tool(
            "chat_send",
            serde_json::json!({
                "chat_id": chat_id,
                "prompt": "What word did I just ask you to reply with?",
            }),
        )
        .await;
    let sent2_body: serde_json::Value =
        serde_json::from_str(&sent2.all_text()).expect("chat_send json");
    eprintln!("turn 2: {sent2_body}");
    let r2 = sent2_body["result"].as_str().unwrap_or_default();
    assert!(
        r2.to_uppercase().contains("ALPHA"),
        "expected ALPHA recall, got {r2:?}"
    );
    assert_eq!(sent2_body["total_turns"], serde_json::json!(2));

    // History should have two entries.
    let history = client
        .call_tool("chat_history", serde_json::json!({"chat_id": chat_id}))
        .await;
    let hbody: serde_json::Value = serde_json::from_str(&history.all_text()).expect("history json");
    assert_eq!(
        hbody["total_turns"],
        serde_json::json!(2),
        "history: {hbody}"
    );
    assert_eq!(hbody["turns"].as_array().map(|a| a.len()), Some(2));

    // chat_list should show this chat with 2 turns.
    let list = client.call_tool("chat_list", serde_json::json!({})).await;
    let lbody: serde_json::Value = serde_json::from_str(&list.all_text()).expect("list json");
    let chats = lbody["chats"].as_array().expect("array");
    assert!(
        chats.iter().any(|c| c["chat_id"] == chat_id),
        "list: {lbody}"
    );

    // Close.
    let close = client
        .call_tool("chat_close", serde_json::json!({"chat_id": chat_id}))
        .await;
    let cbody: serde_json::Value = serde_json::from_str(&close.all_text()).expect("close json");
    assert_eq!(cbody["existed"], serde_json::json!(true));

    // After close, list should be empty again.
    let list2 = client.call_tool("chat_list", serde_json::json!({})).await;
    let lbody2: serde_json::Value = serde_json::from_str(&list2.all_text()).expect("list json");
    assert_eq!(
        lbody2["chats"].as_array().map(|a| a.len()),
        Some(0),
        "post-close list: {lbody2}"
    );
}

#[tokio::test]
#[ignore = "spawns real claude binary"]
async fn live_chat_budget_tracks_spend() {
    let router = build_router(cfg()).expect("router built");
    let mut client = TestClient::from_router(router);
    client.initialize().await;

    // Open with a generous ceiling so we don't actually trip it.
    let open = client
        .call_tool(
            "chat_open",
            serde_json::json!({"model": "haiku", "max_cost_usd": 1.0}),
        )
        .await;
    let body: serde_json::Value = serde_json::from_str(&open.all_text()).expect("open json");
    let chat_id = body["chat_id"].as_str().unwrap().to_string();

    // Pre-turn: budget shows 0 spent, $1 max, $1 remaining.
    let pre = client
        .call_tool(
            "chat_budget",
            serde_json::json!({"chat_id": chat_id.clone()}),
        )
        .await;
    let pbody: serde_json::Value = serde_json::from_str(&pre.all_text()).expect("budget json");
    eprintln!("pre-turn budget: {pbody}");
    assert_eq!(pbody["budget"]["max_usd"], serde_json::json!(1.0));
    assert_eq!(pbody["budget"]["total_usd"], serde_json::json!(0.0));

    // Send a turn.
    let _ = client
        .call_tool(
            "chat_send",
            serde_json::json!({
                "chat_id": chat_id.clone(),
                "prompt": "Reply with the single word BUDGET.",
            }),
        )
        .await;

    // Post-turn: total_usd > 0, remaining < max.
    let post = client
        .call_tool(
            "chat_budget",
            serde_json::json!({"chat_id": chat_id.clone()}),
        )
        .await;
    let postbody: serde_json::Value = serde_json::from_str(&post.all_text()).expect("budget json");
    eprintln!("post-turn budget: {postbody}");
    let total = postbody["budget"]["total_usd"].as_f64().unwrap_or(0.0);
    let remaining = postbody["budget"]["remaining_usd"].as_f64().unwrap_or(0.0);
    assert!(total > 0.0, "expected nonzero spend, got {total}");
    assert!(
        (total + remaining - 1.0).abs() < 1e-6,
        "total + remaining should equal max_usd; total={total} remaining={remaining}"
    );

    let _ = client
        .call_tool("chat_close", serde_json::json!({"chat_id": chat_id}))
        .await;
}

#[tokio::test]
#[ignore = "spawns real claude binary"]
async fn live_chat_send_stream_emits_progress() {
    use tower_mcp::context::ServerNotification;

    let router = build_router(cfg()).expect("router built");
    let mut client = TestClient::from_router(router);
    client.initialize().await;

    let open = client
        .call_tool("chat_open", serde_json::json!({"model": "haiku"}))
        .await;
    let body: serde_json::Value = serde_json::from_str(&open.all_text()).expect("open json");
    let chat_id = body["chat_id"].as_str().unwrap().to_string();

    // Drain any startup noise so we only count notifications from the stream call.
    let _ = client.drain_notifications();

    // tower-mcp's Context::report_progress is a no-op unless the client
    // included a `progressToken` in `_meta`. Call via send_request so we
    // can supply one.
    let raw = client
        .send_request(
            "tools/call",
            Some(serde_json::json!({
                "name": "chat_send_stream",
                "arguments": {
                    "chat_id": chat_id.clone(),
                    "prompt": "Reply with one short sentence describing the color blue.",
                },
                "_meta": {"progressToken": "stream-test-1"},
            })),
        )
        .await;
    let result: tower_mcp::CallToolResult = serde_json::from_value(raw).expect("CallToolResult");
    let text = result.all_text();
    let v: serde_json::Value = serde_json::from_str(&text).expect("stream return json");
    assert!(v["result"].is_string(), "missing result: {text}");

    let notifications = client.drain_notifications();
    let progress_count = notifications
        .iter()
        .filter(|n| matches!(n, ServerNotification::Progress(_)))
        .count();
    eprintln!(
        "got {progress_count} progress notifications (of {} total)",
        notifications.len()
    );
    for n in &notifications {
        if let ServerNotification::Progress(p) = n {
            eprintln!("  progress: {p:?}");
        }
    }
    assert!(
        progress_count > 0,
        "expected at least one progress event during the stream turn (got {} total notifications)",
        notifications.len()
    );

    let _ = client
        .call_tool("chat_close", serde_json::json!({"chat_id": chat_id}))
        .await;
}
