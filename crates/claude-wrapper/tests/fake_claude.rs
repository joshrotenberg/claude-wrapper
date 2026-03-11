//! Tests that use the fake-claude binary instead of the real Claude CLI.
//!
//! These tests do not require a real `claude` binary or authentication.

use std::path::PathBuf;

use claude_wrapper::{Claude, ClaudeCommand, QueryCommand};

const FAKE_CLAUDE: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../test-helpers/fake-claude.sh"
);

fn fake_binary() -> PathBuf {
    PathBuf::from(FAKE_CLAUDE)
}

/// Build a Claude client backed by the fake binary with optional env overrides.
fn claude_with_env(pairs: &[(&str, &str)]) -> Claude {
    let mut builder = Claude::builder().binary(fake_binary());
    for (k, v) in pairs {
        builder = builder.env(*k, *v);
    }
    builder.build().expect("failed to build Claude client")
}

/// Verify that the fake binary executes and returns expected plain-text output.
#[tokio::test]
async fn fake_claude_basic_execution() {
    let claude = claude_with_env(&[("FAKE_CLAUDE_OUTPUT", "hello from fake")]);

    let output = claude_wrapper::VersionCommand::new()
        .execute(&claude)
        .await
        .expect("fake claude should succeed");

    assert!(output.success);
    assert!(output.stdout.contains("hello from fake"));
}

/// Verify that stream-json NDJSON output is parsed correctly by execute_json.
#[tokio::test]
async fn fake_claude_stream_json_output() {
    let claude = claude_with_env(&[("FAKE_CLAUDE_OUTPUT", "42")]);

    let result = QueryCommand::new("What is 2+2?")
        .no_session_persistence()
        .execute_json(&claude)
        .await
        .expect("fake claude json query should succeed");

    assert_eq!(result.result, "42");
    assert_eq!(result.session_id, "fake-session-id");
}

/// Verify that a non-zero exit code is surfaced as an error.
#[tokio::test]
async fn fake_claude_non_zero_exit_is_error() {
    let claude = claude_with_env(&[
        ("FAKE_CLAUDE_EXIT_CODE", "1"),
        ("FAKE_CLAUDE_ERROR_MSG", "simulated failure"),
    ]);

    let result = claude_wrapper::VersionCommand::new().execute(&claude).await;

    assert!(result.is_err(), "non-zero exit should be an error");
}
