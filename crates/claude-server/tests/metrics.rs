//! Metrics counter + tool/resource surface tests.
//!
//! Counter logic exercised directly against [`crate::turns::Metrics`]
//! via the registry-test path; tool/resource shape exercised through
//! [`tower_mcp::TestClient`].

use claude_server::{ServerConfig, build_router};
use tower_mcp::TestClient;

fn cfg() -> ServerConfig {
    ServerConfig::default()
}

#[tokio::test]
async fn metrics_summary_starts_zero() {
    let router = build_router(cfg()).expect("router built");
    let mut client = TestClient::from_router(router);
    client.initialize().await;

    let r = client
        .call_tool("metrics_summary", serde_json::json!({}))
        .await;
    let text = r.all_text();
    let v: serde_json::Value = serde_json::from_str(&text).expect("json");
    assert_eq!(v["turns_fired"], serde_json::json!(0));
    assert_eq!(v["turns_done"], serde_json::json!(0));
    assert_eq!(v["turns_failed"], serde_json::json!(0));
    assert_eq!(v["turns_cancelled"], serde_json::json!(0));
    assert_eq!(v["in_flight"], serde_json::json!(0));
    assert_eq!(v["total_cost_usd"], serde_json::json!(0.0));
}

#[tokio::test]
async fn metrics_resource_starts_zero() {
    let router = build_router(cfg()).expect("router built");
    let mut client = TestClient::from_router(router);
    client.initialize().await;

    let r = client.read_resource("claude://metrics").await;
    let text = r
        .contents
        .first()
        .and_then(|c| c.text.clone())
        .unwrap_or_default();
    let v: serde_json::Value = serde_json::from_str(&text).expect("json");
    assert_eq!(v["turns_fired"], serde_json::json!(0));
    assert_eq!(v["in_flight"], serde_json::json!(0));
}

#[tokio::test]
#[ignore = "spawns real claude binary"]
async fn live_metrics_increment_through_full_turn() {
    let router = build_router(cfg()).expect("router built");
    let mut client = TestClient::from_router(router);
    client.initialize().await;

    // Fire one async claude_query and let it settle.
    let fire = client
        .call_tool(
            "claude_query",
            serde_json::json!({
                "prompt": "Reply with the word METRIC.",
                "model": "haiku",
            }),
        )
        .await;
    let turn_id = serde_json::from_str::<serde_json::Value>(&fire.all_text()).unwrap()["turn_id"]
        .as_str()
        .unwrap()
        .to_string();

    // While running, in_flight should be 1.
    let mid = client
        .call_tool("metrics_summary", serde_json::json!({}))
        .await;
    let mv: serde_json::Value = serde_json::from_str(&mid.all_text()).expect("json");
    eprintln!("mid: {mv}");
    assert_eq!(mv["turns_fired"], serde_json::json!(1));
    // in_flight could already be 0 if the turn was very fast; both ok
    let in_flight = mv["in_flight"].as_i64().unwrap_or(-1);
    assert!(
        (0..=1).contains(&in_flight),
        "in_flight should be 0 or 1 mid-turn, got {in_flight}"
    );

    let _ = client
        .call_tool(
            "turn_wait",
            serde_json::json!({"turn_id": turn_id, "timeout_secs": 30.0}),
        )
        .await;

    // After settle: in_flight back to 0, turns_done == 1, cost > 0.
    let post = client
        .call_tool("metrics_summary", serde_json::json!({}))
        .await;
    let pv: serde_json::Value = serde_json::from_str(&post.all_text()).expect("json");
    eprintln!("post: {pv}");
    assert_eq!(pv["turns_fired"], serde_json::json!(1));
    assert_eq!(pv["turns_done"], serde_json::json!(1));
    assert_eq!(pv["in_flight"], serde_json::json!(0));
    let cost = pv["total_cost_usd"].as_f64().unwrap_or(0.0);
    assert!(cost > 0.0, "expected nonzero cumulative cost, got {cost}");
}
