//! Git worktree isolation for parallel workers.
//!
//! When multiple workers operate on the same repository, they need
//! isolated working directories to avoid stepping on each other's
//! git state. This module manages git worktree creation and cleanup.

use std::path::{Path, PathBuf};

use crate::error::{Error, Result};
use crate::types::WorkerId;

/// Manages git worktrees for pool workers.
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

    /// Create a worktree for a worker.
    ///
    /// Creates a git worktree at `{base_dir}/{worker_id}` branched from
    /// the current HEAD.
    pub async fn create(&self, worker_id: &WorkerId) -> Result<PathBuf> {
        let worktree_path = self.base_dir.join(&worker_id.0);

        // Ensure base directory exists.
        tokio::fs::create_dir_all(&self.base_dir)
            .await
            .map_err(|e| Error::Store(format!("failed to create worktree base dir: {e}")))?;

        // Remove existing worktree if it exists (stale from previous run).
        if worktree_path.exists() {
            self.remove(worker_id).await?;
        }

        let branch_name = format!("claude-pool/{}", worker_id.0);
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
            worker_id = %worker_id.0,
            path = %worktree_path.display(),
            "created git worktree"
        );

        Ok(worktree_path)
    }

    /// Remove a worker's worktree and its branch.
    pub async fn remove(&self, worker_id: &WorkerId) -> Result<()> {
        let worktree_path = self.base_dir.join(&worker_id.0);

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
                    worker_id = %worker_id.0,
                    error = %stderr,
                    "failed to remove worktree, cleaning up manually"
                );
                // Fall back to manual removal.
                let _ = tokio::fs::remove_dir_all(&worktree_path).await;
            }
        }

        // Clean up the branch.
        let branch_name = format!("claude-pool/{}", worker_id.0);
        let _ = tokio::process::Command::new("git")
            .args(["branch", "-D", &branch_name])
            .current_dir(&self.repo_dir)
            .output()
            .await;

        tracing::debug!(
            worker_id = %worker_id.0,
            "removed git worktree"
        );

        Ok(())
    }

    /// Remove all worktrees managed by this pool.
    pub async fn cleanup_all(&self, worker_ids: &[WorkerId]) -> Result<()> {
        for id in worker_ids {
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

    /// Get the worktree path for a worker (may not exist yet).
    pub fn worktree_path(&self, worker_id: &WorkerId) -> PathBuf {
        self.base_dir.join(&worker_id.0)
    }

    /// Get the base directory for all worktrees.
    pub fn base_dir(&self) -> &Path {
        &self.base_dir
    }

    /// Get the source repository directory.
    pub fn repo_dir(&self) -> &Path {
        &self.repo_dir
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn worktree_path_construction() {
        let mgr = WorktreeManager::new("/repo", Some(PathBuf::from("/tmp/wt")));
        let id = WorkerId("worker-0".into());
        assert_eq!(mgr.worktree_path(&id), PathBuf::from("/tmp/wt/worker-0"));
    }

    #[test]
    fn default_base_dir() {
        let mgr = WorktreeManager::new("/repo", None);
        let expected = std::env::temp_dir().join("claude-pool").join("worktrees");
        assert_eq!(mgr.base_dir(), expected);
    }
}
