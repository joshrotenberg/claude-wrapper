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

    for expected in [
        // L2 passthrough
        "claude_version",
        "claude_cli_version",
        "claude_query",
        "claude_agents",
        "claude_auth_status",
        "claude_mcp_list",
        "claude_mcp_get",
        "claude_plugin_list",
        "claude_plugin_validate",
        "claude_marketplace_list",
        "claude_auto_mode_config",
        "claude_auto_mode_defaults",
        "claude_doctor",
        // L2.5 chat
        "chat_open",
        "chat_send",
        "chat_send_stream",
        "chat_list",
        "chat_history",
        "chat_interrupt",
        "chat_budget",
        "chat_close",
    ] {
        assert!(
            names.contains(&expected),
            "missing tool {expected} in {names:?}"
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
async fn prompts_list_contains_describe_server() {
    let router = build_router(cfg()).expect("router built");
    let mut client = TestClient::from_router(router);
    client.initialize().await;

    let prompts = client.list_prompts().await;
    let names: Vec<&str> = prompts
        .iter()
        .map(|p| p["name"].as_str().unwrap_or_default())
        .collect();
    assert!(names.contains(&"describe_server"), "names: {names:?}");
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

#[tokio::test]
#[ignore = "spawns real claude binary; run with --ignored"]
async fn live_claude_query_simple_prompt() {
    let router = build_router(cfg()).expect("router built");
    let mut client = TestClient::from_router(router);
    client.initialize().await;

    let result = client
        .call_tool(
            "claude_query",
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
