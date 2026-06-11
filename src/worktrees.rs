//! Read-side introspection for git worktrees.
//!
//! Claude Code's `--worktree [name]` flag (and its duplex equivalent
//! [`crate::duplex::DuplexOptions::worktree`]) creates fresh git
//! worktrees so an agent's writes can land somewhere other than the
//! current working tree. Hosts that orchestrate worktree-isolated
//! chats need a way to enumerate the worktrees that exist for a
//! given repo, see what branches they're on, and notice when one is
//! locked or prunable.
//!
//! This module is a thin Rust API over `git worktree list
//! --porcelain`. It is read-only on purpose; mutations (pruning,
//! removing worktrees) are tracked separately so consumers that
//! only want to introspect don't opt into write semantics.
//!
//! # Why shell out
//!
//! Reading `.git/worktrees/` directly is brittle: git tracks
//! `locked`, `prunable`, detached-HEAD, and bare-repo state in ways
//! that have evolved across releases. Asking git itself is the
//! cheap, correct option, and `git` is already a transitive
//! dependency of any worktree-using workflow.
//!
//! # Example
//!
//! ```no_run
//! use claude_wrapper::worktrees::WorktreeRoot;
//!
//! # fn example() -> claude_wrapper::Result<()> {
//! let root = WorktreeRoot::for_repo(".");
//! for wt in root.list()? {
//!     println!(
//!         "{}  {}  {}{}",
//!         wt.path.display(),
//!         wt.branch.as_deref().unwrap_or("(detached)"),
//!         wt.head.as_deref().unwrap_or(""),
//!         if wt.is_locked { "  [locked]" } else { "" },
//!     );
//! }
//! # Ok(()) }
//! ```

use std::path::{Path, PathBuf};
use std::process::Command;

use serde::Serialize;

use crate::error::{Error, Result};

/// Entry point for listing git worktrees rooted at a repository.
/// Construct with [`Self::for_repo`] pointing at any path inside the
/// repo (typically the repo root); git resolves the actual `.git`
/// from there.
#[derive(Debug, Clone)]
pub struct WorktreeRoot {
    repo_path: PathBuf,
}

impl WorktreeRoot {
    /// Address worktrees for the repository containing `path`.
    ///
    /// `path` can be any directory inside the repo; git's `-C`
    /// handling resolves the `.git` itself. Doesn't validate that
    /// `path` is in fact a git repo until you call [`Self::list`].
    pub fn for_repo(path: impl Into<PathBuf>) -> Self {
        Self {
            repo_path: path.into(),
        }
    }

    /// The configured repository path.
    pub fn path(&self) -> &Path {
        &self.repo_path
    }

    /// List every worktree git knows about for this repository.
    ///
    /// Spawns `git -C <repo_path> worktree list --porcelain` and
    /// parses the output. The first entry is the main worktree.
    /// Errors if `git` isn't on PATH, the path isn't a git repo, or
    /// the porcelain output is malformed.
    pub fn list(&self) -> Result<Vec<Worktree>> {
        let output = Command::new("git")
            .arg("-C")
            .arg(&self.repo_path)
            .arg("worktree")
            .arg("list")
            .arg("--porcelain")
            .output()
            .map_err(|e| Error::Worktrees {
                message: format!("failed to spawn git: {e}"),
            })?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(Error::Worktrees {
                message: format!(
                    "git worktree list failed (exit {}): {}",
                    output.status.code().unwrap_or(-1),
                    stderr.trim()
                ),
            });
        }

        let stdout = String::from_utf8_lossy(&output.stdout);
        Ok(parse_porcelain(&stdout))
    }
}

/// One git worktree as reported by `git worktree list --porcelain`.
#[derive(Debug, Clone, Serialize)]
pub struct Worktree {
    /// Absolute path to the worktree directory.
    pub path: PathBuf,
    /// Commit SHA at HEAD. None for bare worktrees.
    pub head: Option<String>,
    /// Branch name without `refs/heads/` prefix. None when the
    /// worktree has detached HEAD (or is bare).
    pub branch: Option<String>,
    /// True for the primary worktree (the one git considers the
    /// "main" worktree). Always the first in the list returned by
    /// [`WorktreeRoot::list`].
    pub is_main: bool,
    /// True if HEAD is detached (no branch checked out).
    pub is_detached: bool,
    /// True for a bare repository worktree.
    pub is_bare: bool,
    /// True if `git worktree lock` has been applied.
    pub is_locked: bool,
    /// Optional human-readable lock reason if `is_locked`.
    pub lock_reason: Option<String>,
    /// True if git considers this worktree prunable (the directory
    /// is missing or the lock file is stale).
    pub is_prunable: bool,
    /// Optional human-readable prunable reason if `is_prunable`.
    pub prune_reason: Option<String>,
}

fn parse_porcelain(input: &str) -> Vec<Worktree> {
    let mut out = Vec::new();
    let mut current: Option<WorktreeBuilder> = None;
    let mut is_first = true;

    for line in input.lines() {
        let line = line.trim_end_matches('\r');

        if line.is_empty() {
            if let Some(b) = current.take() {
                let mut wt = b.build();
                if is_first {
                    wt.is_main = true;
                    is_first = false;
                }
                out.push(wt);
            }
            continue;
        }

        let (key, value) = match line.split_once(' ') {
            Some((k, v)) => (k, Some(v)),
            None => (line, None),
        };

        match key {
            "worktree" => {
                // New block; flush any pending one.
                if let Some(b) = current.take() {
                    let mut wt = b.build();
                    if is_first {
                        wt.is_main = true;
                        is_first = false;
                    }
                    out.push(wt);
                }
                current = Some(WorktreeBuilder::new(
                    value.map(PathBuf::from).unwrap_or_default(),
                ));
            }
            "HEAD" => {
                if let Some(b) = current.as_mut() {
                    b.head = value.map(str::to_string);
                }
            }
            "branch" => {
                if let Some(b) = current.as_mut() {
                    b.branch = value.map(strip_branch_prefix);
                }
            }
            "detached" => {
                if let Some(b) = current.as_mut() {
                    b.is_detached = true;
                }
            }
            "bare" => {
                if let Some(b) = current.as_mut() {
                    b.is_bare = true;
                }
            }
            "locked" => {
                if let Some(b) = current.as_mut() {
                    b.is_locked = true;
                    b.lock_reason = value.map(str::to_string).filter(|s| !s.is_empty());
                }
            }
            "prunable" => {
                if let Some(b) = current.as_mut() {
                    b.is_prunable = true;
                    b.prune_reason = value.map(str::to_string).filter(|s| !s.is_empty());
                }
            }
            _ => {
                // Unknown key -- skip. Keeps the parser
                // forward-compatible with future porcelain fields.
            }
        }
    }

    // Trailing block without a closing blank line.
    if let Some(b) = current.take() {
        let mut wt = b.build();
        if is_first {
            wt.is_main = true;
        }
        out.push(wt);
    }

    out
}

fn strip_branch_prefix(branch: &str) -> String {
    branch
        .strip_prefix("refs/heads/")
        .unwrap_or(branch)
        .to_string()
}

#[derive(Debug)]
struct WorktreeBuilder {
    path: PathBuf,
    head: Option<String>,
    branch: Option<String>,
    is_detached: bool,
    is_bare: bool,
    is_locked: bool,
    lock_reason: Option<String>,
    is_prunable: bool,
    prune_reason: Option<String>,
}

impl WorktreeBuilder {
    fn new(path: PathBuf) -> Self {
        Self {
            path,
            head: None,
            branch: None,
            is_detached: false,
            is_bare: false,
            is_locked: false,
            lock_reason: None,
            is_prunable: false,
            prune_reason: None,
        }
    }

    fn build(self) -> Worktree {
        Worktree {
            path: self.path,
            head: self.head,
            branch: self.branch,
            is_main: false, // set by caller when applicable
            is_detached: self.is_detached,
            is_bare: self.is_bare,
            is_locked: self.is_locked,
            lock_reason: self.lock_reason,
            is_prunable: self.is_prunable,
            prune_reason: self.prune_reason,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_single_main_worktree() {
        let raw = "\
worktree /repo/main
HEAD abc123
branch refs/heads/main
";
        let out = parse_porcelain(raw);
        assert_eq!(out.len(), 1);
        let wt = &out[0];
        assert_eq!(wt.path, PathBuf::from("/repo/main"));
        assert_eq!(wt.head.as_deref(), Some("abc123"));
        assert_eq!(wt.branch.as_deref(), Some("main"));
        assert!(wt.is_main);
        assert!(!wt.is_detached);
        assert!(!wt.is_bare);
        assert!(!wt.is_locked);
        assert!(!wt.is_prunable);
    }

    #[test]
    fn parse_multiple_worktrees_marks_first_as_main() {
        let raw = "\
worktree /repo/main
HEAD aaa
branch refs/heads/main

worktree /repo/feature-x
HEAD bbb
branch refs/heads/feature-x

worktree /repo/feature-y
HEAD ccc
branch refs/heads/feature-y
";
        let out = parse_porcelain(raw);
        assert_eq!(out.len(), 3);
        assert!(out[0].is_main);
        assert!(!out[1].is_main);
        assert!(!out[2].is_main);
        assert_eq!(out[0].branch.as_deref(), Some("main"));
        assert_eq!(out[1].branch.as_deref(), Some("feature-x"));
        assert_eq!(out[2].branch.as_deref(), Some("feature-y"));
    }

    #[test]
    fn parse_detached_head() {
        let raw = "\
worktree /repo/main
HEAD aaa
branch refs/heads/main

worktree /repo/poking
HEAD ddd
detached
";
        let out = parse_porcelain(raw);
        assert_eq!(out.len(), 2);
        assert!(out[1].is_detached);
        assert!(out[1].branch.is_none());
        assert_eq!(out[1].head.as_deref(), Some("ddd"));
    }

    #[test]
    fn parse_bare_worktree() {
        let raw = "\
worktree /repo/bare
bare
";
        let out = parse_porcelain(raw);
        assert_eq!(out.len(), 1);
        assert!(out[0].is_bare);
        assert!(out[0].head.is_none());
        assert!(out[0].branch.is_none());
    }

    #[test]
    fn parse_locked_with_reason() {
        let raw = "\
worktree /repo/main
HEAD aaa
branch refs/heads/main

worktree /repo/release-prep
HEAD bbb
branch refs/heads/release-prep
locked Cutting v2.0
";
        let out = parse_porcelain(raw);
        assert_eq!(out.len(), 2);
        assert!(out[1].is_locked);
        assert_eq!(out[1].lock_reason.as_deref(), Some("Cutting v2.0"));
    }

    #[test]
    fn parse_locked_without_reason() {
        let raw = "\
worktree /repo/main
HEAD aaa
branch refs/heads/main

worktree /repo/wedged
HEAD bbb
branch refs/heads/wedged
locked
";
        let out = parse_porcelain(raw);
        assert_eq!(out.len(), 2);
        assert!(out[1].is_locked);
        assert!(out[1].lock_reason.is_none());
    }

    #[test]
    fn parse_prunable_with_reason() {
        let raw = "\
worktree /repo/main
HEAD aaa
branch refs/heads/main

worktree /repo/gone
HEAD bbb
branch refs/heads/gone
prunable gitdir file points to non-existent location
";
        let out = parse_porcelain(raw);
        assert_eq!(out.len(), 2);
        assert!(out[1].is_prunable);
        assert!(
            out[1]
                .prune_reason
                .as_deref()
                .unwrap_or("")
                .contains("non-existent")
        );
    }

    #[test]
    fn parse_handles_trailing_block_without_blank_line() {
        let raw = "\
worktree /repo/main
HEAD aaa
branch refs/heads/main";
        let out = parse_porcelain(raw);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].path, PathBuf::from("/repo/main"));
    }

    #[test]
    fn parse_strips_refs_heads_prefix() {
        let raw = "\
worktree /repo/x
HEAD aaa
branch refs/heads/feature/long/path
";
        let out = parse_porcelain(raw);
        assert_eq!(out[0].branch.as_deref(), Some("feature/long/path"));
    }

    #[test]
    fn parse_unknown_keys_are_ignored() {
        // Forward-compat: future porcelain fields shouldn't break us.
        let raw = "\
worktree /repo/main
HEAD aaa
branch refs/heads/main
some-future-field who-knows
";
        let out = parse_porcelain(raw);
        assert_eq!(out.len(), 1);
        assert!(out[0].is_main);
    }

    #[test]
    fn parse_empty_input_returns_empty() {
        assert!(parse_porcelain("").is_empty());
        assert!(parse_porcelain("\n\n\n").is_empty());
    }

    // -- live test against a real git binary ------------------------

    #[test]
    fn live_lists_at_least_the_main_worktree() {
        // This test runs against the repo it lives in -- always at
        // least the main worktree exists.
        let root = WorktreeRoot::for_repo(env!("CARGO_MANIFEST_DIR"));
        let wts = root.list().expect("git worktree list should work");
        assert!(!wts.is_empty(), "expected at least the main worktree");
        assert!(wts[0].is_main);
        assert!(!wts[0].path.as_os_str().is_empty());
    }

    #[test]
    fn list_errors_on_non_git_path() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let root = WorktreeRoot::for_repo(tmp.path());
        let err = root.list().unwrap_err();
        assert!(
            err.to_string().to_lowercase().contains("worktree")
                || err.to_string().to_lowercase().contains("git"),
            "unexpected error: {err}"
        );
    }
}
