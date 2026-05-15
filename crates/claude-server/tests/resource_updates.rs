//! Tests for `notifications/resources/updated` firing on
//! `claude://chats/{id}` after a turn settles.
//!
//! Approach: build the router with our own notification channel
//! (via `build_router_with_notification_sender`), drive tools via
//! `TestClient::from_router`, and drain notifications from the rx
//! we own. TestClient overrides the router-internal sender, but
//! ServerState's notifier is the channel we set up at build time
//! (independent of TestClient's), so chat-worker firings land in
//! our rx as expected.

use claude_server::{ServerConfig, build_router_with_notification_sender, notification_channel};
use tower_mcp::TestClient;
use tower_mcp::context::{NotificationReceiver, ServerNotification};

fn cfg() -> ServerConfig {
    ServerConfig::default()
}

fn drain(rx: &mut NotificationReceiver) -> Vec<ServerNotification> {
    let mut out = Vec::new();
    while let Ok(n) = rx.try_recv() {
        out.push(n);
    }
    out
}

#[tokio::test]
async fn build_router_with_notification_sender_does_not_panic() {
    // Smoke: the new entry point builds a working router.
    let (tx, _rx) = notification_channel(256);
    let router = build_router_with_notification_sender(cfg(), tx).expect("router built");
    let mut client = TestClient::from_router(router);
    let _ = client.initialize().await;
    let tools = client.list_tools().await;
    assert!(!tools.is_empty());
}

#[tokio::test]
async fn unknown_chat_id_does_not_fire_resource_update() {
    // chat_send to a non-existent chat fails synchronously without
    // registering a turn -- nothing should hit the notifier.
    let (tx, mut rx) = notification_channel(256);
    let router = build_router_with_notification_sender(cfg(), tx).expect("router built");
    let mut client = TestClient::from_router(router);
    client.initialize().await;
    let _ = drain(&mut rx); // clear any startup notifications

    let _ = client
        .call_tool(
            "chat_send",
            serde_json::json!({"chat_id": "nope", "prompt": "hi"}),
        )
        .await;

    let notifs = drain(&mut rx);
    let updates: Vec<&ServerNotification> = notifs
        .iter()
        .filter(|n| matches!(n, ServerNotification::ResourceUpdated { .. }))
        .collect();
    assert!(
        updates.is_empty(),
        "no resource updates should fire for unknown chat; got {updates:?}"
    );
}

#[tokio::test]
#[ignore = "spawns real claude binary"]
async fn live_chat_send_fires_resource_update_for_chats_id() {
    let (tx, mut rx) = notification_channel(256);
    let router = build_router_with_notification_sender(cfg(), tx).expect("router built");
    let mut client = TestClient::from_router(router);
    client.initialize().await;

    let open = client
        .call_tool("chat_open", serde_json::json!({"model": "haiku"}))
        .await;
    let body: serde_json::Value = serde_json::from_str(&open.all_text()).expect("open json");
    let chat_id = body["chat_id"].as_str().unwrap().to_string();

    // Drain any startup / chat_open list-changed notifications.
    let _ = drain(&mut rx);

    let fire = client
        .call_tool(
            "chat_send",
            serde_json::json!({
                "chat_id": chat_id.clone(),
                "prompt": "Reply with the word UPDATED.",
            }),
        )
        .await;
    let turn_id = serde_json::from_str::<serde_json::Value>(&fire.all_text()).unwrap()["turn_id"]
        .as_str()
        .unwrap()
        .to_string();

    // Wait for the turn to settle.
    let _ = client
        .call_tool(
            "turn_wait",
            serde_json::json!({"turn_id": turn_id, "timeout_secs": 30.0}),
        )
        .await;

    let notifs = drain(&mut rx);
    eprintln!("got {} notifications", notifs.len());
    let expected_uri = format!("claude://chats/{chat_id}");
    let saw_update = notifs.iter().any(|n| {
        matches!(
            n,
            ServerNotification::ResourceUpdated { uri } if uri == &expected_uri
        )
    });
    assert!(
        saw_update,
        "expected ResourceUpdated for {expected_uri}; got {notifs:?}"
    );

    let _ = client
        .call_tool("chat_close", serde_json::json!({"chat_id": chat_id}))
        .await;
}

#[tokio::test]
#[ignore = "spawns real claude binary"]
async fn live_chat_open_and_close_fire_list_changed() {
    let (tx, mut rx) = notification_channel(256);
    let router = build_router_with_notification_sender(cfg(), tx).expect("router built");
    let mut client = TestClient::from_router(router);
    client.initialize().await;
    let _ = drain(&mut rx);

    let open = client
        .call_tool("chat_open", serde_json::json!({"model": "haiku"}))
        .await;
    let body: serde_json::Value = serde_json::from_str(&open.all_text()).expect("open json");
    let chat_id = body["chat_id"].as_str().unwrap().to_string();

    let after_open = drain(&mut rx);
    let opens = after_open
        .iter()
        .filter(|n| matches!(n, ServerNotification::ResourcesListChanged))
        .count();
    assert!(
        opens >= 1,
        "expected ResourcesListChanged from chat_open; got {after_open:?}"
    );

    let _ = client
        .call_tool("chat_close", serde_json::json!({"chat_id": chat_id}))
        .await;

    let after_close = drain(&mut rx);
    let closes = after_close
        .iter()
        .filter(|n| matches!(n, ServerNotification::ResourcesListChanged))
        .count();
    assert!(
        closes >= 1,
        "expected ResourcesListChanged from chat_close; got {after_close:?}"
    );
}
