//! Test helpers for claude-pool integration tests.

#![allow(dead_code)]

use std::path::{Path, PathBuf};
use std::process::Command;

/// Path to the fake-claude.sh script relative to the workspace root.
pub const FAKE_CLAUDE_SCRIPT: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../test-helpers/fake-claude.sh"
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
