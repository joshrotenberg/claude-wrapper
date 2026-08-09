//! Integration tests for the blocking/sync exec path.
//!
//! These cover the primitives in `exec::*_sync` against the fake-claude
//! shell script. The sync command surface (per-builder `execute_sync`)
//! is landing in a follow-up PR, so these tests reach into the exec
//! module directly.

#![cfg(feature = "sync")]

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use claude_wrapper::Claude;
use claude_wrapper::exec::run_claude_sync;

const FAKE_CLAUDE: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fake-claude.sh");

fn fake_binary() -> PathBuf {
    PathBuf::from(FAKE_CLAUDE)
}

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

fn assert_cleared_environment(path: &Path) {
    let env: std::collections::HashMap<String, String> = std::fs::read_to_string(path)
        .unwrap_or_else(|error| panic!("failed to read {}: {error}", path.display()))
        .lines()
        .filter_map(|line| {
            line.split_once('=')
                .map(|(key, value)| (key.to_string(), value.to_string()))
        })
        .collect();
    let mut keys: Vec<_> = env.keys().cloned().collect();
    keys.sort();
    assert_eq!(
        env.get("WRAPPER_EXPLICIT").map(String::as_str),
        Some("present")
    );
    assert!(
        !env.contains_key("HOME"),
        "cleared child unexpectedly inherited HOME; exported keys: {keys:?}"
    );
}

#[test]
fn sync_basic_execution_returns_stdout() {
    let claude = claude_with_env(&[("FAKE_CLAUDE_OUTPUT", "hello sync")]);
    let output = run_claude_sync(&claude, vec!["--version".to_string()])
        .expect("sync execution should succeed");
    assert!(output.success);
    assert!(
        output.stdout.contains("hello sync"),
        "unexpected stdout: {:?}",
        output.stdout
    );
    assert_eq!(output.exit_code, 0);
}

#[test]
fn clear_env_applies_to_all_sync_exec_paths() {
    use claude_wrapper::exec::run_claude_with_stdin_prompt_sync;

    let dir = tempfile::tempdir().expect("tempdir");

    let buffered_path = dir.path().join("buffered.env");
    run_claude_sync(
        &env_capture_client(&buffered_path),
        vec!["--version".to_string()],
    )
    .expect("buffered sync execution");
    assert_cleared_environment(&buffered_path);

    let buffered_timeout_path = dir.path().join("buffered-timeout.env");
    let buffered_timeout = env_capture_builder(&buffered_timeout_path)
        .timeout(Duration::from_secs(5))
        .build()
        .expect("timeout client");
    run_claude_sync(&buffered_timeout, vec!["--version".to_string()])
        .expect("buffered sync timeout-path execution");
    assert_cleared_environment(&buffered_timeout_path);

    let stdin_path = dir.path().join("stdin.env");
    run_claude_with_stdin_prompt_sync(
        &env_capture_client(&stdin_path),
        vec!["--version".to_string()],
        "prompt on stdin".to_string(),
    )
    .expect("stdin sync execution");
    assert_cleared_environment(&stdin_path);

    let stdin_timeout_path = dir.path().join("stdin-timeout.env");
    let stdin_timeout = env_capture_builder(&stdin_timeout_path)
        .timeout(Duration::from_secs(5))
        .build()
        .expect("stdin timeout client");
    run_claude_with_stdin_prompt_sync(
        &stdin_timeout,
        vec!["--version".to_string()],
        "prompt on stdin".to_string(),
    )
    .expect("stdin sync timeout-path execution");
    assert_cleared_environment(&stdin_timeout_path);

    let retry_path = dir.path().join("retry.env");
    let retry_marker = dir.path().join("retried");
    let retry = env_capture_builder(&retry_path)
        .env("FAKE_CLAUDE_FAIL_ONCE_FILE", retry_marker.to_string_lossy())
        .retry(
            claude_wrapper::RetryPolicy::new()
                .max_attempts(2)
                .initial_backoff(Duration::from_millis(1))
                .retry_on_exit_codes([75]),
        )
        .build()
        .expect("retry client");
    run_claude_sync(&retry, vec!["--version".to_string()])
        .expect("second sync retry attempt should succeed");
    assert!(retry_marker.is_file(), "first attempt should create marker");
    assert_cleared_environment(&retry_path);
}

#[cfg(feature = "json")]
#[test]
fn clear_env_applies_to_sync_streaming() {
    use claude_wrapper::streaming::{StreamEvent, stream_query_sync};
    use claude_wrapper::{OutputFormat, QueryCommand};

    let dir = tempfile::tempdir().expect("tempdir");
    let capture_path = dir.path().join("stream.env");
    let claude = env_capture_client(&capture_path);
    let command = QueryCommand::new("stream")
        .output_format(OutputFormat::StreamJson)
        .no_session_persistence();
    stream_query_sync(&claude, &command, |_: StreamEvent| {}).expect("sync streaming execution");
    assert_cleared_environment(&capture_path);
}

#[test]
fn sync_non_zero_exit_surfaces_error() {
    let claude = claude_with_env(&[
        ("FAKE_CLAUDE_EXIT_CODE", "1"),
        ("FAKE_CLAUDE_ERROR_MSG", "sync boom"),
    ]);
    let result = run_claude_sync(&claude, vec!["--version".to_string()]);
    let err = result.expect_err("non-zero exit must be an error");
    assert!(
        err.to_string().contains("exit code 1"),
        "unexpected error: {err}"
    );
}

#[test]
fn sync_timeout_fires_and_returns_promptly() {
    // Fake child sleeps 5s; timeout is 300ms. Without a working
    // concurrent drain + wait-timeout path, this either hangs (pipe
    // buffer deadlock) or overshoots massively.
    let claude = Claude::builder()
        .binary(fake_binary())
        .env("FAKE_CLAUDE_DELAY", "5")
        .timeout(Duration::from_millis(300))
        .build()
        .unwrap();

    let start = Instant::now();
    let result = run_claude_sync(&claude, vec!["--version".to_string()]);
    let elapsed = start.elapsed();

    let err = result.expect_err("expected a timeout error");
    let msg = err.to_string();
    assert!(
        msg.contains("timed out") || msg.contains("timeout"),
        "expected timeout error, got: {msg}"
    );
    assert!(
        elapsed < Duration::from_secs(2),
        "sync timeout should return promptly, took {elapsed:?}"
    );
}

#[test]
fn sync_large_output_does_not_deadlock() {
    // Emit ~256KB of output — larger than any reasonable pipe buffer —
    // so the test catches a naive "wait then read" implementation
    // that would block the child once the pipe fills. The payload is
    // generated inside the child (FAKE_CLAUDE_OUTPUT_BYTES): passing it
    // via the environment would exceed Linux's 128KB per-env-string
    // limit and fail execve with E2BIG.
    let bytes = 256 * 1024;
    let claude = claude_with_env(&[("FAKE_CLAUDE_OUTPUT_BYTES", &bytes.to_string())]);

    let output = run_claude_sync(&claude, vec!["--version".to_string()])
        .expect("sync execution with large output should succeed");

    assert!(output.success);
    assert!(output.stdout.len() >= bytes);
}

#[test]
fn sync_large_output_does_not_deadlock_with_timeout() {
    // Same as above but routed through the run_with_timeout_sync path
    // (i.e. a Claude with a timeout set, even though we finish well
    // under it). This is the one that proves the concurrent-drain
    // threads in the sync timeout code path actually work.
    let bytes = 256 * 1024;
    let claude = Claude::builder()
        .binary(fake_binary())
        .env("FAKE_CLAUDE_OUTPUT_BYTES", bytes.to_string())
        .timeout(Duration::from_secs(10))
        .build()
        .unwrap();

    let output = run_claude_sync(&claude, vec!["--version".to_string()])
        .expect("sync timeout-path execution with large output should succeed");

    assert!(output.success);
    assert!(output.stdout.len() >= bytes);
}

// ── command-surface tests ─────────────────────────────────────────

#[test]
fn sync_version_command_via_blanket_trait() {
    use claude_wrapper::{ClaudeCommandSyncExt, VersionCommand};

    let claude = claude_with_env(&[("FAKE_CLAUDE_OUTPUT", "1.2.3 (Claude Code)")]);
    let output = VersionCommand::new()
        .execute_sync(&claude)
        .expect("VersionCommand::execute_sync should succeed");
    assert!(output.success);
    assert!(output.stdout.contains("1.2.3"));
}

#[test]
fn sync_claude_cli_version_helper() {
    use claude_wrapper::CliVersion;

    let claude = claude_with_env(&[("FAKE_CLAUDE_OUTPUT", "1.2.3 (Claude Code)")]);
    let version = claude
        .cli_version_sync()
        .expect("cli_version_sync should succeed");
    assert_eq!(version, CliVersion::new(1, 2, 3));
}

#[test]
fn sync_claude_check_version_helper() {
    use claude_wrapper::CliVersion;

    let claude = claude_with_env(&[("FAKE_CLAUDE_OUTPUT", "2.0.0 (Claude Code)")]);
    let v = claude
        .check_version_sync(&CliVersion::new(1, 0, 0))
        .expect("check_version_sync should satisfy minimum");
    assert_eq!(v, CliVersion::new(2, 0, 0));

    let err = claude
        .check_version_sync(&CliVersion::new(3, 0, 0))
        .expect_err("check_version_sync should reject too-low version");
    assert!(
        err.to_string()
            .contains("does not meet minimum requirement")
    );
}

#[cfg(feature = "json")]
#[test]
fn sync_query_execute_json_parses_result() {
    use claude_wrapper::QueryCommand;

    let claude = claude_with_env(&[
        ("FAKE_CLAUDE_OUTPUT", "42"),
        ("FAKE_CLAUDE_SESSION_ID", "sync-sess"),
    ]);

    let result = QueryCommand::new("what is 2+2?")
        .no_session_persistence()
        .execute_json_sync(&claude)
        .expect("execute_json_sync should succeed");

    assert_eq!(result.result, "42");
    assert_eq!(result.session_id, "sync-sess");
}

#[cfg(feature = "json")]
#[test]
fn sync_query_execute_retries_via_policy() {
    // Sync execute_sync on QueryCommand must go through the retry-aware
    // helper. We exercise that path indirectly: set a policy and let
    // a successful call go through without errors.
    use claude_wrapper::{QueryCommand, RetryPolicy};
    use std::time::Duration;

    let claude = claude_with_env(&[("FAKE_CLAUDE_OUTPUT", "ok")]);
    let cmd = QueryCommand::new("ping").no_session_persistence().retry(
        RetryPolicy::new()
            .max_attempts(2)
            .initial_backoff(Duration::from_millis(1))
            .retry_on_timeout(true),
    );

    let out = cmd
        .execute_sync(&claude)
        .expect("sync query with retry policy should succeed");
    assert!(out.success);
    assert!(out.stdout.contains("ok"));
}

// ── sync streaming tests ─────────────────────────────────────────

#[cfg(feature = "json")]
#[test]
fn sync_stream_emits_three_event_types() {
    use claude_wrapper::streaming::{StreamEvent, stream_query_sync};
    use claude_wrapper::{OutputFormat, QueryCommand};

    let claude = claude_with_env(&[("FAKE_CLAUDE_OUTPUT", "streamed sync")]);
    let cmd = QueryCommand::new("test prompt")
        .output_format(OutputFormat::StreamJson)
        .no_session_persistence();

    let mut events: Vec<StreamEvent> = Vec::new();
    stream_query_sync(&claude, &cmd, |event| events.push(event))
        .expect("sync streaming should succeed");

    // Fake binary emits exactly three NDJSON lines: system, assistant,
    // result. Verify all three surface in order.
    assert_eq!(events.len(), 3, "expected 3 events, got {}", events.len());
    assert_eq!(events[0].event_type(), Some("system"));
    assert_eq!(events[1].event_type(), Some("assistant"));
    assert_eq!(events[2].event_type(), Some("result"));
}

#[cfg(feature = "json")]
#[test]
fn sync_stream_result_event_carries_metadata() {
    use claude_wrapper::streaming::{StreamEvent, stream_query_sync};
    use claude_wrapper::{OutputFormat, QueryCommand};

    let claude = claude_with_env(&[
        ("FAKE_CLAUDE_OUTPUT", "sync output"),
        ("FAKE_CLAUDE_SESSION_ID", "sync-stream-123"),
    ]);
    let cmd = QueryCommand::new("test")
        .output_format(OutputFormat::StreamJson)
        .no_session_persistence();

    let mut result_event: Option<StreamEvent> = None;
    stream_query_sync(&claude, &cmd, |event| {
        if event.is_result() {
            result_event = Some(event);
        }
    })
    .expect("sync streaming should succeed");

    let result = result_event.expect("should have received a result event");
    assert_eq!(result.session_id(), Some("sync-stream-123"));
    assert_eq!(result.result_text(), Some("sync output"));
    assert_eq!(result.cost_usd(), Some(0.0));
}

#[cfg(feature = "json")]
#[test]
fn sync_stream_stdout_auth_error_preserves_only_diagnostics() {
    use claude_wrapper::auth::AuthErrorKind;
    use claude_wrapper::streaming::{StreamEvent, stream_query_sync};
    use claude_wrapper::{Error, OutputFormat, QueryCommand};

    let claude = claude_with_env(&[("FAKE_CLAUDE_STDOUT_AUTH_ERROR", "1")]);
    let cmd = QueryCommand::new("test prompt")
        .output_format(OutputFormat::StreamJson)
        .no_session_persistence();

    let mut events: Vec<StreamEvent> = Vec::new();
    let error = stream_query_sync(&claude, &cmd, |event| events.push(event))
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

#[cfg(feature = "json")]
#[test]
fn sync_stream_timeout_fires_and_returns_promptly() {
    use claude_wrapper::streaming::stream_query_sync;
    use claude_wrapper::{OutputFormat, QueryCommand};

    // Fake child sleeps 5s before emitting anything.
    let claude = Claude::builder()
        .binary(fake_binary())
        .env("FAKE_CLAUDE_DELAY", "5")
        .env("FAKE_CLAUDE_OUTPUT", "unreachable")
        .timeout(Duration::from_millis(300))
        .build()
        .unwrap();

    let cmd = QueryCommand::new("test")
        .output_format(OutputFormat::StreamJson)
        .no_session_persistence();

    let start = Instant::now();
    let result = stream_query_sync(&claude, &cmd, |_| {});
    let elapsed = start.elapsed();

    let err = result.expect_err("expected a timeout error");
    let msg = err.to_string();
    assert!(
        msg.contains("timed out") || msg.contains("timeout"),
        "expected timeout error, got: {msg}"
    );
    assert!(
        elapsed < Duration::from_secs(2),
        "sync streaming timeout should return promptly, took {elapsed:?}"
    );
}

#[cfg(feature = "json")]
#[test]
fn sync_stream_handler_runs_on_caller_thread_non_send_state() {
    use claude_wrapper::streaming::stream_query_sync;
    use claude_wrapper::{OutputFormat, QueryCommand};
    use std::rc::Rc;

    // Rc<RefCell<_>> is !Send, so this compiling and running is the
    // proof that the handler isn't forced onto a worker thread.
    let counter = Rc::new(std::cell::RefCell::new(0usize));
    let counter_clone = Rc::clone(&counter);

    let claude = claude_with_env(&[("FAKE_CLAUDE_OUTPUT", "bump")]);
    let cmd = QueryCommand::new("test")
        .output_format(OutputFormat::StreamJson)
        .no_session_persistence();

    stream_query_sync(&claude, &cmd, move |_| {
        *counter_clone.borrow_mut() += 1;
    })
    .expect("sync streaming should succeed");

    assert_eq!(*counter.borrow(), 3);
}
