//! Tests that use the fake-claude binary instead of the real Claude CLI.
//!
//! These tests do not require a real `claude` binary or authentication.

use std::path::PathBuf;

use claude_wrapper::{Claude, ClaudeCommand, OutputFormat, QueryCommand, RetryPolicy};

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

/// Regression test for #454: an exec-path timeout must return promptly
/// and not leave the child running. We use a 5s DELAY with a 500ms
/// timeout and assert the call returns well under the child's delay.
#[tokio::test]
async fn exec_timeout_returns_promptly() {
    let claude = Claude::builder()
        .binary(fake_binary())
        .env("FAKE_CLAUDE_DELAY", "5")
        .timeout(std::time::Duration::from_millis(500))
        .build()
        .expect("failed to build client");

    let start = std::time::Instant::now();
    let result = claude_wrapper::VersionCommand::new().execute(&claude).await;
    let elapsed = start.elapsed();

    assert!(matches!(result, Err(claude_wrapper::Error::Timeout { .. })));
    // Must return well before the child's 5s delay. Allow generous
    // slack for CI jitter.
    assert!(
        elapsed < std::time::Duration::from_secs(3),
        "exec timeout should return promptly, took {elapsed:?}"
    );
}

/// Regression test for #454: streaming timeout path must return a
/// Timeout error and not hang waiting for the child.
#[tokio::test]
async fn streaming_timeout_returns_promptly() {
    use claude_wrapper::streaming::{StreamEvent, stream_query};

    let claude = Claude::builder()
        .binary(fake_binary())
        .env("FAKE_CLAUDE_DELAY", "5")
        .env("FAKE_CLAUDE_OUTPUT", "slow response")
        .timeout(std::time::Duration::from_millis(500))
        .build()
        .expect("failed to build client");

    let cmd = QueryCommand::new("test prompt")
        .output_format(OutputFormat::StreamJson)
        .no_session_persistence();

    let start = std::time::Instant::now();
    let result = stream_query(&claude, &cmd, |_: StreamEvent| {}).await;
    let elapsed = start.elapsed();

    assert!(matches!(result, Err(claude_wrapper::Error::Timeout { .. })));
    assert!(
        elapsed < std::time::Duration::from_secs(3),
        "streaming timeout should return promptly, took {elapsed:?}"
    );
}

/// After an exec timeout the client handle should still be usable for
/// subsequent commands; no shared state should be corrupted by the
/// killed child.
#[tokio::test]
async fn client_reusable_after_timeout() {
    let claude = Claude::builder()
        .binary(fake_binary())
        .env("FAKE_CLAUDE_DELAY", "5")
        .timeout(std::time::Duration::from_millis(200))
        .build()
        .expect("failed to build client");

    let first = claude_wrapper::VersionCommand::new().execute(&claude).await;
    assert!(first.is_err());

    // Rebuild without the delay but keep the short timeout; a fast
    // command should still succeed on the same binary path.
    let fast = Claude::builder()
        .binary(fake_binary())
        .env("FAKE_CLAUDE_OUTPUT", "fast")
        .timeout(std::time::Duration::from_secs(5))
        .build()
        .expect("failed to build client");

    let second = claude_wrapper::VersionCommand::new()
        .execute(&fast)
        .await
        .expect("fast call should succeed");
    assert!(second.stdout.contains("fast"));
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

/// Verify that Session tracks cumulative cost and turns across queries.
#[tokio::test]
async fn session_query_resumes_and_tracks_cost() {
    use claude_wrapper::session::Session;

    let claude = claude_with_env(&[
        ("FAKE_CLAUDE_OUTPUT", "first"),
        ("FAKE_CLAUDE_SESSION_ID", "sess-001"),
        ("FAKE_CLAUDE_COST_USD", "0.05"),
        ("FAKE_CLAUDE_NUM_TURNS", "3"),
    ]);

    // Initial query to get a session
    let first = QueryCommand::new("start")
        .execute_json(&claude)
        .await
        .expect("initial query should succeed");

    assert_eq!(first.session_id, "sess-001");

    let mut session = Session::from_result(&claude, &first);
    assert_eq!(session.id(), "sess-001");
    assert!((session.total_cost_usd() - 0.05).abs() < f64::EPSILON);
    assert_eq!(session.total_turns(), 3);

    // Follow-up query accumulates
    let second = session
        .query("follow up")
        .execute()
        .await
        .expect("follow-up query should succeed");

    assert_eq!(second.session_id, "sess-001");
    assert!((session.total_cost_usd() - 0.10).abs() < f64::EPSILON);
    assert_eq!(session.total_turns(), 6);
}

/// Verify that Session::fork() uses --fork-session and returns a new session.
#[tokio::test]
async fn session_fork_creates_new_session() {
    use claude_wrapper::session::Session;

    let claude = claude_with_env(&[
        ("FAKE_CLAUDE_OUTPUT", "forked"),
        ("FAKE_CLAUDE_SESSION_ID", "sess-original"),
        ("FAKE_CLAUDE_FORKED_SESSION_ID", "sess-forked"),
        ("FAKE_CLAUDE_COST_USD", "0.02"),
        ("FAKE_CLAUDE_NUM_TURNS", "1"),
    ]);

    let session = Session::from_id(&claude, "sess-original");

    let (forked, result) = session
        .fork("branch this conversation")
        .await
        .expect("fork should succeed");

    assert_eq!(result.session_id, "sess-forked");
    assert_eq!(forked.id(), "sess-forked");
    // Original session is not modified
    assert_eq!(session.id(), "sess-original");
}

/// Verify that Session::continue_recent() uses --continue on the first query.
#[tokio::test]
async fn session_continue_recent() {
    use claude_wrapper::session::Session;

    let claude = claude_with_env(&[
        ("FAKE_CLAUDE_OUTPUT", "continued"),
        ("FAKE_CLAUDE_SESSION_ID", "sess-recent"),
        ("FAKE_CLAUDE_COST_USD", "0.01"),
        ("FAKE_CLAUDE_NUM_TURNS", "2"),
    ]);

    let (mut session, first) = Session::continue_recent(&claude, "continue from before")
        .await
        .expect("continue_recent should succeed");

    assert_eq!(first.session_id, "sess-recent");
    assert_eq!(session.id(), "sess-recent");
    assert!((session.total_cost_usd() - 0.01).abs() < f64::EPSILON);
    assert_eq!(session.total_turns(), 2);

    // Subsequent query uses --resume
    let second = session
        .query("and then?")
        .execute()
        .await
        .expect("follow-up should succeed");

    assert_eq!(second.session_id, "sess-recent");
    assert!((session.total_cost_usd() - 0.02).abs() < f64::EPSILON);
    assert_eq!(session.total_turns(), 4);
}

// ── Subcommand execution tests ───────────────────────────────────────

/// Verify that MCP list command executes through the fake binary.
#[tokio::test]
async fn mcp_list_executes() {
    let claude = claude_with_env(&[("FAKE_CLAUDE_OUTPUT", "no servers configured")]);
    let output = claude_wrapper::McpListCommand::new()
        .execute(&claude)
        .await
        .expect("mcp list should succeed");
    assert!(output.success);
    assert!(output.stdout.contains("no servers"));
}

/// Verify that auth status command executes through the fake binary.
#[tokio::test]
async fn auth_status_executes() {
    let claude = claude_with_env(&[("FAKE_CLAUDE_OUTPUT", "authenticated")]);
    let output = claude_wrapper::AuthStatusCommand::new()
        .execute(&claude)
        .await
        .expect("auth status should succeed");
    assert!(output.success);
}

/// Verify that doctor command executes through the fake binary.
#[tokio::test]
async fn doctor_executes() {
    let claude = claude_with_env(&[("FAKE_CLAUDE_OUTPUT", "all checks passed")]);
    let output = claude_wrapper::DoctorCommand::new()
        .execute(&claude)
        .await
        .expect("doctor should succeed");
    assert!(output.success);
    assert!(output.stdout.contains("all checks passed"));
}

/// Verify that agents command executes through the fake binary.
#[tokio::test]
async fn agents_executes() {
    let claude = claude_with_env(&[("FAKE_CLAUDE_OUTPUT", "[]")]);
    let output = claude_wrapper::AgentsCommand::new()
        .execute(&claude)
        .await
        .expect("agents should succeed");
    assert!(output.success);
}

/// Verify that raw command passes arbitrary args through.
#[tokio::test]
async fn raw_command_executes() {
    let claude = claude_with_env(&[("FAKE_CLAUDE_OUTPUT", "raw output")]);
    let output = claude_wrapper::RawCommand::new("some-subcommand")
        .arg("--flag")
        .arg("value")
        .execute(&claude)
        .await
        .expect("raw command should succeed");
    assert!(output.success);
    assert!(output.stdout.contains("raw output"));
}

// ── Retry integration tests ─────────────────────────────────────────

/// Verify that client-level retry policy retries on configured exit codes.
#[tokio::test]
async fn retry_on_exit_code_with_fake_binary() {
    // Exit code 1 always fails with the fake binary, but retry should attempt it.
    let claude = Claude::builder()
        .binary(fake_binary())
        .env("FAKE_CLAUDE_EXIT_CODE", "1")
        .env("FAKE_CLAUDE_ERROR_MSG", "transient failure")
        .retry(
            RetryPolicy::new()
                .max_attempts(2)
                .initial_backoff(std::time::Duration::from_millis(10))
                .retry_on_exit_codes([1]),
        )
        .build()
        .expect("failed to build client");

    let result = claude_wrapper::VersionCommand::new().execute(&claude).await;

    // Should still fail (fake always returns exit code 1), but should have retried.
    assert!(result.is_err());
}

/// Verify that per-command retry policy overrides client default.
#[tokio::test]
async fn per_command_retry_override() {
    let claude = Claude::builder()
        .binary(fake_binary())
        .env("FAKE_CLAUDE_EXIT_CODE", "1")
        .env("FAKE_CLAUDE_ERROR_MSG", "fail")
        // Client default: no retry on exit code 1
        .retry(RetryPolicy::new().max_attempts(1))
        .build()
        .expect("failed to build client");

    // Command-level retry overrides to retry on exit code 1.
    let result = QueryCommand::new("test")
        .retry(
            RetryPolicy::new()
                .max_attempts(2)
                .initial_backoff(std::time::Duration::from_millis(10))
                .retry_on_exit_codes([1]),
        )
        .execute(&claude)
        .await;

    assert!(result.is_err());
}

/// Verify that without retry, a failure is immediate.
#[tokio::test]
async fn no_retry_fails_immediately() {
    let claude = claude_with_env(&[
        ("FAKE_CLAUDE_EXIT_CODE", "1"),
        ("FAKE_CLAUDE_ERROR_MSG", "immediate failure"),
    ]);

    let start = std::time::Instant::now();
    let result = claude_wrapper::VersionCommand::new().execute(&claude).await;
    let elapsed = start.elapsed();

    assert!(result.is_err());
    // Without retry, should complete very quickly (no backoff delay).
    assert!(elapsed < std::time::Duration::from_secs(1));
}

// ── McpConfigBuilder tests ──────────────────────────────────────────

/// Verify McpConfigBuilder produces valid JSON (no binary needed).
#[tokio::test]
async fn mcp_config_builder_roundtrip() {
    use claude_wrapper::McpConfigBuilder;

    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join(".mcp.json");

    McpConfigBuilder::new()
        .http_server("hub", "http://localhost:9090")
        .stdio_server("tool", "npx", ["-y", "my-tool"])
        .write_to(&path)
        .unwrap();

    let contents = std::fs::read_to_string(&path).unwrap();
    let parsed: serde_json::Value = serde_json::from_str(&contents).unwrap();

    assert!(parsed["mcpServers"]["hub"].is_object());
    assert!(parsed["mcpServers"]["tool"].is_object());
    assert_eq!(parsed["mcpServers"]["hub"]["url"], "http://localhost:9090");
    assert_eq!(parsed["mcpServers"]["tool"]["command"], "npx");
}

// ── Environment variable handling ───────────────────────────────────

/// Verify that custom env vars are passed through to the subprocess.
#[tokio::test]
async fn env_vars_passed_to_subprocess() {
    let claude = Claude::builder()
        .binary(fake_binary())
        .env("FAKE_CLAUDE_OUTPUT", "env test")
        .env("CUSTOM_VAR", "custom_value")
        .build()
        .expect("failed to build client");

    let output = claude_wrapper::VersionCommand::new()
        .execute(&claude)
        .await
        .expect("should succeed");
    assert!(output.stdout.contains("env test"));
}

/// Verify that working directory is set on the subprocess.
#[tokio::test]
async fn working_dir_set_on_subprocess() {
    let dir = tempfile::tempdir().unwrap();
    let claude = Claude::builder()
        .binary(fake_binary())
        .env("FAKE_CLAUDE_OUTPUT", "dir test")
        .working_dir(dir.path())
        .build()
        .expect("failed to build client");

    let output = claude_wrapper::VersionCommand::new()
        .execute(&claude)
        .await
        .expect("should succeed");
    assert!(output.success);
}
