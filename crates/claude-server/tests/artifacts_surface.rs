//! Integration tests for the `artifacts` Cargo feature: read-only
//! tools and resources backed by `claude_wrapper::artifacts` reading
//! `~/.claude/agents/<file_stem>.md`.
//!
//! Most tests point a tempdir at the server via
//! `ServerConfig::agents_root` and seed it with synthetic markdown.
//! One live test (`#[ignore]`) reads the user's real
//! `~/.claude/agents/`.

#![cfg(feature = "artifacts")]

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use claude_server::{ServerConfig, build_router, registered_tools};
use tower_mcp::TestClient;

fn write_agent(dir: &Path, file_stem: &str, contents: &str) -> PathBuf {
    let path = dir.join(format!("{file_stem}.md"));
    let mut f = fs::File::create(&path).expect("create md");
    f.write_all(contents.as_bytes()).expect("write md");
    path
}

fn fixture_root() -> tempfile::TempDir {
    let tmp = tempfile::tempdir().expect("tempdir");
    write_agent(
        tmp.path(),
        "rust-qa",
        "---\nname: rust-qa\ndescription: Rust quality gate\ntools: Read, Grep, Bash\nmodel: sonnet\n---\n\nYou are a Rust quality gate.\n",
    );
    write_agent(
        tmp.path(),
        "minimal",
        "---\nname: minimal\ndescription: Minimal agent\n---\nBody here.\n",
    );
    write_agent(
        tmp.path(),
        "weird",
        "---\nname: weird\ndescription: has extras\ncustom_key: custom_value\n---\nbody\n",
    );
    tmp
}

fn cfg_with(root: &Path) -> ServerConfig {
    ServerConfig {
        agents_root: Some(root.to_path_buf()),
        ..Default::default()
    }
}

#[test]
fn registered_tools_includes_artifacts_surface() {
    let tmp = fixture_root();
    let tools = registered_tools(cfg_with(tmp.path())).expect("config built");
    let names: Vec<&str> = tools.iter().map(|t| t.name.as_str()).collect();
    for expected in ["agent_list", "agent_get"] {
        assert!(
            names.contains(&expected),
            "missing artifacts tool {expected} in {names:?}"
        );
    }
}

#[tokio::test]
async fn agent_list_returns_synthetic_agents_sorted() {
    let tmp = fixture_root();
    let router = build_router(cfg_with(tmp.path())).expect("router built");
    let mut client = TestClient::from_router(router);
    client.initialize().await;

    let result = client.call_tool("agent_list", serde_json::json!({})).await;
    let v: serde_json::Value = serde_json::from_str(&result.all_text()).expect("json");
    let stems: Vec<&str> = v["agents"]
        .as_array()
        .expect("agents array")
        .iter()
        .map(|a| a["file_stem"].as_str().unwrap_or_default())
        .collect();
    assert_eq!(stems, ["minimal", "rust-qa", "weird"]);
}

#[tokio::test]
async fn agent_list_carries_typed_metadata() {
    let tmp = fixture_root();
    let router = build_router(cfg_with(tmp.path())).expect("router built");
    let mut client = TestClient::from_router(router);
    client.initialize().await;

    let result = client.call_tool("agent_list", serde_json::json!({})).await;
    let v: serde_json::Value = serde_json::from_str(&result.all_text()).expect("json");
    let agents = v["agents"].as_array().expect("agents");
    let rust_qa = agents
        .iter()
        .find(|a| a["file_stem"] == "rust-qa")
        .expect("rust-qa entry");
    assert_eq!(rust_qa["name"].as_str().unwrap(), "rust-qa");
    assert_eq!(
        rust_qa["description"].as_str().unwrap(),
        "Rust quality gate"
    );
    let tools: Vec<&str> = rust_qa["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t.as_str().unwrap_or_default())
        .collect();
    assert_eq!(tools, ["Read", "Grep", "Bash"]);
    assert_eq!(rust_qa["model"].as_str().unwrap(), "sonnet");
    assert!(rust_qa["size_bytes"].as_u64().unwrap() > 0);
}

#[tokio::test]
async fn agent_get_returns_full_record_with_body_and_extras() {
    let tmp = fixture_root();
    let router = build_router(cfg_with(tmp.path())).expect("router built");
    let mut client = TestClient::from_router(router);
    client.initialize().await;

    let result = client
        .call_tool("agent_get", serde_json::json!({"file_stem": "weird"}))
        .await;
    let v: serde_json::Value = serde_json::from_str(&result.all_text()).expect("json");
    assert_eq!(v["file_stem"].as_str().unwrap(), "weird");
    assert_eq!(v["name"].as_str().unwrap(), "weird");
    assert_eq!(v["body"].as_str().unwrap(), "body");
    assert_eq!(v["extra"]["custom_key"].as_str().unwrap(), "custom_value");
}

#[tokio::test]
async fn agent_get_unknown_stem_errors() {
    let tmp = fixture_root();
    let router = build_router(cfg_with(tmp.path())).expect("router built");
    let mut client = TestClient::from_router(router);
    client.initialize().await;

    let result = client
        .call_tool("agent_get", serde_json::json!({"file_stem": "nope"}))
        .await;
    assert!(
        result.is_error,
        "expected is_error=true for unknown stem; got {}",
        result.all_text()
    );
}

#[tokio::test]
async fn agents_resource_lists_synthetic_agents() {
    let tmp = fixture_root();
    let router = build_router(cfg_with(tmp.path())).expect("router built");
    let mut client = TestClient::from_router(router);
    client.initialize().await;

    let body = client.read_resource("claude://agents").await;
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
    let v: serde_json::Value = serde_json::from_str(&text).expect("json body");
    let stems: Vec<&str> = v["agents"]
        .as_array()
        .unwrap()
        .iter()
        .map(|a| a["file_stem"].as_str().unwrap_or_default())
        .collect();
    assert!(stems.contains(&"rust-qa"));
    assert!(stems.contains(&"minimal"));
}

#[tokio::test]
async fn agent_detail_template_returns_full_record() {
    let tmp = fixture_root();
    let router = build_router(cfg_with(tmp.path())).expect("router built");
    let mut client = TestClient::from_router(router);
    client.initialize().await;

    let body = client.read_resource("claude://agents/rust-qa").await;
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
    let v: serde_json::Value = serde_json::from_str(&text).expect("json body");
    assert_eq!(v["file_stem"].as_str().unwrap(), "rust-qa");
    assert_eq!(v["body"].as_str().unwrap(), "You are a Rust quality gate.");
}

// ---------------------------------------------------------------
// Live #[ignore] test -- reads the user's real ~/.claude/agents/.
// Run with: cargo test -p claude-server --features artifacts -- --ignored
// ---------------------------------------------------------------

#[tokio::test]
#[ignore = "reads the user's real ~/.claude/agents; may be empty in CI"]
async fn live_agent_list_works_against_real_home() {
    let router = build_router(ServerConfig::default()).expect("router built");
    let mut client = TestClient::from_router(router);
    client.initialize().await;

    let result = client.call_tool("agent_list", serde_json::json!({})).await;
    let v: serde_json::Value = serde_json::from_str(&result.all_text()).expect("json");
    assert!(v["agents"].is_array(), "agents array missing");
}
