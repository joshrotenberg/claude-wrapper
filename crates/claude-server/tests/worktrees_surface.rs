//! Integration tests for the `worktrees` Cargo feature: read-only
//! tool and resource backed by `claude_wrapper::worktrees`.
//!
//! Tests build small synthetic git repos in tempdirs (`git init` +
//! `git worktree add`) and point the server at them via
//! `ServerConfig::worktrees_root`. One live test (`#[ignore]`)
//! reads worktrees for the actual repo this lives in.

#![cfg(feature = "worktrees")]

use std::path::{Path, PathBuf};
use std::process::Command;

use claude_server::{ServerConfig, build_router, registered_tools};
use tower_mcp::TestClient;

/// Initialize a fresh git repo at `dir` with one commit on `main`.
/// Sets a deterministic identity so the commit succeeds in CI.
fn git_init_with_one_commit(dir: &Path) {
    let must = |args: &[&str]| {
        let out = Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(args)
            .output()
            .expect("git invocation");
        assert!(
            out.status.success(),
            "git {args:?} failed: stderr={}",
            String::from_utf8_lossy(&out.stderr)
        );
    };
    must(&["init", "-b", "main"]);
    must(&["config", "user.email", "test@example.com"]);
    must(&["config", "user.name", "Test"]);
    must(&["config", "commit.gpgsign", "false"]);
    std::fs::write(dir.join("README.md"), "hello\n").expect("write README");
    must(&["add", "README.md"]);
    must(&["commit", "-m", "initial"]);
}

/// Add a worktree at `path` checking out a fresh branch `branch`.
fn git_worktree_add(repo: &Path, path: &Path, branch: &str) {
    let out = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args([
            "worktree",
            "add",
            "-b",
            branch,
            path.to_str().expect("utf8 path"),
        ])
        .output()
        .expect("git worktree add");
    assert!(
        out.status.success(),
        "git worktree add failed: stderr={}",
        String::from_utf8_lossy(&out.stderr)
    );
}

/// Build a fixture repo at `<tempdir>/repo` with one extra worktree
/// at `<tempdir>/wt-feature-x` on branch `feature-x`. Returns the
/// tempdir handle (kept alive for fixture lifetime).
fn fixture_repo() -> (tempfile::TempDir, PathBuf, PathBuf) {
    let tmp = tempfile::tempdir().expect("tempdir");
    let repo = tmp.path().join("repo");
    let extra = tmp.path().join("wt-feature-x");
    std::fs::create_dir_all(&repo).expect("mkdir");
    git_init_with_one_commit(&repo);
    git_worktree_add(&repo, &extra, "feature-x");
    (tmp, repo, extra)
}

fn cfg_for_repo(repo: &Path) -> ServerConfig {
    ServerConfig {
        worktrees_root: Some(repo.to_path_buf()),
        ..Default::default()
    }
}

#[test]
fn registered_tools_includes_worktrees_surface() {
    let (_tmp, repo, _extra) = fixture_repo();
    let tools = registered_tools(cfg_for_repo(&repo)).expect("config built");
    let names: Vec<&str> = tools.iter().map(|t| t.name.as_str()).collect();
    assert!(
        names.contains(&"worktree_list"),
        "missing worktree_list in {names:?}"
    );
}

#[tokio::test]
async fn worktree_list_returns_main_and_extra() {
    let (_tmp, repo, _extra) = fixture_repo();
    let router = build_router(cfg_for_repo(&repo)).expect("router built");
    let mut client = TestClient::from_router(router);
    client.initialize().await;

    let result = client
        .call_tool("worktree_list", serde_json::json!({}))
        .await;
    let v: serde_json::Value = serde_json::from_str(&result.all_text()).expect("json body");
    let wts = v["worktrees"].as_array().expect("worktrees array");
    assert_eq!(wts.len(), 2, "expected main + 1 extra; got {wts:?}");

    // First entry is the main worktree.
    assert_eq!(wts[0]["is_main"].as_bool(), Some(true));
    assert_eq!(wts[0]["branch"].as_str(), Some("main"));
    assert!(!wts[0]["path"].as_str().unwrap_or("").is_empty());

    // Second entry is the feature-x worktree.
    assert_eq!(wts[1]["is_main"].as_bool(), Some(false));
    assert_eq!(wts[1]["branch"].as_str(), Some("feature-x"));
    assert!(wts[1]["head"].is_string());
}

#[tokio::test]
async fn worktree_list_accepts_explicit_repo_path_override() {
    let (_tmp, repo, _extra) = fixture_repo();
    // Server config points at a different (empty) repo, but the
    // explicit repo_path argument wins.
    let other = tempfile::tempdir().expect("tempdir");
    let cfg = ServerConfig {
        worktrees_root: Some(other.path().to_path_buf()),
        ..Default::default()
    };
    let router = build_router(cfg).expect("router built");
    let mut client = TestClient::from_router(router);
    client.initialize().await;

    let result = client
        .call_tool(
            "worktree_list",
            serde_json::json!({"repo_path": repo.to_str().unwrap()}),
        )
        .await;
    let v: serde_json::Value = serde_json::from_str(&result.all_text()).expect("json body");
    assert_eq!(v["worktrees"].as_array().map(Vec::len), Some(2));
}

#[tokio::test]
async fn worktree_list_errors_for_non_git_path() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let router = build_router(cfg_for_repo(tmp.path())).expect("router built");
    let mut client = TestClient::from_router(router);
    client.initialize().await;

    let result = client
        .call_tool("worktree_list", serde_json::json!({}))
        .await;
    assert!(
        result.is_error,
        "expected error for non-git path; got {}",
        result.all_text()
    );
}

#[tokio::test]
async fn worktrees_resource_returns_same_shape() {
    let (_tmp, repo, _extra) = fixture_repo();
    let router = build_router(cfg_for_repo(&repo)).expect("router built");
    let mut client = TestClient::from_router(router);
    client.initialize().await;

    let body = client.read_resource("claude://worktrees").await;
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
    let wts = v["worktrees"].as_array().expect("worktrees array");
    assert_eq!(wts.len(), 2);
    assert_eq!(wts[0]["is_main"].as_bool(), Some(true));
}

// ---------------------------------------------------------------
// Live #[ignore] test -- runs against the actual repo this lives in.
// Run with: cargo test -p claude-server --features worktrees -- --ignored
// ---------------------------------------------------------------

#[tokio::test]
#[ignore = "uses the real git repo this test lives in"]
async fn live_worktree_list_against_this_repo() {
    let cfg = ServerConfig {
        worktrees_root: Some(PathBuf::from(env!("CARGO_MANIFEST_DIR"))),
        ..Default::default()
    };
    let router = build_router(cfg).expect("router built");
    let mut client = TestClient::from_router(router);
    client.initialize().await;

    let result = client
        .call_tool("worktree_list", serde_json::json!({}))
        .await;
    let v: serde_json::Value = serde_json::from_str(&result.all_text()).expect("json body");
    let wts = v["worktrees"].as_array().expect("worktrees array");
    assert!(!wts.is_empty(), "at least the main worktree must exist");
    assert_eq!(wts[0]["is_main"].as_bool(), Some(true));
}
