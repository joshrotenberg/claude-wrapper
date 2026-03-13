//! Git worktree isolation for parallel slots.
//!
//! When multiple slots operate on the same repository, they need
//! isolated working directories to avoid stepping on each other's
//! git state. This module manages git worktree creation and cleanup.

use std::path::{Path, PathBuf};

use crate::error::{Error, Result};
use crate::types::{SlotId, TaskId};

/// Manages git worktrees for pool slots.
///
/// When dropped, the manager attempts best-effort cleanup of all worktrees
/// under its base directory. This ensures stale worktrees are removed even
/// if the pool panics or exits without calling [`cleanup_all`][Self::cleanup_all].
#[derive(Debug)]
pub struct WorktreeManager {
    /// Root directory for worktrees (e.g. `/tmp/claude-pool/worktrees`).
    base_dir: PathBuf,
    /// Source repository path.
    repo_dir: PathBuf,
    /// Slot IDs currently tracked (for cleanup on drop).
    tracked_slots: std::sync::Mutex<Vec<SlotId>>,
    /// Chain task IDs currently tracked (for cleanup on drop).
    tracked_chains: std::sync::Mutex<Vec<TaskId>>,
}

impl WorktreeManager {
    /// Create a new worktree manager.
    ///
    /// - `repo_dir`: The source repository to create worktrees from.
    /// - `base_dir`: Directory where worktrees will be created. If `None`,
    ///   uses a temp directory under the system temp dir.
    pub fn new(repo_dir: impl Into<PathBuf>, base_dir: Option<PathBuf>) -> Self {
        let repo_dir = repo_dir.into();
        let base_dir =
            base_dir.unwrap_or_else(|| std::env::temp_dir().join("claude-pool").join("worktrees"));
        Self {
            base_dir,
            repo_dir,
            tracked_slots: std::sync::Mutex::new(Vec::new()),
            tracked_chains: std::sync::Mutex::new(Vec::new()),
        }
    }

    /// Create a worktree manager after verifying the repo directory is a git repository.
    ///
    /// Returns an error if `repo_dir` is not inside a git working tree.
    pub async fn new_validated(
        repo_dir: impl Into<PathBuf>,
        base_dir: Option<PathBuf>,
    ) -> Result<Self> {
        let repo_dir = repo_dir.into();
        let output = tokio::process::Command::new("git")
            .args(["rev-parse", "--is-inside-work-tree"])
            .current_dir(&repo_dir)
            .output()
            .await
            .map_err(|e| {
                Error::Store(format!(
                    "failed to check git repo at {}: {e}",
                    repo_dir.display()
                ))
            })?;

        if !output.status.success() {
            return Err(Error::Store(format!(
                "worktree isolation requires a git repository, but {} is not inside a git work tree",
                repo_dir.display()
            )));
        }

        Ok(Self::new(repo_dir, base_dir))
    }

    /// Track a slot ID for cleanup on drop.
    fn track_slot(&self, slot_id: &SlotId) {
        if let Ok(mut slots) = self.tracked_slots.lock()
            && !slots.iter().any(|s| s.0 == slot_id.0)
        {
            slots.push(slot_id.clone());
        }
    }

    /// Untrack a slot ID (after explicit removal).
    fn untrack_slot(&self, slot_id: &SlotId) {
        if let Ok(mut slots) = self.tracked_slots.lock() {
            slots.retain(|s| s.0 != slot_id.0);
        }
    }

    /// Track a chain task ID for cleanup on drop.
    fn track_chain(&self, task_id: &TaskId) {
        if let Ok(mut chains) = self.tracked_chains.lock()
            && !chains.iter().any(|t| t.0 == task_id.0)
        {
            chains.push(task_id.clone());
        }
    }

    /// Untrack a chain task ID (after explicit removal).
    fn untrack_chain(&self, task_id: &TaskId) {
        if let Ok(mut chains) = self.tracked_chains.lock() {
            chains.retain(|t| t.0 != task_id.0);
        }
    }

    /// Create a worktree for a slot.
    ///
    /// Creates a git worktree at `{base_dir}/{slot_id}` branched from
    /// the current HEAD.
    pub async fn create(&self, slot_id: &SlotId) -> Result<PathBuf> {
        let worktree_path = self.base_dir.join(&slot_id.0);

        // Ensure base directory exists.
        tokio::fs::create_dir_all(&self.base_dir)
            .await
            .map_err(|e| Error::Store(format!("failed to create worktree base dir: {e}")))?;

        // Remove existing worktree if it exists (stale from previous run).
        if worktree_path.exists() {
            self.remove(slot_id).await?;
        }

        let branch_name = format!("claude-pool/{}", slot_id.0);
        let output = tokio::process::Command::new("git")
            .args([
                "worktree",
                "add",
                "-b",
                &branch_name,
                worktree_path.to_str().unwrap_or_default(),
                "HEAD",
            ])
            .current_dir(&self.repo_dir)
            .output()
            .await
            .map_err(|e| Error::Store(format!("failed to create git worktree: {e}")))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(Error::Store(format!("git worktree add failed: {stderr}")));
        }

        self.track_slot(slot_id);

        tracing::info!(
            slot_id = %slot_id.0,
            path = %worktree_path.display(),
            "created git worktree"
        );

        Ok(worktree_path)
    }

    /// Remove a slot's worktree and its branch.
    pub async fn remove(&self, slot_id: &SlotId) -> Result<()> {
        let worktree_path = self.base_dir.join(&slot_id.0);

        if worktree_path.exists() {
            let output = tokio::process::Command::new("git")
                .args([
                    "worktree",
                    "remove",
                    "--force",
                    worktree_path.to_str().unwrap_or_default(),
                ])
                .current_dir(&self.repo_dir)
                .output()
                .await
                .map_err(|e| Error::Store(format!("failed to remove git worktree: {e}")))?;

            if !output.status.success() {
                let stderr = String::from_utf8_lossy(&output.stderr);
                tracing::warn!(
                    slot_id = %slot_id.0,
                    error = %stderr,
                    "failed to remove worktree, cleaning up manually"
                );
                // Fall back to manual removal.
                let _ = tokio::fs::remove_dir_all(&worktree_path).await;
            }
        }

        // Clean up the branch.
        let branch_name = format!("claude-pool/{}", slot_id.0);
        let _ = tokio::process::Command::new("git")
            .args(["branch", "-D", &branch_name])
            .current_dir(&self.repo_dir)
            .output()
            .await;

        self.untrack_slot(slot_id);

        tracing::debug!(
            slot_id = %slot_id.0,
            "removed git worktree"
        );

        Ok(())
    }

    /// Remove all worktrees managed by this pool.
    pub async fn cleanup_all(&self, slot_ids: &[SlotId]) -> Result<()> {
        for id in slot_ids {
            self.remove(id).await?;
        }

        // Prune stale worktree references.
        let _ = tokio::process::Command::new("git")
            .args(["worktree", "prune"])
            .current_dir(&self.repo_dir)
            .output()
            .await;

        Ok(())
    }

    /// Get the worktree path for a slot (may not exist yet).
    pub fn worktree_path(&self, slot_id: &SlotId) -> PathBuf {
        self.base_dir.join(&slot_id.0)
    }

    /// Get the base directory for all worktrees.
    pub fn base_dir(&self) -> &Path {
        &self.base_dir
    }

    /// Get the source repository directory.
    pub fn repo_dir(&self) -> &Path {
        &self.repo_dir
    }

    /// Create a worktree for a chain execution.
    ///
    /// Creates a git worktree at `{base_dir}/chains/{task_id}` branched from
    /// the current HEAD.
    pub async fn create_for_chain(&self, task_id: &TaskId) -> Result<PathBuf> {
        let worktree_path = self.chain_worktree_path(task_id);

        // Ensure chains directory exists.
        let chains_dir = self.base_dir.join("chains");
        tokio::fs::create_dir_all(&chains_dir)
            .await
            .map_err(|e| Error::Store(format!("failed to create chains dir: {e}")))?;

        // Remove existing worktree if it exists (stale from previous run).
        if worktree_path.exists() {
            self.remove_chain(task_id).await?;
        }

        let branch_name = format!("claude-pool/chain/{}", task_id.0);
        let output = tokio::process::Command::new("git")
            .args([
                "worktree",
                "add",
                "-b",
                &branch_name,
                worktree_path.to_str().unwrap_or_default(),
                "HEAD",
            ])
            .current_dir(&self.repo_dir)
            .output()
            .await
            .map_err(|e| Error::Store(format!("failed to create chain worktree: {e}")))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(Error::Store(format!(
                "git worktree add failed for chain: {stderr}"
            )));
        }

        self.track_chain(task_id);

        tracing::info!(
            task_id = %task_id.0,
            path = %worktree_path.display(),
            "created chain worktree"
        );

        Ok(worktree_path)
    }

    /// Remove a chain's worktree and its branch.
    pub async fn remove_chain(&self, task_id: &TaskId) -> Result<()> {
        let worktree_path = self.chain_worktree_path(task_id);

        if worktree_path.exists() {
            let output = tokio::process::Command::new("git")
                .args([
                    "worktree",
                    "remove",
                    "--force",
                    worktree_path.to_str().unwrap_or_default(),
                ])
                .current_dir(&self.repo_dir)
                .output()
                .await
                .map_err(|e| Error::Store(format!("failed to remove chain worktree: {e}")))?;

            if !output.status.success() {
                let stderr = String::from_utf8_lossy(&output.stderr);
                tracing::warn!(
                    task_id = %task_id.0,
                    error = %stderr,
                    "failed to remove chain worktree, cleaning up manually"
                );
                let _ = tokio::fs::remove_dir_all(&worktree_path).await;
            }
        }

        // Clean up the branch.
        let branch_name = format!("claude-pool/chain/{}", task_id.0);
        let _ = tokio::process::Command::new("git")
            .args(["branch", "-D", &branch_name])
            .current_dir(&self.repo_dir)
            .output()
            .await;

        self.untrack_chain(task_id);

        tracing::debug!(
            task_id = %task_id.0,
            "removed chain worktree"
        );

        Ok(())
    }

    /// Get the worktree path for a chain (may not exist yet).
    pub fn chain_worktree_path(&self, task_id: &TaskId) -> PathBuf {
        self.base_dir.join("chains").join(&task_id.0)
    }

    /// Create a full clone for a chain execution using `git clone --local --shared`.
    ///
    /// Creates a clone at `{base_dir}/clones/{task_id}` with no shared .git directory.
    pub async fn create_clone_for_chain(&self, task_id: &TaskId) -> Result<PathBuf> {
        let clone_path = self.clone_path(task_id);

        let clones_dir = self.base_dir.join("clones");
        tokio::fs::create_dir_all(&clones_dir)
            .await
            .map_err(|e| Error::Store(format!("failed to create clones dir: {e}")))?;

        if clone_path.exists() {
            self.remove_clone(task_id).await?;
        }

        let output = tokio::process::Command::new("git")
            .args([
                "clone",
                "--local",
                "--shared",
                self.repo_dir.to_str().unwrap_or_default(),
                clone_path.to_str().unwrap_or_default(),
            ])
            .output()
            .await
            .map_err(|e| Error::Store(format!("failed to create chain clone: {e}")))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(Error::Store(format!(
                "git clone failed for chain: {stderr}"
            )));
        }

        // Preserve the GitHub remote URL from the source repo so that
        // tools like `gh pr create` work inside the clone (#152).
        let remote_output = tokio::process::Command::new("git")
            .args(["remote", "get-url", "origin"])
            .current_dir(&self.repo_dir)
            .output()
            .await
            .map_err(|e| Error::Store(format!("failed to get origin URL: {e}")))?;

        if remote_output.status.success() {
            let url = String::from_utf8_lossy(&remote_output.stdout)
                .trim()
                .to_string();
            // Only override if the source has a non-local remote (i.e. GitHub URL).
            if !url.is_empty() && !url.starts_with('/') {
                let set_output = tokio::process::Command::new("git")
                    .args(["remote", "set-url", "origin", &url])
                    .current_dir(&clone_path)
                    .output()
                    .await
                    .map_err(|e| Error::Store(format!("failed to set origin URL in clone: {e}")))?;

                if !set_output.status.success() {
                    let stderr = String::from_utf8_lossy(&set_output.stderr);
                    tracing::warn!(
                        task_id = %task_id.0,
                        error = %stderr,
                        "failed to set origin URL in clone"
                    );
                }
            }
        }

        tracing::info!(
            task_id = %task_id.0,
            path = %clone_path.display(),
            "created chain clone"
        );

        Ok(clone_path)
    }

    /// Remove a chain's clone directory.
    pub async fn remove_clone(&self, task_id: &TaskId) -> Result<()> {
        let clone_path = self.clone_path(task_id);

        if clone_path.exists() {
            tokio::fs::remove_dir_all(&clone_path).await.map_err(|e| {
                Error::Store(format!(
                    "failed to remove chain clone at {}: {e}",
                    clone_path.display()
                ))
            })?;
        }

        tracing::debug!(task_id = %task_id.0, "removed chain clone");
        Ok(())
    }

    /// Get the clone path for a chain (may not exist yet).
    pub fn clone_path(&self, task_id: &TaskId) -> PathBuf {
        self.base_dir.join("clones").join(&task_id.0)
    }
}

impl Drop for WorktreeManager {
    fn drop(&mut self) {
        let slots: Vec<SlotId> = self
            .tracked_slots
            .lock()
            .map(|s| s.clone())
            .unwrap_or_default();
        let chains: Vec<TaskId> = self
            .tracked_chains
            .lock()
            .map(|c| c.clone())
            .unwrap_or_default();

        if slots.is_empty() && chains.is_empty() {
            return;
        }

        tracing::info!(
            slots = slots.len(),
            chains = chains.len(),
            "cleaning up worktrees on drop"
        );

        // Best-effort synchronous cleanup using blocking git commands.
        for slot_id in &slots {
            let worktree_path = self.base_dir.join(&slot_id.0);
            if worktree_path.exists() {
                let _ = std::process::Command::new("git")
                    .args([
                        "worktree",
                        "remove",
                        "--force",
                        worktree_path.to_str().unwrap_or_default(),
                    ])
                    .current_dir(&self.repo_dir)
                    .output();

                // Fall back to manual removal if git worktree remove fails.
                if worktree_path.exists() {
                    let _ = std::fs::remove_dir_all(&worktree_path);
                }
            }
            let branch_name = format!("claude-pool/{}", slot_id.0);
            let _ = std::process::Command::new("git")
                .args(["branch", "-D", &branch_name])
                .current_dir(&self.repo_dir)
                .output();
        }

        for task_id in &chains {
            let worktree_path = self.base_dir.join("chains").join(&task_id.0);
            if worktree_path.exists() {
                let _ = std::process::Command::new("git")
                    .args([
                        "worktree",
                        "remove",
                        "--force",
                        worktree_path.to_str().unwrap_or_default(),
                    ])
                    .current_dir(&self.repo_dir)
                    .output();

                if worktree_path.exists() {
                    let _ = std::fs::remove_dir_all(&worktree_path);
                }
            }
            let branch_name = format!("claude-pool/chain/{}", task_id.0);
            let _ = std::process::Command::new("git")
                .args(["branch", "-D", &branch_name])
                .current_dir(&self.repo_dir)
                .output();
        }

        // Prune stale worktree references.
        let _ = std::process::Command::new("git")
            .args(["worktree", "prune"])
            .current_dir(&self.repo_dir)
            .output();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn new_validated_rejects_non_repo() {
        let tmpdir = tempfile::tempdir().unwrap();
        let result = WorktreeManager::new_validated(tmpdir.path(), None).await;
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("not inside a git work tree"),
            "expected git work tree error, got: {err}"
        );
    }

    #[tokio::test]
    async fn new_validated_accepts_git_repo() {
        let tmpdir = tempfile::tempdir().unwrap();
        std::process::Command::new("git")
            .args(["init"])
            .current_dir(tmpdir.path())
            .output()
            .unwrap();
        let mgr = WorktreeManager::new_validated(tmpdir.path(), None).await;
        assert!(mgr.is_ok());
    }

    #[test]
    fn worktree_path_construction() {
        let mgr = WorktreeManager::new("/repo", Some(PathBuf::from("/tmp/wt")));
        let id = SlotId("slot-0".into());
        assert_eq!(mgr.worktree_path(&id), PathBuf::from("/tmp/wt/slot-0"));
    }

    #[test]
    fn default_base_dir() {
        let mgr = WorktreeManager::new("/repo", None);
        let expected = std::env::temp_dir().join("claude-pool").join("worktrees");
        assert_eq!(mgr.base_dir(), expected);
    }

    #[tokio::test]
    async fn clone_preserves_non_local_remote() {
        // Set up a source repo with a non-local origin.
        let src = tempfile::tempdir().unwrap();
        std::process::Command::new("git")
            .args(["init"])
            .current_dir(src.path())
            .output()
            .unwrap();
        std::process::Command::new("git")
            .args(["remote", "add", "origin", "git@github.com:user/repo.git"])
            .current_dir(src.path())
            .output()
            .unwrap();
        // Need at least one commit for clone to work.
        std::process::Command::new("git")
            .args(["commit", "--allow-empty", "-m", "init"])
            .current_dir(src.path())
            .output()
            .unwrap();

        let base = tempfile::tempdir().unwrap();
        let mgr = WorktreeManager::new(src.path(), Some(base.path().to_path_buf()));
        let task_id = TaskId("chain-test-remote".into());
        let clone_path = mgr.create_clone_for_chain(&task_id).await.unwrap();

        // Verify the clone's origin points to GitHub, not the local path.
        let output = std::process::Command::new("git")
            .args(["remote", "get-url", "origin"])
            .current_dir(&clone_path)
            .output()
            .unwrap();
        let url = String::from_utf8_lossy(&output.stdout).trim().to_string();
        assert_eq!(url, "git@github.com:user/repo.git");

        mgr.remove_clone(&task_id).await.unwrap();
    }

    #[test]
    fn chain_worktree_path_construction() {
        let mgr = WorktreeManager::new("/repo", Some(PathBuf::from("/tmp/wt")));
        let task_id = TaskId("chain-abc123".into());
        assert_eq!(
            mgr.chain_worktree_path(&task_id),
            PathBuf::from("/tmp/wt/chains/chain-abc123")
        );
    }

    #[tokio::test]
    async fn drop_cleans_up_slot_worktrees() {
        let src = tempfile::tempdir().unwrap();
        std::process::Command::new("git")
            .args(["init"])
            .current_dir(src.path())
            .output()
            .unwrap();
        std::process::Command::new("git")
            .args(["commit", "--allow-empty", "-m", "init"])
            .current_dir(src.path())
            .output()
            .unwrap();

        let base = tempfile::tempdir().unwrap();
        let slot_id = SlotId("drop-test-slot".into());
        let worktree_path;

        {
            let mgr = WorktreeManager::new(src.path(), Some(base.path().to_path_buf()));
            worktree_path = mgr.create(&slot_id).await.unwrap();
            assert!(worktree_path.exists(), "worktree should exist after create");
            // mgr is dropped here, triggering cleanup
        }

        assert!(
            !worktree_path.exists(),
            "worktree should be cleaned up after drop"
        );
    }

    #[tokio::test]
    async fn drop_cleans_up_chain_worktrees() {
        let src = tempfile::tempdir().unwrap();
        std::process::Command::new("git")
            .args(["init"])
            .current_dir(src.path())
            .output()
            .unwrap();
        std::process::Command::new("git")
            .args(["commit", "--allow-empty", "-m", "init"])
            .current_dir(src.path())
            .output()
            .unwrap();

        let base = tempfile::tempdir().unwrap();
        let task_id = TaskId("drop-test-chain".into());
        let worktree_path;

        {
            let mgr = WorktreeManager::new(src.path(), Some(base.path().to_path_buf()));
            worktree_path = mgr.create_for_chain(&task_id).await.unwrap();
            assert!(
                worktree_path.exists(),
                "chain worktree should exist after create"
            );
            // mgr is dropped here
        }

        assert!(
            !worktree_path.exists(),
            "chain worktree should be cleaned up after drop"
        );
    }

    #[tokio::test]
    async fn explicit_remove_prevents_double_cleanup() {
        let src = tempfile::tempdir().unwrap();
        std::process::Command::new("git")
            .args(["init"])
            .current_dir(src.path())
            .output()
            .unwrap();
        std::process::Command::new("git")
            .args(["commit", "--allow-empty", "-m", "init"])
            .current_dir(src.path())
            .output()
            .unwrap();

        let base = tempfile::tempdir().unwrap();
        let slot_id = SlotId("explicit-remove-test".into());

        let mgr = WorktreeManager::new(src.path(), Some(base.path().to_path_buf()));
        let _path = mgr.create(&slot_id).await.unwrap();
        mgr.remove(&slot_id).await.unwrap();

        // After explicit remove, tracked_slots should be empty.
        let tracked = mgr.tracked_slots.lock().unwrap();
        assert!(
            tracked.is_empty(),
            "slot should be untracked after explicit remove"
        );
    }

    #[test]
    fn tracking_is_idempotent() {
        let mgr = WorktreeManager::new("/repo", Some(PathBuf::from("/tmp/wt")));
        let id = SlotId("dup-test".into());
        mgr.track_slot(&id);
        mgr.track_slot(&id);
        let tracked = mgr.tracked_slots.lock().unwrap();
        assert_eq!(tracked.len(), 1, "duplicate tracking should be prevented");
    }
}
