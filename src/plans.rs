//! Read-side access to Claude Code's saved **plan** documents.
//!
//! Plan mode writes each accepted plan to
//! `~/.claude/plans/<slugged-name>.md` as plain markdown, one file
//! per plan, with a human-readable slug for a name (e.g.
//! `review-draft-pr-129-zany-lightning.md`). This module lists and
//! reads them; it is read-only on purpose, like the other
//! introspection modules. The layout is undocumented Claude Code
//! internal state (observed against CLI 2.1.219) and can change
//! across CLI versions.
//!
//! - [`PlansRoot::list`] -- every plan with summary metadata
//!   (first-heading title, size, modified time), most recently
//!   modified first.
//! - [`PlansRoot::get`] -- one plan's full markdown content.
//!
//! # Example
//!
//! ```no_run
//! use claude_wrapper::plans::PlansRoot;
//!
//! # fn example() -> claude_wrapper::Result<()> {
//! let root = PlansRoot::home()?;
//! for plan in root.list()? {
//!     println!("{}: {}", plan.file_stem, plan.title.as_deref().unwrap_or("(untitled)"));
//! }
//! # Ok(()) }
//! ```

use std::fs;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use serde::Serialize;

use crate::error::{Error, Result};

/// Root directory of Claude Code's saved plan documents. Defaults
/// to `~/.claude/plans`; override with [`PlansRoot::at`] for tests
/// or non-default installs.
#[derive(Debug, Clone)]
pub struct PlansRoot {
    path: PathBuf,
}

impl PlansRoot {
    /// Resolve the default `~/.claude/plans`. Errors if `$HOME`
    /// (or the platform-specific user home) cannot be determined.
    pub fn home() -> Result<Self> {
        let home = home_dir().ok_or_else(|| Error::Artifacts {
            message: "could not determine user home directory".to_string(),
        })?;
        Ok(Self {
            path: home.join(".claude").join("plans"),
        })
    }

    /// Use a specific path as the plans root. Useful for tests
    /// (point at a tempdir) and for non-default installs.
    pub fn at(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    /// The configured root directory.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// List every plan at the root, most recently modified first
    /// (ties broken by file stem). A missing root returns an empty
    /// vec. Files that fail to read contribute a tracing warning
    /// and are skipped.
    pub fn list(&self) -> Result<Vec<PlanSummary>> {
        let entries = match fs::read_dir(&self.path) {
            Ok(it) => it,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(e) => return Err(e.into()),
        };
        let mut out = Vec::new();
        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_file() || path.extension().and_then(|s| s.to_str()) != Some("md") {
                continue;
            }
            let Some(stem) = path.file_stem().and_then(|s| s.to_str()) else {
                continue;
            };
            match summarize_plan(&path, stem) {
                Ok(summary) => out.push(summary),
                Err(e) => tracing::warn!(?path, "skipping plan: {e}"),
            }
        }
        out.sort_by(|a, b| {
            b.modified
                .cmp(&a.modified)
                .then_with(|| a.file_stem.cmp(&b.file_stem))
        });
        Ok(out)
    }

    /// Read one plan's full markdown content by file stem (the
    /// basename of `<stem>.md` under the root). Errors if no such
    /// file exists.
    pub fn get(&self, file_stem: &str) -> Result<Plan> {
        let path = self.path.join(format!("{file_stem}.md"));
        if !path.is_file() {
            return Err(Error::Artifacts {
                message: format!("no plan at {}", path.display()),
            });
        }
        let content = fs::read_to_string(&path)?;
        Ok(Plan {
            file_stem: file_stem.to_string(),
            title: first_heading(&content),
            file_path: path,
            content,
        })
    }
}

/// Lightweight metadata for one plan, returned by
/// [`PlansRoot::list`]. Strips the content to keep listings cheap.
#[derive(Debug, Clone, Serialize)]
pub struct PlanSummary {
    /// File stem (the basename of `<stem>.md`). The canonical
    /// handle for [`PlansRoot::get`].
    pub file_stem: String,
    /// The first `#` heading in the document, when present.
    pub title: Option<String>,
    /// Absolute path to the source `.md`.
    pub file_path: PathBuf,
    /// File size in bytes.
    pub size_bytes: u64,
    /// Last-modified time, when the filesystem reports one.
    pub modified: Option<SystemTime>,
}

/// Full plan record returned by [`PlansRoot::get`].
#[derive(Debug, Clone, Serialize)]
pub struct Plan {
    /// File stem (the basename of `<stem>.md`).
    pub file_stem: String,
    /// The first `#` heading in the document, when present.
    pub title: Option<String>,
    /// Absolute path to the source `.md`.
    pub file_path: PathBuf,
    /// The full markdown content.
    pub content: String,
}

fn summarize_plan(path: &Path, stem: &str) -> Result<PlanSummary> {
    let meta = fs::metadata(path)?;
    // Only the head of the file is needed for the title; plans are
    // small, so a full read keeps this simple.
    let content = fs::read_to_string(path)?;
    Ok(PlanSummary {
        file_stem: stem.to_string(),
        title: first_heading(&content),
        file_path: path.to_path_buf(),
        size_bytes: meta.len(),
        modified: meta.modified().ok(),
    })
}

/// The text of the first markdown `#` heading (any level), trimmed.
fn first_heading(content: &str) -> Option<String> {
    for line in content.lines() {
        let trimmed = line.trim_start();
        if let Some(rest) = trimmed.strip_prefix('#') {
            let title = rest.trim_start_matches('#').trim();
            if !title.is_empty() {
                return Some(title.to_string());
            }
        }
    }
    None
}

fn home_dir() -> Option<PathBuf> {
    if let Ok(h) = std::env::var("HOME")
        && !h.is_empty()
    {
        return Some(PathBuf::from(h));
    }
    if let Ok(h) = std::env::var("USERPROFILE")
        && !h.is_empty()
    {
        return Some(PathBuf::from(h));
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_plan(root: &Path, stem: &str, contents: &str) {
        fs::create_dir_all(root).unwrap();
        fs::write(root.join(format!("{stem}.md")), contents).unwrap();
    }

    fn set_mtime(root: &Path, stem: &str, secs: u64) {
        let f = fs::OpenOptions::new()
            .write(true)
            .open(root.join(format!("{stem}.md")))
            .unwrap();
        f.set_modified(SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(secs))
            .unwrap();
    }

    fn fixture_root() -> tempfile::TempDir {
        let tmp = tempfile::tempdir().expect("tempdir");
        write_plan(
            tmp.path(),
            "older-plan",
            "# The older plan\n\n## Context\n\nDetails.\n",
        );
        write_plan(tmp.path(), "newer-plan", "No heading here, just prose.\n");
        set_mtime(tmp.path(), "older-plan", 1_000);
        set_mtime(tmp.path(), "newer-plan", 2_000);
        fs::write(tmp.path().join("not-a-plan.txt"), "ignored").unwrap();
        tmp
    }

    #[test]
    fn list_sorts_recent_first_and_extracts_titles() {
        let tmp = fixture_root();
        let root = PlansRoot::at(tmp.path());
        let plans = root.list().expect("list");
        let stems: Vec<&str> = plans.iter().map(|p| p.file_stem.as_str()).collect();
        assert_eq!(stems, ["newer-plan", "older-plan"]);
        assert_eq!(plans[0].title, None);
        assert_eq!(plans[1].title.as_deref(), Some("The older plan"));
        assert!(plans[1].size_bytes > 0);
        assert!(plans[1].modified.is_some());
    }

    #[test]
    fn list_missing_root_returns_empty() {
        let tmp = tempfile::tempdir().unwrap();
        let root = PlansRoot::at(tmp.path().join("does-not-exist"));
        assert!(root.list().expect("ok").is_empty());
    }

    #[test]
    fn get_returns_full_content() {
        let tmp = fixture_root();
        let root = PlansRoot::at(tmp.path());
        let plan = root.get("older-plan").expect("get");
        assert_eq!(plan.title.as_deref(), Some("The older plan"));
        assert!(plan.content.contains("## Context"));
    }

    #[test]
    fn get_unknown_stem_errors() {
        let tmp = fixture_root();
        let root = PlansRoot::at(tmp.path());
        let err = root.get("nope").unwrap_err();
        assert!(err.to_string().contains("no plan at"));
    }

    #[test]
    fn first_heading_skips_deeper_levels_only_when_empty() {
        assert_eq!(first_heading("## Sub only\n"), Some("Sub only".to_string()));
        assert_eq!(first_heading("#\n# Real\n"), Some("Real".to_string()));
        assert_eq!(first_heading("plain text\n"), None);
    }
}
