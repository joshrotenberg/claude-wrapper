//! Integration tests for the `history` Cargo feature: tools and
//! resources backed by `claude_wrapper::history` reading
//! `~/.claude/projects/<slug>/<session_id>.jsonl`.
//!
//! Most tests point a tempdir at the server via
//! `ServerConfig::history_root` and seed it with synthetic JSONL.
//! One live test (`#[ignore]`) reads the user's real
//! `~/.claude/projects/`.

#![cfg(feature = "history")]

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use claude_server::{ServerConfig, build_router, registered_tools};
use tower_mcp::TestClient;

fn write_session(dir: &Path, session_id: &str, lines: &[&str]) -> PathBuf {
    let path = dir.join(format!("{session_id}.jsonl"));
    let mut f = fs::File::create(&path).expect("create jsonl");
    for line in lines {
        writeln!(f, "{line}").unwrap();
    }
    path
}

fn fixture_root() -> tempfile::TempDir {
    let tmp = tempfile::tempdir().expect("tempdir");
    let a = tmp.path().join("-Users-josh-Code-projA");
    fs::create_dir_all(&a).unwrap();
    write_session(
        &a,
        "session-aaa",
        &[
            r#"{"type":"user","uuid":"u1","timestamp":"2026-01-01T00:00:00Z","cwd":"/Users/josh/Code/projA","gitBranch":"main","message":{"role":"user","content":"hello"}}"#,
            r#"{"type":"assistant","uuid":"a1","timestamp":"2026-01-01T00:00:01Z","message":{"role":"assistant","content":"hi"}}"#,
            r#"{"type":"ai-title","title":"hello world"}"#,
        ],
    );
    write_session(
        &a,
        "session-bbb",
        &[
            r#"{"type":"user","uuid":"u2","timestamp":"2026-01-02T00:00:00Z","message":{"role":"user","content":"second"}}"#,
        ],
    );
    let b = tmp.path().join("-private-tmp-projB");
    fs::create_dir_all(&b).unwrap();
    write_session(
        &b,
        "session-ccc",
        &[
            r#"{"type":"user","uuid":"u3","timestamp":"2026-02-01T00:00:00Z","message":{"role":"user","content":"x"}}"#,
            r#"{"type":"assistant","uuid":"a3","timestamp":"2026-02-01T00:00:01Z","message":{"role":"assistant","content":"y"}}"#,
        ],
    );
    tmp
}

fn cfg_with(root: &Path) -> ServerConfig {
    ServerConfig {
        history_root: Some(root.to_path_buf()),
        ..Default::default()
    }
}

#[test]
fn registered_tools_includes_history_surface() {
    let tmp = fixture_root();
    let tools = registered_tools(cfg_with(tmp.path())).expect("config built");
    let names: Vec<&str> = tools.iter().map(|t| t.name.as_str()).collect();
    for expected in [
        "claude_project_list",
        "claude_session_list",
        "claude_session_get",
    ] {
        assert!(
            names.contains(&expected),
            "missing history tool {expected} in {names:?}"
        );
    }
}

#[tokio::test]
async fn project_list_returns_synthetic_projects() {
    let tmp = fixture_root();
    let router = build_router(cfg_with(tmp.path())).expect("router built");
    let mut client = TestClient::from_router(router);
    client.initialize().await;

    let result = client
        .call_tool("claude_project_list", serde_json::json!({}))
        .await;
    let v: serde_json::Value = serde_json::from_str(&result.all_text()).expect("json");
    let slugs: Vec<&str> = v["projects"]
        .as_array()
        .expect("projects array")
        .iter()
        .map(|p| p["slug"].as_str().unwrap_or_default())
        .collect();
    assert_eq!(slugs, ["-Users-josh-Code-projA", "-private-tmp-projB"]);
    let counts: Vec<u64> = v["projects"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| p["session_count"].as_u64().unwrap_or(0))
        .collect();
    assert_eq!(counts, [2, 1]);
}

#[tokio::test]
async fn session_list_with_filter_returns_only_one_project() {
    let tmp = fixture_root();
    let router = build_router(cfg_with(tmp.path())).expect("router built");
    let mut client = TestClient::from_router(router);
    client.initialize().await;

    let result = client
        .call_tool(
            "claude_session_list",
            serde_json::json!({"project_slug": "-private-tmp-projB"}),
        )
        .await;
    let v: serde_json::Value = serde_json::from_str(&result.all_text()).expect("json");
    let ids: Vec<&str> = v["sessions"]
        .as_array()
        .expect("sessions")
        .iter()
        .map(|s| s["session_id"].as_str().unwrap_or_default())
        .collect();
    assert_eq!(ids, ["session-ccc"]);
}

#[tokio::test]
async fn session_list_no_filter_returns_union() {
    let tmp = fixture_root();
    let router = build_router(cfg_with(tmp.path())).expect("router built");
    let mut client = TestClient::from_router(router);
    client.initialize().await;

    let result = client
        .call_tool("claude_session_list", serde_json::json!({}))
        .await;
    let v: serde_json::Value = serde_json::from_str(&result.all_text()).expect("json");
    let ids: Vec<&str> = v["sessions"]
        .as_array()
        .expect("sessions")
        .iter()
        .map(|s| s["session_id"].as_str().unwrap_or_default())
        .collect();
    assert_eq!(ids.len(), 3);
    for id in ["session-aaa", "session-bbb", "session-ccc"] {
        assert!(ids.contains(&id), "missing {id} in {ids:?}");
    }
}

#[tokio::test]
async fn session_get_returns_typed_entries_in_order() {
    let tmp = fixture_root();
    let router = build_router(cfg_with(tmp.path())).expect("router built");
    let mut client = TestClient::from_router(router);
    client.initialize().await;

    let result = client
        .call_tool(
            "claude_session_get",
            serde_json::json!({"session_id": "session-aaa"}),
        )
        .await;
    let v: serde_json::Value = serde_json::from_str(&result.all_text()).expect("json");
    assert_eq!(v["session_id"].as_str().unwrap(), "session-aaa");
    let entries = v["entries"].as_array().expect("entries");
    assert_eq!(entries.len(), 3, "expected 3 entries: {entries:?}");
    assert_eq!(entries[0]["kind"].as_str().unwrap(), "user");
    assert_eq!(entries[1]["kind"].as_str().unwrap(), "assistant");
    assert_eq!(entries[2]["kind"].as_str().unwrap(), "other");
    assert_eq!(entries[2]["type_tag"].as_str().unwrap(), "ai-title");
}

#[tokio::test]
async fn session_get_unknown_id_errors() {
    let tmp = fixture_root();
    let router = build_router(cfg_with(tmp.path())).expect("router built");
    let mut client = TestClient::from_router(router);
    client.initialize().await;

    let result = client
        .call_tool(
            "claude_session_get",
            serde_json::json!({"session_id": "session-nope"}),
        )
        .await;
    assert!(
        result.is_error,
        "expected is_error=true for unknown id; got {}",
        result.all_text()
    );
}

#[tokio::test]
async fn projects_resource_lists_synthetic_projects() {
    let tmp = fixture_root();
    let router = build_router(cfg_with(tmp.path())).expect("router built");
    let mut client = TestClient::from_router(router);
    client.initialize().await;

    let body = client.read_resource("claude://projects").await;
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
    let slugs: Vec<&str> = v["projects"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| p["slug"].as_str().unwrap_or_default())
        .collect();
    assert!(slugs.contains(&"-Users-josh-Code-projA"));
    assert!(slugs.contains(&"-private-tmp-projB"));
}

#[tokio::test]
async fn project_detail_template_returns_session_list() {
    let tmp = fixture_root();
    let router = build_router(cfg_with(tmp.path())).expect("router built");
    let mut client = TestClient::from_router(router);
    client.initialize().await;

    let body = client
        .read_resource("claude://projects/-Users-josh-Code-projA")
        .await;
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
    assert_eq!(
        v["project_slug"].as_str().unwrap(),
        "-Users-josh-Code-projA"
    );
    let ids: Vec<&str> = v["sessions"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| s["session_id"].as_str().unwrap_or_default())
        .collect();
    assert_eq!(ids.len(), 2);
}

#[tokio::test]
async fn session_detail_template_returns_entries() {
    let tmp = fixture_root();
    let router = build_router(cfg_with(tmp.path())).expect("router built");
    let mut client = TestClient::from_router(router);
    client.initialize().await;

    let body = client.read_resource("claude://sessions/session-ccc").await;
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
    assert_eq!(v["session_id"].as_str().unwrap(), "session-ccc");
    let entries = v["entries"].as_array().expect("entries");
    assert!(entries.len() >= 2);
}

// ---------------------------------------------------------------
// Live #[ignore] tests -- read the user's real ~/.claude/projects/.
// Run with: cargo test -p claude-server --features history -- --ignored
// ---------------------------------------------------------------

#[tokio::test]
#[ignore = "reads the user's real ~/.claude/projects; may be empty in CI"]
async fn live_project_list_works_against_real_home() {
    let router = build_router(ServerConfig::default()).expect("router built");
    let mut client = TestClient::from_router(router);
    client.initialize().await;

    let result = client
        .call_tool("claude_project_list", serde_json::json!({}))
        .await;
    let v: serde_json::Value = serde_json::from_str(&result.all_text()).expect("json");
    assert!(v["projects"].is_array(), "projects array missing");
}
