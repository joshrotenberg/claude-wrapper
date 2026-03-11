//! Test helpers for claude-pool integration tests.

#![allow(dead_code)]

use std::path::{Path, PathBuf};
use std::process::Command;

/// Path to the fake-claude.sh script relative to the workspace root.
pub const FAKE_CLAUDE_SCRIPT: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../test-helpers/fake-claude.sh"
);

/// Return the path to the fake-claude binary.
pub fn fake_claude_path() -> PathBuf {
    PathBuf::from(FAKE_CLAUDE_SCRIPT)
}

/// Create a temporary git repository.
///
/// Initialises a bare repo with a single empty commit so that worktree
/// operations have a HEAD to branch from.
pub fn temp_git_repo() -> tempfile::TempDir {
    let dir = tempfile::tempdir().expect("failed to create temp dir");

    let run = |args: &[&str]| {
        let status = Command::new("git")
            .args(args)
            .current_dir(dir.path())
            .status()
            .expect("git command failed");
        assert!(status.success(), "git {args:?} failed");
    };

    run(&["init"]);
    run(&["config", "user.email", "test@example.com"]);
    run(&["config", "user.name", "Test"]);

    // Create an initial commit so HEAD exists.
    let file = dir.path().join("README.md");
    std::fs::write(&file, "# test repo\n").expect("write failed");
    run(&["add", "."]);
    run(&["commit", "-m", "init"]);

    dir
}

/// Build a [`claude_wrapper::Claude`] client that points at the fake-claude binary.
pub fn claude_with_fake_binary(fake_binary: &Path) -> claude_wrapper::Claude {
    claude_wrapper::Claude::builder()
        .binary(fake_binary)
        .build()
        .expect("failed to build Claude client")
}

/// Build a [`claude_wrapper::Claude`] client with env vars injected.
///
/// Passes env vars via the builder (safe for Rust 2024 — no `set_var`).
pub fn claude_with_fake_binary_env(
    fake_binary: &Path,
    env: &[(&str, &str)],
) -> claude_wrapper::Claude {
    let mut builder = claude_wrapper::Claude::builder().binary(fake_binary);
    for (k, v) in env {
        builder = builder.env(*k, *v);
    }
    builder.build().expect("failed to build Claude client")
}

/// Write a temporary wrapper shell script that sets env vars before exec-ing the real binary.
///
/// Returns the temp file (keep it alive for the duration of the test).
/// This approach avoids `std::env::set_var` (unsafe in Rust 2024).
pub fn write_env_wrapper(env: &[(&str, &str)], target: &Path) -> tempfile::NamedTempFile {
    use std::fmt::Write as _;
    use std::os::unix::fs::PermissionsExt;

    let tmp = tempfile::NamedTempFile::new().expect("temp file");
    let mut script = String::from("#!/bin/sh\n");
    for (k, v) in env {
        // Shell-escape the value: wrap in single quotes, escape embedded single quotes.
        let escaped = v.replace('\'', "'\\''");
        write!(script, "{}='{}' ", k, escaped).unwrap();
    }
    writeln!(script, "exec {} \"$@\"", target.display()).unwrap();

    std::fs::write(tmp.path(), &script).expect("write wrapper");
    std::fs::set_permissions(tmp.path(), std::fs::Permissions::from_mode(0o755))
        .expect("chmod wrapper");

    tmp
}
