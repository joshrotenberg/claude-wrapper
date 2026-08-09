//! Tests that use the fake-claude binary instead of the real Claude CLI.
//!
//! These tests do not require a real `claude` binary or authentication.

#![cfg(feature = "async")]

use std::path::{Path, PathBuf};

use claude_wrapper::{Claude, ClaudeCommand, OutputFormat, QueryCommand, RetryPolicy};

const FAKE_CLAUDE: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fake-claude.sh");

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

fn env_capture_builder(capture_path: &Path) -> claude_wrapper::ClaudeBuilder {
    Claude::builder()
        .binary(fake_binary())
        .clear_env()
        .env(
            "FAKE_CLAUDE_ENV_CAPTURE_FILE",
            capture_path.to_string_lossy(),
        )
        .env("FAKE_CLAUDE_OUTPUT", "environment captured")
        .env("WRAPPER_EXPLICIT", "present")
}

fn env_capture_client(capture_path: &Path) -> Claude {
    env_capture_builder(capture_path)
        .build()
        .expect("failed to build cleared Claude client")
}

fn captured_env(path: &Path) -> std::collections::HashMap<String, String> {
    std::fs::read_to_string(path)
        .unwrap_or_else(|error| panic!("failed to read {}: {error}", path.display()))
        .lines()
        .filter_map(|line| {
            line.split_once('=')
                .map(|(key, value)| (key.to_string(), value.to_string()))
        })
        .collect()
}

fn assert_cleared_environment(path: &Path) {
    let env = captured_env(path);
    assert_eq!(
        env.get("WRAPPER_EXPLICIT").map(String::as_str),
        Some("present")
    );
    assert!(
        !env.contains_key("HOME"),
        "cleared child unexpectedly inherited HOME: {env:?}"
    );
}

async fn wait_for_capture(path: &Path) {
    for _ in 0..100 {
        if std::fs::read_to_string(path).is_ok_and(|contents| {
            contents
                .lines()
                .any(|line| line == "WRAPPER_EXPLICIT=present")
        }) {
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    panic!("timed out waiting for {}", path.display());
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

#[tokio::test]
async fn child_environment_inherits_by_default_and_clears_on_request() {
    let parent_home = std::env::var("HOME").expect("test process should have HOME");
    let dir = tempfile::tempdir().expect("tempdir");

    let inherited_path = dir.path().join("inherited.env");
    let inherited = Claude::builder()
        .binary(fake_binary())
        .env(
            "FAKE_CLAUDE_ENV_CAPTURE_FILE",
            inherited_path.to_string_lossy(),
        )
        .env("WRAPPER_EXPLICIT", "present")
        .build()
        .expect("inheriting client");
    claude_wrapper::VersionCommand::new()
        .execute(&inherited)
        .await
        .expect("inheriting spawn");
    let inherited_env = captured_env(&inherited_path);
    assert_eq!(
        inherited_env.get("HOME").map(String::as_str),
        Some(parent_home.as_str())
    );
    assert_eq!(
        inherited_env.get("WRAPPER_EXPLICIT").map(String::as_str),
        Some("present")
    );

    let cleared_path = dir.path().join("cleared.env");
    claude_wrapper::VersionCommand::new()
        .execute(&env_capture_client(&cleared_path))
        .await
        .expect("cleared spawn");
    assert_cleared_environment(&cleared_path);
}

#[tokio::test]
async fn clear_env_applies_to_all_async_exec_paths() {
    use claude_wrapper::exec::run_claude_with_stdin_prompt;

    let dir = tempfile::tempdir().expect("tempdir");

    let buffered_path = dir.path().join("buffered.env");
    claude_wrapper::VersionCommand::new()
        .execute(&env_capture_client(&buffered_path))
        .await
        .expect("buffered execution");
    assert_cleared_environment(&buffered_path);

    let buffered_timeout_path = dir.path().join("buffered-timeout.env");
    let buffered_timeout = env_capture_builder(&buffered_timeout_path)
        .timeout(std::time::Duration::from_secs(5))
        .build()
        .expect("timeout client");
    claude_wrapper::VersionCommand::new()
        .execute(&buffered_timeout)
        .await
        .expect("buffered timeout-path execution");
    assert_cleared_environment(&buffered_timeout_path);

    let stdin_path = dir.path().join("stdin.env");
    run_claude_with_stdin_prompt(
        &env_capture_client(&stdin_path),
        vec!["--version".to_string()],
        "prompt on stdin".to_string(),
    )
    .await
    .expect("stdin execution");
    assert_cleared_environment(&stdin_path);

    let stdin_timeout_path = dir.path().join("stdin-timeout.env");
    let stdin_timeout = env_capture_builder(&stdin_timeout_path)
        .timeout(std::time::Duration::from_secs(5))
        .build()
        .expect("stdin timeout client");
    run_claude_with_stdin_prompt(
        &stdin_timeout,
        vec!["--version".to_string()],
        "prompt on stdin".to_string(),
    )
    .await
    .expect("stdin timeout-path execution");
    assert_cleared_environment(&stdin_timeout_path);

    let retry_path = dir.path().join("retry.env");
    let retry_marker = dir.path().join("retried");
    let retry = env_capture_builder(&retry_path)
        .env("FAKE_CLAUDE_FAIL_ONCE_FILE", retry_marker.to_string_lossy())
        .retry(
            RetryPolicy::new()
                .max_attempts(2)
                .initial_backoff(std::time::Duration::from_millis(1))
                .retry_on_exit_codes([75]),
        )
        .build()
        .expect("retry client");
    claude_wrapper::VersionCommand::new()
        .execute(&retry)
        .await
        .expect("second retry attempt should succeed");
    assert!(retry_marker.is_file(), "first attempt should create marker");
    assert_cleared_environment(&retry_path);
}

#[cfg(feature = "json")]
#[tokio::test]
async fn clear_env_applies_to_streaming_and_session_open_resume() {
    use claude_wrapper::session::Session;
    use claude_wrapper::streaming::{StreamEvent, stream_query};
    use std::sync::Arc;

    let dir = tempfile::tempdir().expect("tempdir");

    let stream_path = dir.path().join("stream.env");
    let stream_client = env_capture_client(&stream_path);
    let stream_command = QueryCommand::new("stream")
        .output_format(OutputFormat::StreamJson)
        .no_session_persistence();
    stream_query(&stream_client, &stream_command, |_: StreamEvent| {})
        .await
        .expect("streaming execution");
    assert_cleared_environment(&stream_path);

    let fresh_path = dir.path().join("session-fresh.env");
    let mut fresh = Session::new(Arc::new(env_capture_client(&fresh_path)));
    fresh.send("fresh").await.expect("fresh session turn");
    assert_cleared_environment(&fresh_path);

    let resumed_path = dir.path().join("session-resumed.env");
    let mut resumed = Session::resume(
        Arc::new(env_capture_client(&resumed_path)),
        "existing-session-id",
    );
    resumed.send("resume").await.expect("resumed session turn");
    assert_cleared_environment(&resumed_path);
}

#[cfg(feature = "json")]
#[tokio::test]
async fn clear_env_applies_to_duplex_open_and_resume() {
    use claude_wrapper::duplex::{DuplexOptions, DuplexSession};

    let dir = tempfile::tempdir().expect("tempdir");

    let fresh_path = dir.path().join("duplex-fresh.env");
    let fresh_client = env_capture_client(&fresh_path);
    let fresh = DuplexSession::spawn(&fresh_client, DuplexOptions::default())
        .await
        .expect("fresh duplex spawn");
    wait_for_capture(&fresh_path).await;
    assert_cleared_environment(&fresh_path);
    fresh.close().await.expect("close fresh duplex session");

    let resumed_path = dir.path().join("duplex-resumed.env");
    let resumed_client = env_capture_client(&resumed_path);
    let resumed = DuplexSession::spawn(
        &resumed_client,
        DuplexOptions::default().resume("existing-session-id"),
    )
    .await
    .expect("resumed duplex spawn");
    wait_for_capture(&resumed_path).await;
    assert_cleared_environment(&resumed_path);
    resumed.close().await.expect("close resumed duplex session");
}

#[tokio::test]
async fn environment_values_are_absent_from_debug_and_errors() {
    let secret = "issue-782-environment-secret";
    let builder = Claude::builder()
        .binary(fake_binary())
        .clear_env()
        .env("WRAPPER_SECRET", secret);
    assert!(!format!("{builder:?}").contains(secret));

    let claude = Claude::builder()
        .binary(fake_binary())
        .clear_env()
        .env("WRAPPER_SECRET", secret)
        .env("FAKE_CLAUDE_EXIT_CODE", "1")
        .env("FAKE_CLAUDE_ERROR_MSG", "generic failure")
        .build()
        .expect("client");
    assert!(!format!("{claude:?}").contains(secret));
    let error = claude_wrapper::VersionCommand::new()
        .execute(&claude)
        .await
        .expect_err("fake failure");
    assert!(!error.to_string().contains(secret));
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

/// A stdout-only auth diagnostic on a failed stream is classified,
/// while valid JSON lines remain ordered callback events only.
#[tokio::test]
async fn streaming_stdout_auth_error_preserves_only_diagnostics() {
    use claude_wrapper::Error;
    use claude_wrapper::auth::AuthErrorKind;
    use claude_wrapper::streaming::{StreamEvent, stream_query};

    let claude = claude_with_env(&[("FAKE_CLAUDE_STDOUT_AUTH_ERROR", "1")]);
    let cmd = QueryCommand::new("test prompt")
        .output_format(OutputFormat::StreamJson)
        .no_session_persistence();

    let mut events: Vec<StreamEvent> = Vec::new();
    let error = stream_query(&claude, &cmd, |event| events.push(event))
        .await
        .expect_err("stdout-only auth failure should return an error");

    assert_eq!(
        events
            .iter()
            .filter_map(StreamEvent::event_type)
            .collect::<Vec<_>>(),
        ["system", "assistant"]
    );
    match error {
        Error::Auth { kind, message, .. } => {
            assert_eq!(kind, AuthErrorKind::NotAuthenticated);
            assert_eq!(message, "Not authenticated. Run `claude login`.");
        }
        other => panic!("expected Auth, got {other:?}"),
    }
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

// ── Session API (#523) ───────────────────────────────────────────────

/// New session: first `send` discovers the id; second `send` reuses it
/// via --resume, and cumulative cost/turns accumulate.
#[tokio::test]
async fn session_send_accumulates_across_turns() {
    use claude_wrapper::session::Session;
    use std::sync::Arc;

    let claude = Arc::new(
        Claude::builder()
            .binary(fake_binary())
            .env("FAKE_CLAUDE_OUTPUT", "hello")
            .env("FAKE_CLAUDE_SESSION_ID", "sess-001")
            .env("FAKE_CLAUDE_COST_USD", "0.05")
            .env("FAKE_CLAUDE_NUM_TURNS", "2")
            .build()
            .expect("failed to build client"),
    );

    let mut session = Session::new(Arc::clone(&claude));
    assert!(session.id().is_none());

    let first = session
        .send("start")
        .await
        .expect("first send should succeed");
    assert_eq!(first.session_id, "sess-001");
    assert_eq!(session.id(), Some("sess-001"));
    assert!((session.total_cost_usd() - 0.05).abs() < f64::EPSILON);
    assert_eq!(session.total_turns(), 2);
    assert_eq!(session.history().len(), 1);

    let second = session
        .send("follow up")
        .await
        .expect("second send should succeed");
    assert_eq!(second.session_id, "sess-001");
    assert!((session.total_cost_usd() - 0.10).abs() < f64::EPSILON);
    assert_eq!(session.total_turns(), 4);
    assert_eq!(session.history().len(), 2);
    assert_eq!(
        session.last_result().map(|r| r.result.as_str()),
        Some("hello")
    );
}

/// Session::resume starts with a preset id that gets passed on the
/// first turn.
#[tokio::test]
async fn session_resume_uses_preset_id() {
    use claude_wrapper::session::Session;
    use std::sync::Arc;

    let claude = Arc::new(
        Claude::builder()
            .binary(fake_binary())
            .env("FAKE_CLAUDE_OUTPUT", "resumed")
            .env("FAKE_CLAUDE_SESSION_ID", "sess-preset")
            .build()
            .expect("failed to build client"),
    );

    let mut session = Session::resume(claude, "sess-preset");
    assert_eq!(session.id(), Some("sess-preset"));

    let result = session
        .send("pick up where we left off")
        .await
        .expect("resumed send should succeed");
    assert_eq!(result.session_id, "sess-preset");
}

/// Session::execute accepts a caller-built QueryCommand and overrides
/// any session-related flags the caller set.
#[tokio::test]
async fn session_execute_overrides_conflicting_flags() {
    use claude_wrapper::session::Session;
    use std::sync::Arc;

    let claude = Arc::new(
        Claude::builder()
            .binary(fake_binary())
            .env("FAKE_CLAUDE_OUTPUT", "ok")
            .env("FAKE_CLAUDE_SESSION_ID", "sess-real")
            .build()
            .expect("failed to build client"),
    );

    let mut session = Session::resume(Arc::clone(&claude), "sess-real");

    // Caller builds a command with a stale/conflicting resume id +
    // continue flag. Session::execute should clear those and inject
    // the session's actual id.
    let cmd = QueryCommand::new("hi")
        .model("sonnet")
        .resume("stale-id")
        .continue_session();

    let result = session.execute(cmd).await.expect("execute should succeed");
    // The fake binary echoes whatever session id is in its env var,
    // but the important check is that the call succeeded (it would
    // have errored with conflicting --resume + --continue on a real
    // CLI).
    assert_eq!(result.session_id, "sess-real");
}

/// Session::stream captures the session id from the first event that
/// carries one, even if the caller ignores the session_id field in
/// their handler.
#[tokio::test]
async fn session_stream_captures_session_id() {
    use claude_wrapper::session::Session;
    use std::sync::Arc;

    let claude = Arc::new(
        Claude::builder()
            .binary(fake_binary())
            .env("FAKE_CLAUDE_OUTPUT", "streamed")
            .env("FAKE_CLAUDE_SESSION_ID", "sess-stream-1")
            .env("FAKE_CLAUDE_COST_USD", "0.03")
            .env("FAKE_CLAUDE_NUM_TURNS", "1")
            .build()
            .expect("failed to build client"),
    );

    let mut session = Session::new(Arc::clone(&claude));
    let mut event_count = 0;

    session
        .stream("tell me a story", |_| {
            event_count += 1;
        })
        .await
        .expect("stream should succeed");

    assert!(
        event_count >= 3,
        "expected at least 3 events (system/assistant/result)"
    );
    assert_eq!(session.id(), Some("sess-stream-1"));
    assert!((session.total_cost_usd() - 0.03).abs() < f64::EPSILON);
    assert_eq!(session.total_turns(), 1);
    assert_eq!(session.history().len(), 1);
}

/// After a stream, a subsequent plain `send` resumes the captured id.
#[tokio::test]
async fn session_stream_then_send_resumes() {
    use claude_wrapper::session::Session;
    use std::sync::Arc;

    let claude = Arc::new(
        Claude::builder()
            .binary(fake_binary())
            .env("FAKE_CLAUDE_OUTPUT", "x")
            .env("FAKE_CLAUDE_SESSION_ID", "sess-stream-2")
            .build()
            .expect("failed to build client"),
    );

    let mut session = Session::new(Arc::clone(&claude));
    session
        .stream("first", |_| {})
        .await
        .expect("stream should succeed");
    assert_eq!(session.id(), Some("sess-stream-2"));

    let second = session
        .send("second")
        .await
        .expect("follow-up send should succeed");
    assert_eq!(second.session_id, "sess-stream-2");
}

/// Session is Send + Sync (compile check): Arc<Claude> unlocks this.
#[tokio::test]
async fn session_is_send_and_sync() {
    use claude_wrapper::session::Session;
    use std::sync::Arc;

    fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<Session>();

    // And can be moved into a spawned task.
    let claude = Arc::new(
        Claude::builder()
            .binary(fake_binary())
            .env("FAKE_CLAUDE_OUTPUT", "spawned")
            .build()
            .expect("failed to build client"),
    );

    let session = Session::new(claude);
    let handle = tokio::spawn(async move {
        // just exercise that Session moved in
        session.id().map(|s| s.to_string())
    });
    let _ = handle.await.unwrap();
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
///
/// `AgentsCommand` is deprecated (the real `claude agents` is now an
/// interactive TUI as of 2.1.143), but the wrapper still constructs
/// the same arg vector and the test verifies that arg vector lands
/// on the fake binary correctly.
#[allow(deprecated)]
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

// ── Cancellation: dropping an in-flight future kills the child ──────

/// Drive `fut` just long enough for fake-claude.sh to write its pid file
/// (FAKE_CLAUDE_PID_FILE), then drop it mid-flight (on return) and hand
/// back the pid. Unix-only helpers: the assertions shell out to `ps`.
#[cfg(unix)]
async fn drop_in_flight_and_capture_pid<F>(fut: F, pid_path: &std::path::Path) -> u32
where
    F: std::future::Future,
    F::Output: std::fmt::Debug,
{
    tokio::pin!(fut);
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    loop {
        if let Some(pid) = std::fs::read_to_string(pid_path)
            .ok()
            .and_then(|s| s.trim().parse().ok())
        {
            // Returning drops the pinned future here, mid-flight.
            return pid;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "child never wrote its pid file"
        );
        tokio::select! {
            out = &mut fut => panic!("future completed before drop: {out:?}"),
            _ = tokio::time::sleep(std::time::Duration::from_millis(10)) => {}
        }
    }
}

/// Poll until `pid` is dead or a zombie awaiting reap. `kill_on_drop`
/// SIGKILLs at drop time, but reaping happens asynchronously on the
/// runtime's process driver, so a transient zombie counts as killed.
#[cfg(unix)]
async fn assert_pid_killed(pid: u32) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    loop {
        let out = std::process::Command::new("ps")
            .args(["-o", "stat=", "-p", &pid.to_string()])
            .output()
            .expect("run ps");
        let stat = String::from_utf8_lossy(&out.stdout).trim().to_string();
        if !out.status.success() || stat.is_empty() || stat.starts_with('Z') {
            return;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "child {pid} still alive (stat {stat}) after future drop"
        );
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
    }
}

/// Read a pid recorded by fake-claude.sh.
#[cfg(unix)]
fn read_pid(path: &std::path::Path) -> u32 {
    std::fs::read_to_string(path)
        .expect("read pid file")
        .trim()
        .parse()
        .expect("parse pid")
}

/// Dropping an in-flight execute future kills the CLI child and its
/// whole process group instead of leaving anything to run on in the
/// background. This is the MCP server shape: execute_json raced against
/// client cancellation with tokio::select!. FAKE_CLAUDE_DELAY keeps the
/// child alive far longer than the test; the drop is what kills it, and
/// the grandchild (FAKE_CLAUDE_GRANDCHILD_PID_FILE) must die with it.
#[cfg(unix)]
#[tokio::test]
async fn dropping_in_flight_execute_kills_child() {
    let dir = tempfile::tempdir().expect("tempdir");
    let pid_path = dir.path().join("pid");
    let gpid_path = dir.path().join("gpid");
    let claude = claude_with_env(&[
        ("FAKE_CLAUDE_DELAY", "30"),
        (
            "FAKE_CLAUDE_PID_FILE",
            pid_path.to_str().expect("utf8 path"),
        ),
        (
            "FAKE_CLAUDE_GRANDCHILD_PID_FILE",
            gpid_path.to_str().expect("utf8 path"),
        ),
    ]);

    let cmd = QueryCommand::new("slow query").no_session_persistence();
    let pid = drop_in_flight_and_capture_pid(cmd.execute(&claude), &pid_path).await;
    assert_pid_killed(pid).await;
    assert_pid_killed(read_pid(&gpid_path)).await;
}

/// Same guarantee for the streaming path.
#[cfg(unix)]
#[tokio::test]
async fn dropping_in_flight_stream_query_kills_child() {
    use claude_wrapper::streaming::{StreamEvent, stream_query};

    let dir = tempfile::tempdir().expect("tempdir");
    let pid_path = dir.path().join("pid");
    let gpid_path = dir.path().join("gpid");
    let claude = claude_with_env(&[
        ("FAKE_CLAUDE_DELAY", "30"),
        (
            "FAKE_CLAUDE_PID_FILE",
            pid_path.to_str().expect("utf8 path"),
        ),
        (
            "FAKE_CLAUDE_GRANDCHILD_PID_FILE",
            gpid_path.to_str().expect("utf8 path"),
        ),
    ]);

    let cmd = QueryCommand::new("slow stream")
        .output_format(OutputFormat::StreamJson)
        .no_session_persistence();
    let pid =
        drop_in_flight_and_capture_pid(stream_query(&claude, &cmd, |_: StreamEvent| {}), &pid_path)
            .await;
    assert_pid_killed(pid).await;
    assert_pid_killed(read_pid(&gpid_path)).await;
}

// ── Token accounting ─────────────────────────────────────────────────

/// A result event carrying a usage object lands typed on QueryResult,
/// with the Anthropic cache key names mapped onto the shared field names.
#[tokio::test]
async fn execute_json_parses_usage() {
    let claude = claude_with_env(&[
        ("FAKE_CLAUDE_OUTPUT", "ok"),
        (
            "FAKE_CLAUDE_USAGE_JSON",
            r#"{"input_tokens":100,"cache_read_input_tokens":400,"output_tokens":25}"#,
        ),
    ]);
    let result = QueryCommand::new("hi")
        .no_session_persistence()
        .execute_json(&claude)
        .await
        .expect("query succeeds");
    let usage = result.usage.expect("usage present");
    assert_eq!(usage.input_tokens, Some(100));
    assert_eq!(usage.cached_input_tokens, Some(400));
    assert_eq!(usage.output_tokens, Some(25));
    assert_eq!(usage.total(), 525);
}

/// A turn whose result carries usage counts toward Session::total_tokens.
#[tokio::test]
async fn session_counts_tokens_when_usage_reported() {
    use claude_wrapper::session::Session;
    use std::sync::Arc;

    let claude = Arc::new(claude_with_env(&[
        ("FAKE_CLAUDE_OUTPUT", "one"),
        (
            "FAKE_CLAUDE_USAGE_JSON",
            r#"{"input_tokens":10,"output_tokens":5}"#,
        ),
    ]));
    let mut session = Session::new(claude);
    session.send("first").await.expect("turn succeeds");

    assert_eq!(session.total_tokens(), 15);
    assert_eq!(session.turns_missing_usage(), 0);
}

/// A turn without usage increments turns_missing_usage rather than
/// counting as a zero-token turn.
#[tokio::test]
async fn session_flags_turn_without_usage() {
    use claude_wrapper::session::Session;
    use std::sync::Arc;

    let claude = Arc::new(claude_with_env(&[("FAKE_CLAUDE_OUTPUT", "one")]));
    let mut session = Session::new(claude);
    session.send("first").await.expect("turn succeeds");

    assert_eq!(session.total_tokens(), 0);
    assert_eq!(session.turns_missing_usage(), 1);
}
