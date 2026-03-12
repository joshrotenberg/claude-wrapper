use std::path::Path;

use anyhow::Result;

/// Codebase context gathered for the decisioner.
#[derive(Debug, Clone)]
pub struct TaskContext {
    /// Output of `git status --short`.
    pub git_status: String,
    /// Top-level directory listing.
    pub file_tree: String,
    /// Recent commit messages.
    pub recent_commits: String,
}

impl TaskContext {
    /// Gather context from the current working directory (or the given path).
    pub async fn gather(working_dir: Option<&Path>) -> Result<Self> {
        let dir = working_dir.unwrap_or_else(|| Path::new("."));

        let git_status = run_command(dir, "git", &["status", "--short"]).await;
        let file_tree = run_command(dir, "git", &["ls-tree", "--name-only", "HEAD"]).await;
        let recent_commits =
            run_command(dir, "git", &["log", "--oneline", "-10", "--no-decorate"]).await;

        Ok(Self {
            git_status,
            file_tree,
            recent_commits,
        })
    }

    /// Format context as a string for inclusion in a prompt.
    pub fn to_prompt_section(&self) -> String {
        let mut s = String::new();

        if !self.git_status.is_empty() {
            s.push_str("## Git Status\n```\n");
            s.push_str(&self.git_status);
            s.push_str("```\n\n");
        } else {
            s.push_str("## Git Status\nClean working tree.\n\n");
        }

        if !self.file_tree.is_empty() {
            s.push_str("## Repository Structure\n```\n");
            s.push_str(&self.file_tree);
            s.push_str("```\n\n");
        }

        if !self.recent_commits.is_empty() {
            s.push_str("## Recent Commits\n```\n");
            s.push_str(&self.recent_commits);
            s.push_str("```\n\n");
        }

        s
    }
}

async fn run_command(dir: &Path, cmd: &str, args: &[&str]) -> String {
    let output = tokio::process::Command::new(cmd)
        .args(args)
        .current_dir(dir)
        .output()
        .await;

    match output {
        Ok(o) if o.status.success() => String::from_utf8_lossy(&o.stdout).trim().to_string(),
        _ => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_prompt_section_clean() {
        let ctx = TaskContext {
            git_status: String::new(),
            file_tree: "Cargo.toml\nsrc\ntests".to_string(),
            recent_commits: "abc1234 feat: initial commit".to_string(),
        };

        let section = ctx.to_prompt_section();
        assert!(section.contains("Clean working tree"));
        assert!(section.contains("Cargo.toml"));
        assert!(section.contains("abc1234"));
    }

    #[test]
    fn test_prompt_section_dirty() {
        let ctx = TaskContext {
            git_status: "M src/main.rs\n?? new_file.rs".to_string(),
            file_tree: String::new(),
            recent_commits: String::new(),
        };

        let section = ctx.to_prompt_section();
        assert!(section.contains("M src/main.rs"));
        assert!(!section.contains("Clean working tree"));
    }
}
