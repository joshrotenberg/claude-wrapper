//! Git worktree isolation for parallel slots.
//!
//! When multiple slots operate on the same repository, they need
//! isolated working directories to avoid stepping on each other's
//! git state. This module manages git worktree creation and cleanup.

use std::path::{Path, PathBuf};

use crate::error::{Error, Result};
use crate::types::{SlotId, TaskId};

/// Manages git worktrees for pool slots.
#[derive(Debug)]
pub struct WorktreeManager {
    /// Root directory for worktrees (e.g. `/tmp/claude-pool/worktrees`).
    base_dir: PathBuf,
    /// Source repository path.
    repo_dir: PathBuf,
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
        Self { base_dir, repo_dir }
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

        // Ensure clones directory exists.
        let clones_dir = self.base_dir.join("clones");
        tokio::fs::create_dir_all(&clones_dir)
            .await
            .map_err(|e| Error::Store(format!("failed to create clones dir: {e}")))?;

        // Remove existing clone if it exists (stale from previous run).
        if clone_path.exists() {
            self.remove_clone(task_id).await?;
        }

        // Use `git clone --local --shared` for full isolation with shared objects.
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

        tracing::debug!(
            task_id = %task_id.0,
            "removed chain clone"
        );

        Ok(())
    }

    /// Get the clone path for a chain (may not exist yet).
    pub fn clone_path(&self, task_id: &TaskId) -> PathBuf {
        self.base_dir.join("clones").join(&task_id.0)
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

    #[test]
    fn chain_worktree_path_construction() {
        let mgr = WorktreeManager::new("/repo", Some(PathBuf::from("/tmp/wt")));
        let task_id = TaskId("chain-abc123".into());
        assert_eq!(
            mgr.chain_worktree_path(&task_id),
            PathBuf::from("/tmp/wt/chains/chain-abc123")
        );
    }

    #[test]
    fn clone_path_construction() {
        let mgr = WorktreeManager::new("/repo", Some(PathBuf::from("/tmp/wt")));
        let task_id = TaskId("chain-xyz789".into());
        assert_eq!(
            mgr.clone_path(&task_id),
            PathBuf::from("/tmp/wt/clones/chain-xyz789")
        );
    }

    #[tokio::test]
    async fn create_clone_for_chain_creates_directory() {
        let tmpdir = tempfile::tempdir().unwrap();
        std::process::Command::new("git")
            .args(["init"])
            .current_dir(tmpdir.path())
            .output()
            .unwrap();

        let mgr_base = tempfile::tempdir().unwrap();
        let mgr =
            WorktreeManager::new_validated(tmpdir.path(), Some(mgr_base.path().to_path_buf()))
                .await
                .unwrap();

        let task_id = TaskId("chain-test".into());
        let clone_path = mgr.create_clone_for_chain(&task_id).await.unwrap();

        assert!(clone_path.exists(), "clone directory should exist");
        assert!(clone_path.join(".git").exists(), "clone should have .git");
    }

    #[tokio::test]
    async fn remove_clone_deletes_directory() {
        let tmpdir = tempfile::tempdir().unwrap();
        std::process::Command::new("git")
            .args(["init"])
            .current_dir(tmpdir.path())
            .output()
            .unwrap();

        let mgr_base = tempfile::tempdir().unwrap();
        let mgr =
            WorktreeManager::new_validated(tmpdir.path(), Some(mgr_base.path().to_path_buf()))
                .await
                .unwrap();

        let task_id = TaskId("chain-remove-test".into());
        let clone_path = mgr.create_clone_for_chain(&task_id).await.unwrap();
        assert!(clone_path.exists());

        mgr.remove_clone(&task_id).await.unwrap();
        assert!(!clone_path.exists(), "clone directory should be deleted");
    }

    #[test]
    fn clone_path_idempotent() {
        let mgr = WorktreeManager::new("/repo", Some(PathBuf::from("/tmp/wt")));
        let task_id = TaskId("chain-test".into());
        let path1 = mgr.clone_path(&task_id);
        let path2 = mgr.clone_path(&task_id);
        assert_eq!(path1, path2);
    }
}
