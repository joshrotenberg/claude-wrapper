//! Integration tests that require a real `claude` binary in PATH.
//!
//! These tests are ignored by default. Run with:
//! ```sh
//! cargo test --test integration -- --ignored
//! ```

#![cfg(feature = "async")]

use claude_wrapper::{Claude, ClaudeCommand, QueryCommand, VersionCommand};

fn claude_client() -> Claude {
    Claude::builder()
        .build()
        .expect("claude binary not found in PATH")
}

#[tokio::test]
#[ignore = "requires claude binary"]
async fn test_version() {
    let claude = claude_client();
    let output = VersionCommand::new().execute(&claude).await.unwrap();
    assert!(output.success);
    assert!(!output.stdout.is_empty());
}

#[tokio::test]
#[ignore = "requires claude binary and auth"]
async fn test_auth_status() {
    let claude = claude_client();
    let status = claude_wrapper::AuthStatusCommand::new()
        .execute_json(&claude)
        .await
        .unwrap();
    assert!(status.logged_in);
}

#[tokio::test]
#[ignore = "requires claude binary and auth"]
async fn test_simple_query() {
    let claude = claude_client();
    let output = QueryCommand::new("What is 2+2? Reply with just the number.")
        .model("haiku")
        .max_turns(1)
        .no_session_persistence()
        .permission_mode(claude_wrapper::PermissionMode::Plan)
        .execute(&claude)
        .await
        .unwrap();

    assert!(output.success);
    assert!(output.stdout.contains('4'));
}

#[tokio::test]
#[ignore = "requires claude binary and auth"]
async fn test_query_json_output() {
    let claude = claude_client();
    let result = QueryCommand::new("What is 2+2? Reply with just the number.")
        .model("haiku")
        .max_turns(1)
        .no_session_persistence()
        .permission_mode(claude_wrapper::PermissionMode::Plan)
        .execute_json(&claude)
        .await
        .unwrap();

    assert!(!result.result.is_empty());
    assert!(!result.session_id.is_empty());
}

#[tokio::test]
#[ignore = "requires claude binary"]
async fn test_mcp_list() {
    let claude = claude_client();
    let output = claude_wrapper::McpListCommand::new()
        .execute(&claude)
        .await
        .unwrap();
    assert!(output.success);
}

#[tokio::test]
#[ignore = "requires claude binary and auth"]
async fn test_duplex_send_and_close() {
    use claude_wrapper::duplex::{DuplexOptions, DuplexSession};

    let claude = claude_client();
    let session = DuplexSession::spawn(&claude, DuplexOptions::default().model("haiku"))
        .await
        .expect("spawn duplex session");

    let turn = session
        .send("What is 2+2? Reply with just the number.")
        .await
        .expect("send turn");

    assert!(
        turn.session_id().is_some(),
        "result should carry a session_id"
    );
    let text = turn.result_text().expect("result text present");
    assert!(text.contains('4'), "expected '4' in {text:?}");

    session.close().await.expect("close session");
}

#[tokio::test]
#[ignore = "requires claude binary and auth"]
async fn test_duplex_two_turns_share_session() {
    use claude_wrapper::duplex::{DuplexOptions, DuplexSession};

    let claude = claude_client();
    let session = DuplexSession::spawn(&claude, DuplexOptions::default().model("haiku"))
        .await
        .expect("spawn duplex session");

    let first = session
        .send("Remember the number 7.")
        .await
        .expect("first turn");
    let second = session
        .send("What number did I ask you to remember? Reply with just the number.")
        .await
        .expect("second turn");

    let first_id = first.session_id().expect("first session_id");
    let second_id = second.session_id().expect("second session_id");
    assert_eq!(
        first_id, second_id,
        "session id should persist across turns"
    );

    let text = second.result_text().unwrap_or("");
    assert!(text.contains('7'), "expected '7' in {text:?}");

    session.close().await.expect("close session");
}

#[tokio::test]
#[ignore = "requires claude binary"]
async fn test_mcp_config_builder() {
    use claude_wrapper::McpConfigBuilder;

    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join(".mcp.json");

    let written_path = McpConfigBuilder::new()
        .http_server("test-hub", "http://127.0.0.1:9090")
        .stdio_server("test-tool", "echo", ["hello"])
        .write_to(&path)
        .unwrap();

    assert_eq!(written_path, path);

    let contents = std::fs::read_to_string(&path).unwrap();
    let parsed: serde_json::Value = serde_json::from_str(&contents).unwrap();

    assert!(
        parsed["mcpServers"]["test-hub"]["url"]
            .as_str()
            .unwrap()
            .contains("9090")
    );
    assert_eq!(
        parsed["mcpServers"]["test-tool"]["command"]
            .as_str()
            .unwrap(),
        "echo"
    );
}
