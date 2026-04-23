//! Integration tests for the blocking/sync exec path.
//!
//! These cover the primitives in `exec::*_sync` against the fake-claude
//! shell script. The sync command surface (per-builder `execute_sync`)
//! is landing in a follow-up PR, so these tests reach into the exec
//! module directly.

#![cfg(feature = "sync")]

use std::path::PathBuf;
use std::time::{Duration, Instant};

use claude_wrapper::Claude;
use claude_wrapper::exec::run_claude_sync;

const FAKE_CLAUDE: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../test-helpers/fake-claude.sh"
);

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
    // that would block the child once the pipe fills.
    let big = "x".repeat(256 * 1024);
    let claude = claude_with_env(&[("FAKE_CLAUDE_OUTPUT", &big)]);

    let output = run_claude_sync(&claude, vec!["--version".to_string()])
        .expect("sync execution with large output should succeed");

    assert!(output.success);
    assert!(output.stdout.len() >= big.len());
}

#[test]
fn sync_large_output_does_not_deadlock_with_timeout() {
    // Same as above but routed through the run_with_timeout_sync path
    // (i.e. a Claude with a timeout set, even though we finish well
    // under it). This is the one that proves the concurrent-drain
    // threads in the sync timeout code path actually work.
    let big = "x".repeat(256 * 1024);
    let claude = Claude::builder()
        .binary(fake_binary())
        .env("FAKE_CLAUDE_OUTPUT", &big)
        .timeout(Duration::from_secs(10))
        .build()
        .unwrap();

    let output = run_claude_sync(&claude, vec!["--version".to_string()])
        .expect("sync timeout-path execution with large output should succeed");

    assert!(output.success);
    assert!(output.stdout.len() >= big.len());
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
