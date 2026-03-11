//! Integration tests for claude-pool-server MCP tool handlers.
//!
//! Each test builds a `State<InMemoryStore>` backed by the fake-claude binary,
//! registers all tools on an `McpRouter`, and drives them through `TestClient`
//! — the same JSON-RPC path a real MCP client would use.

use std::path::PathBuf;
use std::sync::Arc;

use claude_pool::{
    InMemoryStore, Pool, PoolConfig, ScalingConfig, SkillRegistry, WorkflowRegistry,
};
use claude_pool_server::{State, tools};
use serde_json::json;
use tokio::sync::RwLock;
use tower_mcp::McpRouter;

const FAKE_CLAUDE: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../test-helpers/fake-claude.sh"
);

/// Build a test `State` with the given slot count, backed by the fake claude binary.
async fn test_state(slots: usize) -> Arc<State<InMemoryStore>> {
    let claude = claude_wrapper::Claude::builder()
        .binary(PathBuf::from(FAKE_CLAUDE))
        .build()
        .expect("failed to build Claude client");

    let config = PoolConfig {
        scaling: ScalingConfig {
            min_slots: 1,
            max_slots: 16,
        },
        ..Default::default()
    };

    let pool = Pool::builder_with_store(claude, InMemoryStore::new())
        .slots(slots)
        .config(config)
        .build()
        .await
        .expect("failed to build pool");

    Arc::new(State {
        pool,
        skills: Arc::new(RwLock::new(SkillRegistry::with_builtins())),
        workflows: WorkflowRegistry::with_builtins(),
        skills_dir: PathBuf::from(".claude-pool/skills"),
    })
}

/// Build a `TestClient` with all pool tools registered.
async fn test_client(slots: usize) -> tower_mcp::TestClient {
    let state = test_state(slots).await;
    let tool_list = tools::all_tools(&state);
    let router = McpRouter::new()
        .server_info("test-pool", "0.0.0")
        .tools(tool_list);
    let mut client = tower_mcp::TestClient::from_router(router);
    client.initialize().await;
    client
}

// ── Tests ─────────────────────────────────────────────────────────────────────

/// pool_status returns a JSON object with slot count information.
#[tokio::test]
async fn tool_pool_status_returns_slot_info() {
    let mut client = test_client(2).await;

    let status = client.call_tool_json("pool_status", json!({})).await;

    assert_eq!(
        status["total_slots"].as_u64(),
        Some(2),
        "total_slots should be 2"
    );
    assert!(
        status["idle_slots"].is_number(),
        "idle_slots should be present"
    );
    assert!(
        status["total_spend_microdollars"].is_number(),
        "total_spend_microdollars should be present"
    );
}

/// context_set followed by context_get returns the same value.
#[tokio::test]
async fn tool_context_set_and_get_round_trip() {
    let mut client = test_client(1).await;

    // Set a value.
    let set_result = client
        .call_tool(
            "context_set",
            json!({"key": "project", "value": "claude-wrapper"}),
        )
        .await;
    assert!(!set_result.is_error, "context_set should not error");
    assert_eq!(set_result.first_text(), Some("ok"));

    // Get it back.
    let get_result = client
        .call_tool("context_get", json!({"key": "project"}))
        .await;
    assert!(!get_result.is_error, "context_get should not error");
    assert_eq!(get_result.first_text(), Some("claude-wrapper"));
}

/// context_list shows all keys that were set.
#[tokio::test]
async fn tool_context_list_shows_all_keys() {
    let mut client = test_client(1).await;

    // Set two entries.
    client
        .call_tool("context_set", json!({"key": "alpha", "value": "1"}))
        .await;
    client
        .call_tool("context_set", json!({"key": "beta", "value": "2"}))
        .await;

    let list = client.call_tool_json("context_list", json!({})).await;

    assert_eq!(
        list["alpha"].as_str(),
        Some("1"),
        "alpha key should be present"
    );
    assert_eq!(
        list["beta"].as_str(),
        Some("2"),
        "beta key should be present"
    );
}

/// pool_scale_up increases the slot count.
#[tokio::test]
async fn tool_pool_scale_up_increases_slot_count() {
    let mut client = test_client(2).await;

    let result = client
        .call_tool_json("pool_scale_up", json!({"count": 1}))
        .await;

    assert_eq!(
        result["success"].as_bool(),
        Some(true),
        "scale_up should succeed"
    );
    assert_eq!(
        result["new_slot_count"].as_u64(),
        Some(3),
        "new_slot_count should be 3"
    );
}

/// pool_scale_down reduces the slot count.
#[tokio::test]
async fn tool_pool_scale_down_reduces_slot_count() {
    let mut client = test_client(3).await;

    let result = client
        .call_tool_json("pool_scale_down", json!({"count": 1}))
        .await;

    assert_eq!(
        result["success"].as_bool(),
        Some(true),
        "scale_down should succeed"
    );
    assert_eq!(
        result["new_slot_count"].as_u64(),
        Some(2),
        "new_slot_count should be 2"
    );
}

/// context_get returns an error result (not a panic) when key is missing.
#[tokio::test]
async fn tool_context_get_missing_key_returns_error_result() {
    let mut client = test_client(1).await;

    let result = client
        .call_tool("context_get", json!({"key": "no_such_key"}))
        .await;

    assert!(result.is_error, "missing key should return an error result");
    assert!(
        result.all_text().contains("no_such_key"),
        "error message should include the key name"
    );
}
