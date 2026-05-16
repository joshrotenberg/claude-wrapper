//! End-to-end smoke tests for the registered MCP surface. These run
//! the router in-process via [`tower_mcp::TestClient`] -- no
//! subprocess, no network, no `claude` binary required for tests
//! that don't actually call out.
//!
//! Tests that hit a real `claude` binary are gated behind `#[ignore]`
//! so they don't run by default; run with
//! `cargo test -p claude-server -- --ignored`.

use claude_server::{ServerConfig, build_router, registered_tools};
use tower_mcp::TestClient;

fn cfg() -> ServerConfig {
    ServerConfig::default()
}

#[test]
fn registered_tools_includes_core_l2_surface() {
    let tools = registered_tools(cfg()).expect("config built");
    let names: Vec<&str> = tools.iter().map(|t| t.name.as_str()).collect();

    #[cfg_attr(not(feature = "sync-agent-turns"), allow(unused_mut))]
    let mut expected: Vec<&str> = vec![
        // L2 passthrough (always-on)
        "claude_version",
        "claude_cli_version",
        "claude_query",
        "claude_auth_status",
        "claude_auth_strategy",
        "claude_mcp_list",
        "claude_mcp_get",
        "claude_plugin_list",
        "claude_plugin_validate",
        "claude_marketplace_list",
        "claude_auto_mode_config",
        "claude_auto_mode_defaults",
        "claude_auto_mode_critique",
        "claude_doctor",
        // L2.5 chat (always-on)
        "chat_open",
        "chat_send",
        "chat_list",
        "chat_history",
        "chat_interrupt",
        "chat_budget",
        "chat_close",
        // Turn registry (always-on)
        "turn_get",
        "turn_wait",
        "turn_cancel",
        "turn_list",
        // Slash-command tools (always-on)
        "chat_compact",
        // Observability (always-on)
        "metrics_summary",
    ];
    #[cfg(feature = "sync-agent-turns")]
    expected.extend([
        "claude_query_sync",
        "chat_send_sync",
        "chat_send_stream_sync",
    ]);
    for expected in expected {
        assert!(
            names.contains(&expected),
            "missing tool {expected} in {names:?}"
        );
    }
}

#[cfg(not(feature = "sync-agent-turns"))]
#[test]
fn registered_tools_omits_sync_agent_turn_tools_when_feature_off() {
    let tools = registered_tools(cfg()).expect("config built");
    let names: Vec<&str> = tools.iter().map(|t| t.name.as_str()).collect();
    for forbidden in [
        "claude_query_sync",
        "chat_send_sync",
        "chat_send_stream_sync",
    ] {
        assert!(
            !names.contains(&forbidden),
            "tool {forbidden} should NOT be registered with sync-agent-turns disabled; got {names:?}"
        );
    }
}

#[test]
fn registered_tools_returns_sorted_unique_names() {
    let tools = registered_tools(cfg()).expect("config built");
    let names: Vec<String> = tools.iter().map(|t| t.name.clone()).collect();

    let mut sorted = names.clone();
    sorted.sort();
    assert_eq!(names, sorted, "tools should come back sorted by name");

    let mut deduped = names.clone();
    deduped.dedup();
    assert_eq!(names.len(), deduped.len(), "tool names should be unique");
}

#[tokio::test]
async fn router_tools_list_matches_registered_tools() {
    let registered = registered_tools(cfg()).expect("config built");
    let router = build_router(cfg()).expect("router built");

    let mut client = TestClient::from_router(router);
    client.initialize().await;
    let listed = client.list_tools().await;

    let mut listed_names: Vec<String> = listed
        .iter()
        .map(|t| t["name"].as_str().unwrap_or_default().to_string())
        .collect();
    let mut registered_names: Vec<String> = registered.iter().map(|t| t.name.clone()).collect();
    listed_names.sort();
    registered_names.sort();

    assert_eq!(listed_names, registered_names);
}

#[tokio::test]
async fn claude_auth_strategy_returns_summary_shape() {
    // Doesn't assert a specific strategy -- the test environment may
    // have any of them set. Just shape: the keys exist and `strategy`
    // is one of the documented values.
    let router = build_router(cfg()).expect("router built");
    let mut client = TestClient::from_router(router);
    client.initialize().await;

    let result = client
        .call_tool("claude_auth_strategy", serde_json::json!({}))
        .await;
    let v: serde_json::Value = serde_json::from_str(&result.all_text()).expect("json body");
    let strat = v["strategy"].as_str().expect("strategy string");
    assert!(
        matches!(
            strat,
            "bedrock" | "vertex" | "api_key" | "oauth_token" | "subscription"
        ),
        "unknown strategy {strat:?}"
    );
    for key in [
        "has_anthropic_api_key",
        "has_oauth_token",
        "bedrock_enabled",
        "vertex_enabled",
    ] {
        assert!(v[key].is_boolean(), "missing/non-bool {key}: {v}");
    }
}

#[tokio::test]
async fn config_resource_includes_auth_strategy() {
    let router = build_router(cfg()).expect("router built");
    let mut client = TestClient::from_router(router);
    client.initialize().await;

    let body = client.read_resource("claude://config").await;
    let text = body
        .contents
        .iter()
        .filter_map(|c| serde_json::to_value(c).ok())
        .filter_map(|v| {
            v.get("text")
                .and_then(|t| t.as_str())
                .map(|s| s.to_string())
        })
        .collect::<String>();
    let v: serde_json::Value = serde_json::from_str(&text).expect("config json");
    assert!(v["auth"].is_object(), "auth block missing: {v}");
    assert!(v["auth"]["strategy"].is_string());
}

#[tokio::test]
async fn claude_version_returns_crate_metadata() {
    let router = build_router(cfg()).expect("router built");
    let mut client = TestClient::from_router(router);
    client.initialize().await;

    let result = client
        .call_tool("claude_version", serde_json::json!({}))
        .await;
    let text = result.all_text();
    assert!(text.contains("claude-server"), "result: {text}");
    assert!(text.contains(env!("CARGO_PKG_VERSION")), "result: {text}");
}

#[tokio::test]
async fn resources_list_contains_config_and_tools() {
    let router = build_router(cfg()).expect("router built");
    let mut client = TestClient::from_router(router);
    client.initialize().await;

    let resources = client.list_resources().await;
    let uris: Vec<&str> = resources
        .iter()
        .map(|r| r["uri"].as_str().unwrap_or_default())
        .collect();
    assert!(uris.contains(&"claude://config"), "uris: {uris:?}");
    assert!(uris.contains(&"claude://tools"), "uris: {uris:?}");
    assert!(uris.contains(&"claude://chats"), "uris: {uris:?}");
}

#[tokio::test]
async fn prompts_list_contains_describe_server_and_usage_guide() {
    let router = build_router(cfg()).expect("router built");
    let mut client = TestClient::from_router(router);
    client.initialize().await;

    let prompts = client.list_prompts().await;
    let names: Vec<&str> = prompts
        .iter()
        .map(|p| p["name"].as_str().unwrap_or_default())
        .collect();
    for expected in ["describe_server", "usage_guide"] {
        assert!(
            names.contains(&expected),
            "missing prompt {expected} in {names:?}"
        );
    }
}

#[tokio::test]
async fn usage_guide_prompt_returns_handbook() {
    // The usage_guide prompt is the agent-facing handbook. Sanity-
    // check shape (substantial body, mentions the design rules and
    // the most important tools/resources) so a fresh agent that
    // requests it gets useful onboarding context, not a stub.
    let router = build_router(cfg()).expect("router built");
    let mut client = TestClient::from_router(router);
    client.initialize().await;

    let result = client
        .get_prompt("usage_guide", std::collections::HashMap::new())
        .await;
    let text: String = result
        .messages
        .iter()
        .filter_map(|m| {
            serde_json::to_value(&m.content).ok().and_then(|v| {
                v.get("text")
                    .and_then(|t| t.as_str())
                    .map(|s| s.to_string())
            })
        })
        .collect();
    assert!(
        text.len() > 1000,
        "usage_guide should be substantial; got {} bytes",
        text.len()
    );
    for needle in [
        "async-by-default",
        "chat_send",
        "claude_query",
        "turn_wait",
        "claude://chats",
        "max_cost_usd",
    ] {
        assert!(
            text.to_lowercase().contains(&needle.to_lowercase()),
            "usage_guide should mention {needle}"
        );
    }
}

// ---------------------------------------------------------------
// Live #[ignore] tests -- exercise the real `claude` binary on PATH.
// Run with: cargo test -p claude-server -- --ignored
// ---------------------------------------------------------------

#[tokio::test]
#[ignore = "spawns real claude binary; run with --ignored"]
async fn live_claude_cli_version_returns_three_numbers() {
    let router = build_router(cfg()).expect("router built");
    let mut client = TestClient::from_router(router);
    client.initialize().await;

    let result = client
        .call_tool("claude_cli_version", serde_json::json!({}))
        .await;
    let text = result.all_text();
    let v: serde_json::Value = serde_json::from_str(&text).expect("json body");
    assert!(v["major"].is_u64(), "major: {text}");
    assert!(v["minor"].is_u64(), "minor: {text}");
    assert!(v["patch"].is_u64(), "patch: {text}");
}

#[cfg(feature = "sync-agent-turns")]
#[tokio::test]
#[ignore = "spawns real claude binary; run with --ignored"]
async fn live_claude_query_sync_simple_prompt() {
    let router = build_router(cfg()).expect("router built");
    let mut client = TestClient::from_router(router);
    client.initialize().await;

    let result = client
        .call_tool(
            "claude_query_sync",
            serde_json::json!({
                "prompt": "Reply with exactly the word OK and nothing else.",
                "model": "haiku",
            }),
        )
        .await;
    let text = result.all_text();
    let v: serde_json::Value = serde_json::from_str(&text).expect("json body");
    assert!(v["result"].is_string(), "result missing: {text}");
    assert!(v["session_id"].is_string(), "session_id missing: {text}");
    let r = v["result"].as_str().unwrap_or_default();
    assert!(r.contains("OK"), "expected OK in result, got {r:?}");
}

#[tokio::test]
#[ignore = "spawns real claude binary; run with --ignored"]
async fn live_claude_mcp_list_returns_output() {
    let router = build_router(cfg()).expect("router built");
    let mut client = TestClient::from_router(router);
    client.initialize().await;

    let result = client
        .call_tool("claude_mcp_list", serde_json::json!({}))
        .await;
    let text = result.all_text();
    let v: serde_json::Value = serde_json::from_str(&text).expect("json body");
    assert!(v["exit_code"].is_number(), "exit_code missing: {text}");
}
