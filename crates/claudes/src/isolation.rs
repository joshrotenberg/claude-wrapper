//! Worktree and clone creation/cleanup for task isolation.

use std::path::{Path, PathBuf};

use tracing::{debug, info};

use crate::error::{Error, Result};
use crate::manifest::Isolation;

/// A prepared isolation environment for a task.
#[derive(Debug)]
pub struct IsolatedEnv {
    /// The working directory for the task.
    pub work_dir: PathBuf,
    /// The isolation type (for cleanup).
    pub kind: IsolationKind,
}

/// What kind of isolation was set up.
#[derive(Debug)]
pub enum IsolationKind {
    /// Git worktree — needs `git worktree remove` on cleanup.
    Worktree {
        /// Path to the worktree.
        path: PathBuf,
    },
    /// No isolation — task runs in the original directory.
    None,
}

/// Reuse an existing worktree directory without running `git worktree add`.
///
/// Used when a chained task shares the same branch as a completed dependency —
/// the second task continues in the first task's worktree.
pub fn reuse_worktree(work_dir: &Path) -> IsolatedEnv {
    info!(path = %work_dir.display(), "reusing existing worktree");
    IsolatedEnv {
        work_dir: work_dir.to_path_buf(),
        kind: IsolationKind::Worktree {
            path: work_dir.to_path_buf(),
        },
    }
}

/// Create an isolated environment for a task.
pub async fn setup(
    project_dir: &Path,
    task_name: &str,
    branch: Option<&str>,
    isolation: Option<&Isolation>,
) -> Result<IsolatedEnv> {
    match isolation {
        Some(Isolation::Worktree { base_dir }) => {
            setup_worktree(project_dir, task_name, branch, base_dir).await
        }
        Some(Isolation::Clone { .. }) => {
            // Clone isolation is a future feature.
            Err(Error::Worktree(
                "clone isolation is not yet implemented".into(),
            ))
        }
        Some(Isolation::None) | None => Ok(IsolatedEnv {
            work_dir: project_dir.to_path_buf(),
            kind: IsolationKind::None,
        }),
    }
}

/// Create a git worktree for the task.
async fn setup_worktree(
    project_dir: &Path,
    task_name: &str,
    branch: Option<&str>,
    base_dir: &str,
) -> Result<IsolatedEnv> {
    let worktree_dir = project_dir.join(base_dir).join(task_name);

    // Ensure the base directory exists.
    let base = project_dir.join(base_dir);
    tokio::fs::create_dir_all(&base).await?;

    // Determine branch name.
    let branch_name = branch
        .map(String::from)
        .unwrap_or_else(|| format!("claudes/{task_name}"));

    // Check if worktree already exists.
    if worktree_dir.exists() {
        return Err(Error::Worktree(format!(
            "worktree already exists at {}; use --force to overwrite",
            worktree_dir.display()
        )));
    }

    info!(task = task_name, branch = %branch_name, path = %worktree_dir.display(), "creating worktree");

    // Create a new branch and worktree.
    let output = tokio::process::Command::new("git")
        .args(["worktree", "add", "-b", &branch_name])
        .arg(&worktree_dir)
        .arg("HEAD")
        .current_dir(project_dir)
        .output()
        .await?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        // If branch already exists, try without -b.
        if stderr.contains("already exists") {
            debug!("branch {branch_name} already exists, trying without -b");
            let output = tokio::process::Command::new("git")
                .args(["worktree", "add"])
                .arg(&worktree_dir)
                .arg(&branch_name)
                .current_dir(project_dir)
                .output()
                .await?;

            if !output.status.success() {
                let stderr = String::from_utf8_lossy(&output.stderr);
                return Err(Error::Worktree(format!(
                    "failed to create worktree: {stderr}"
                )));
            }
        } else {
            return Err(Error::Worktree(format!(
                "failed to create worktree: {stderr}"
            )));
        }
    }

    Ok(IsolatedEnv {
        work_dir: worktree_dir.clone(),
        kind: IsolationKind::Worktree { path: worktree_dir },
    })
}

/// Remove a worktree.
pub async fn cleanup(project_dir: &Path, env: &IsolatedEnv, force: bool) -> Result<()> {
    match &env.kind {
        IsolationKind::Worktree { path } => {
            info!(path = %path.display(), "removing worktree");

            let mut args = vec!["worktree", "remove"];
            if force {
                args.push("--force");
            }
            let path_str = path.to_string_lossy().to_string();
            args.push(&path_str);

            let output = tokio::process::Command::new("git")
                .args(&args)
                .current_dir(project_dir)
                .output()
                .await?;

            if !output.status.success() {
                let stderr = String::from_utf8_lossy(&output.stderr);
                return Err(Error::Worktree(format!(
                    "failed to remove worktree: {stderr}"
                )));
            }

            Ok(())
        }
        IsolationKind::None => Ok(()),
    }
}
