//! Integration tests for the `jobs` Cargo feature: read-only tools
//! and resources backed by `claude_wrapper::jobs` reading
//! `~/.claude/jobs/<short_id>/state.json` + `timeline.jsonl`.
//!
//! Most tests point a tempdir at the server via
//! `ServerConfig::jobs_root` and seed it with synthetic state.json
//! files. One live test (`#[ignore]`) reads the user's real
//! `~/.claude/jobs/`.

#![cfg(feature = "jobs")]

use std::fs;
use std::io::Write;
use std::path::Path;

use claude_server::{ServerConfig, build_router, registered_tools};
use tower_mcp::TestClient;

fn write_job(root: &Path, short_id: &str, state_json: &str, timeline_lines: &[&str]) {
    let dir = root.join(short_id);
    fs::create_dir_all(&dir).expect("mkdir job dir");
    fs::write(dir.join("state.json"), state_json).expect("write state.json");
    if !timeline_lines.is_empty() {
        let mut f = fs::File::create(dir.join("timeline.jsonl")).expect("create timeline");
        for line in timeline_lines {
            writeln!(f, "{line}").unwrap();
        }
    }
}

fn fixture_root() -> tempfile::TempDir {
    let tmp = tempfile::tempdir().expect("tempdir");
    write_job(
        tmp.path(),
        "aaaaaaaa",
        r#"{"state":"done","detail":"42","intent":"meaning of life",
             "sessionId":"sess-aaa","linkScanPath":"/p/sess-aaa.jsonl",
             "cwd":"/work","createdAt":"2026-05-15T01:00:00Z",
             "updatedAt":"2026-05-15T01:01:00Z","name":"meaning",
             "backend":"daemon","cliVersion":"2.1.143"}"#,
        &[
            r#"{"at":"2026-05-15T01:00:30Z","state":"running","detail":"thinking"}"#,
            r#"{"at":"2026-05-15T01:00:55Z","state":"done","detail":"42","text":"the answer is 42"}"#,
        ],
    );
    write_job(
        tmp.path(),
        "bbbbbbbb",
        r#"{"state":"running","intent":"compute primes","sessionId":"sess-bbb"}"#,
        &[],
    );
    tmp
}

fn cfg_with(root: &Path) -> ServerConfig {
    ServerConfig {
        jobs_root: Some(root.to_path_buf()),
        ..Default::default()
    }
}

#[test]
fn registered_tools_includes_jobs_surface() {
    let tmp = fixture_root();
    let tools = registered_tools(cfg_with(tmp.path())).expect("config built");
    let names: Vec<&str> = tools.iter().map(|t| t.name.as_str()).collect();
    for expected in ["claude_job_list", "claude_job_get"] {
        assert!(
            names.contains(&expected),
            "missing jobs tool {expected} in {names:?}"
        );
    }
}

#[tokio::test]
async fn job_list_returns_synthetic_jobs_sorted() {
    let tmp = fixture_root();
    let router = build_router(cfg_with(tmp.path())).expect("router built");
    let mut client = TestClient::from_router(router);
    client.initialize().await;

    let result = client
        .call_tool("claude_job_list", serde_json::json!({}))
        .await;
    let v: serde_json::Value = serde_json::from_str(&result.all_text()).expect("json");
    let ids: Vec<&str> = v["jobs"]
        .as_array()
        .expect("jobs array")
        .iter()
        .map(|j| j["short_id"].as_str().unwrap_or_default())
        .collect();
    assert_eq!(ids, ["aaaaaaaa", "bbbbbbbb"]);
}

#[tokio::test]
async fn job_list_carries_typed_metadata() {
    let tmp = fixture_root();
    let router = build_router(cfg_with(tmp.path())).expect("router built");
    let mut client = TestClient::from_router(router);
    client.initialize().await;

    let result = client
        .call_tool("claude_job_list", serde_json::json!({}))
        .await;
    let v: serde_json::Value = serde_json::from_str(&result.all_text()).expect("json");
    let job = v["jobs"]
        .as_array()
        .unwrap()
        .iter()
        .find(|j| j["short_id"] == "aaaaaaaa")
        .expect("aaaaaaaa entry");
    assert_eq!(job["state"].as_str().unwrap(), "done");
    assert_eq!(job["intent"].as_str().unwrap(), "meaning of life");
    assert_eq!(job["session_id"].as_str().unwrap(), "sess-aaa");
    assert_eq!(job["session_path"].as_str().unwrap(), "/p/sess-aaa.jsonl");
    assert_eq!(job["cwd"].as_str().unwrap(), "/work");
    assert_eq!(job["name"].as_str().unwrap(), "meaning");
    assert_eq!(job["backend"].as_str().unwrap(), "daemon");
    assert_eq!(job["cli_version"].as_str().unwrap(), "2.1.143");
}

#[tokio::test]
async fn job_get_returns_full_record_with_timeline() {
    let tmp = fixture_root();
    let router = build_router(cfg_with(tmp.path())).expect("router built");
    let mut client = TestClient::from_router(router);
    client.initialize().await;

    let result = client
        .call_tool(
            "claude_job_get",
            serde_json::json!({"short_id": "aaaaaaaa"}),
        )
        .await;
    let v: serde_json::Value = serde_json::from_str(&result.all_text()).expect("json");
    assert_eq!(v["summary"]["short_id"].as_str().unwrap(), "aaaaaaaa");
    assert_eq!(v["summary"]["state"].as_str().unwrap(), "done");

    let timeline = v["timeline"].as_array().expect("timeline array");
    assert_eq!(timeline.len(), 2);
    assert_eq!(timeline[0]["state"].as_str().unwrap(), "running");
    assert_eq!(timeline[1]["state"].as_str().unwrap(), "done");
    assert_eq!(timeline[1]["text"].as_str().unwrap(), "the answer is 42");

    // raw_state preserved for forward-compat drilling.
    assert!(v["raw_state"].is_object());
    assert_eq!(
        v["raw_state"]["intent"].as_str().unwrap(),
        "meaning of life"
    );
}

#[tokio::test]
async fn job_get_unknown_short_id_errors() {
    let tmp = fixture_root();
    let router = build_router(cfg_with(tmp.path())).expect("router built");
    let mut client = TestClient::from_router(router);
    client.initialize().await;

    let result = client
        .call_tool("claude_job_get", serde_json::json!({"short_id": "nope"}))
        .await;
    assert!(
        result.is_error,
        "expected error for unknown short_id; got {}",
        result.all_text()
    );
}

#[tokio::test]
async fn jobs_resource_lists_synthetic_jobs() {
    let tmp = fixture_root();
    let router = build_router(cfg_with(tmp.path())).expect("router built");
    let mut client = TestClient::from_router(router);
    client.initialize().await;

    let body = client.read_resource("claude://jobs").await;
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
    let ids: Vec<&str> = v["jobs"]
        .as_array()
        .unwrap()
        .iter()
        .map(|j| j["short_id"].as_str().unwrap_or_default())
        .collect();
    assert_eq!(ids, ["aaaaaaaa", "bbbbbbbb"]);
}

#[tokio::test]
async fn job_detail_template_returns_full_record() {
    let tmp = fixture_root();
    let router = build_router(cfg_with(tmp.path())).expect("router built");
    let mut client = TestClient::from_router(router);
    client.initialize().await;

    let body = client.read_resource("claude://jobs/aaaaaaaa").await;
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
    assert_eq!(v["summary"]["short_id"].as_str().unwrap(), "aaaaaaaa");
    assert_eq!(v["timeline"].as_array().unwrap().len(), 2);
}

// ---------------------------------------------------------------
// Live #[ignore] test against the user's real ~/.claude/jobs/.
// Run with: cargo test -p claude-server --features jobs -- --ignored
// ---------------------------------------------------------------

#[tokio::test]
#[ignore = "reads the user's real ~/.claude/jobs; may be empty in CI"]
async fn live_job_list_works_against_real_home() {
    let router = build_router(ServerConfig::default()).expect("router built");
    let mut client = TestClient::from_router(router);
    client.initialize().await;

    let result = client
        .call_tool("claude_job_list", serde_json::json!({}))
        .await;
    let v: serde_json::Value = serde_json::from_str(&result.all_text()).expect("json");
    assert!(v["jobs"].is_array(), "jobs array missing");
}
