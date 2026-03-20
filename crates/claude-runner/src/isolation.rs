//! Work isolation — git worktrees and cleanup.

use std::path::{Path, PathBuf};

/// Create an isolated work directory for a run.
pub async fn create_worktree(
    repo_dir: &Path,
    branch: &str,
    base_dir: &str,
) -> crate::error::Result<PathBuf> {
    let slug: String = branch
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || c == '-' {
                c
            } else {
                '-'
            }
        })
        .collect();
    let worktree_dir = repo_dir.join(base_dir).join(&slug);

    tokio::fs::create_dir_all(repo_dir.join(base_dir)).await?;

    let output = tokio::process::Command::new("git")
        .args(["worktree", "add", "-b", branch])
        .arg(&worktree_dir)
        .arg("HEAD")
        .current_dir(repo_dir)
        .output()
        .await?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        // Branch may already exist — try without -b.
        if stderr.contains("already exists") {
            let output = tokio::process::Command::new("git")
                .args(["worktree", "add"])
                .arg(&worktree_dir)
                .arg(branch)
                .current_dir(repo_dir)
                .output()
                .await?;
            if !output.status.success() {
                let stderr = String::from_utf8_lossy(&output.stderr);
                return Err(crate::error::Error::Isolation(format!(
                    "failed to create worktree: {stderr}"
                )));
            }
        } else {
            return Err(crate::error::Error::Isolation(format!(
                "failed to create worktree: {stderr}"
            )));
        }
    }

    Ok(worktree_dir)
}

/// Remove a worktree.
pub async fn remove_worktree(
    repo_dir: &Path,
    worktree_dir: &Path,
    force: bool,
) -> crate::error::Result<()> {
    let mut args = vec!["worktree", "remove"];
    if force {
        args.push("--force");
    }
    let path_str = worktree_dir.to_string_lossy().to_string();
    args.push(&path_str);

    let output = tokio::process::Command::new("git")
        .args(&args)
        .current_dir(repo_dir)
        .output()
        .await?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(crate::error::Error::Isolation(format!(
            "failed to remove worktree: {stderr}"
        )));
    }

    Ok(())
}
