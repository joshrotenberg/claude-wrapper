//! Tests that use the fake-claude binary instead of the real Claude CLI.
//!
//! These tests do not require a real `claude` binary or authentication.

use std::path::PathBuf;

use claude_wrapper::{Claude, ClaudeCommand, OutputFormat, QueryCommand};

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

/// Verify that stream_query fires the handler for each NDJSON event type.
#[tokio::test]
async fn streaming_ndjson_parsed_correctly() {
    use claude_wrapper::streaming::{StreamEvent, stream_query};

    let claude = claude_with_env(&[("FAKE_CLAUDE_OUTPUT", "streamed response")]);
    let cmd = QueryCommand::new("test prompt")
        .output_format(OutputFormat::StreamJson)
        .no_session_persistence();

    let mut events: Vec<StreamEvent> = Vec::new();
    stream_query(&claude, &cmd, |event| events.push(event))
        .await
        .expect("streaming should succeed");

    // Fake binary emits exactly three lines: system, assistant, result.
    assert_eq!(events.len(), 3, "expected 3 events, got {}", events.len());
    assert_eq!(events[0].event_type(), Some("system"));
    assert_eq!(events[1].event_type(), Some("assistant"));
    assert_eq!(events[2].event_type(), Some("result"));
}

/// Verify that the result event contains the correct session_id, result text,
/// and cost fields.
#[tokio::test]
async fn streaming_extracts_cost_and_session() {
    use claude_wrapper::streaming::{StreamEvent, stream_query};

    let claude = claude_with_env(&[
        ("FAKE_CLAUDE_OUTPUT", "test output"),
        ("FAKE_CLAUDE_SESSION_ID", "test-session-123"),
    ]);
    let cmd = QueryCommand::new("test prompt")
        .output_format(OutputFormat::StreamJson)
        .no_session_persistence();

    let mut result_event: Option<StreamEvent> = None;
    stream_query(&claude, &cmd, |event| {
        if event.is_result() {
            result_event = Some(event);
        }
    })
    .await
    .expect("streaming should succeed");

    let result = result_event.expect("should have received a result event");
    assert_eq!(result.session_id(), Some("test-session-123"));
    assert_eq!(result.result_text(), Some("test output"));
    assert_eq!(result.cost_usd(), Some(0.0));
}

/// Verify that a short timeout fires before the fake binary finishes sleeping.
#[tokio::test]
async fn fake_claude_timeout_fires() {
    let claude = Claude::builder()
        .binary(fake_binary())
        .env("FAKE_CLAUDE_DELAY", "5")
        .timeout_secs(1)
        .build()
        .expect("failed to build client");

    let result = claude_wrapper::VersionCommand::new().execute(&claude).await;

    assert!(result.is_err(), "expected a timeout error");
    let err_str = result.unwrap_err().to_string().to_lowercase();
    assert!(
        err_str.contains("timeout") || err_str.contains("timed out"),
        "expected timeout error, got: {err_str}"
    );
}

/// Verify that pointing at a nonexistent binary surfaces an error at execution
/// time (not at build time).
#[tokio::test]
async fn binary_not_found_returns_error() {
    let claude = Claude::builder()
        .binary("/nonexistent/binary/fake-claude")
        .build()
        .expect("builder should not validate binary existence");

    let result = claude_wrapper::VersionCommand::new().execute(&claude).await;

    assert!(result.is_err(), "should fail when binary does not exist");
}

/// Verify that CommandFailed includes the working directory in its error message.
#[tokio::test]
async fn command_failed_includes_working_dir() {
    use claude_wrapper::Error;

    let dir = tempfile::tempdir().expect("failed to create tempdir");
    let claude = Claude::builder()
        .binary(fake_binary())
        .env("FAKE_CLAUDE_EXIT_CODE", "1")
        .env("FAKE_CLAUDE_ERROR_MSG", "oops")
        .working_dir(dir.path())
        .build()
        .expect("failed to build client");

    let result = claude_wrapper::VersionCommand::new().execute(&claude).await;

    assert!(result.is_err());
    match result.unwrap_err() {
        Error::CommandFailed { working_dir, .. } => {
            assert_eq!(working_dir.as_deref(), Some(dir.path()));
        }
        other => panic!("expected CommandFailed, got {other:?}"),
    }
}

/// Verify that CommandFailed error message includes the working directory path.
#[tokio::test]
async fn command_failed_error_message_includes_working_dir() {
    let dir = tempfile::tempdir().expect("failed to create tempdir");
    let claude = Claude::builder()
        .binary(fake_binary())
        .env("FAKE_CLAUDE_EXIT_CODE", "1")
        .working_dir(dir.path())
        .build()
        .expect("failed to build client");

    let result = claude_wrapper::VersionCommand::new().execute(&claude).await;

    let err_str = result.unwrap_err().to_string();
    assert!(
        err_str.contains(dir.path().to_str().unwrap()),
        "error message should include working dir, got: {err_str}"
    );
}

/// Verify that spawn failure (nonexistent binary) includes the working directory.
#[tokio::test]
async fn io_error_includes_working_dir() {
    use claude_wrapper::Error;

    let dir = tempfile::tempdir().expect("failed to create tempdir");
    let claude = Claude::builder()
        .binary("/nonexistent/binary/fake-claude")
        .working_dir(dir.path())
        .build()
        .expect("builder should not validate binary existence");

    let result = claude_wrapper::VersionCommand::new().execute(&claude).await;

    assert!(result.is_err());
    match result.unwrap_err() {
        Error::Io { working_dir, .. } => {
            assert_eq!(working_dir.as_deref(), Some(dir.path()));
        }
        other => panic!("expected Io, got {other:?}"),
    }
}

/// Verify that without a working directory set, working_dir is None in errors.
#[tokio::test]
async fn command_failed_without_working_dir_is_none() {
    use claude_wrapper::Error;

    let claude = claude_with_env(&[("FAKE_CLAUDE_EXIT_CODE", "1")]);

    let result = claude_wrapper::VersionCommand::new().execute(&claude).await;

    assert!(result.is_err());
    match result.unwrap_err() {
        Error::CommandFailed { working_dir, .. } => {
            assert!(working_dir.is_none());
        }
        other => panic!("expected CommandFailed, got {other:?}"),
    }
}
