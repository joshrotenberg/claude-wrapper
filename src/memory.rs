//! Read-side access to Claude Code's per-project **memory**
//! directories.
//!
//! Claude Code's auto-memory persists facts per project under
//! `~/.claude/projects/<slug>/memory/`: a `MEMORY.md` index (one
//! line per memory, loaded into context each session) plus one fact
//! per `<stem>.md` file with YAML frontmatter. A typical fact file:
//!
//! ```text
//! ---
//! name: cluster-72-canonical-slot-crash
//! description: one-line summary used for recall relevance
//! metadata:
//!   type: project
//! ---
//!
//! The fact body, with [[wiki-style]] links to other memories.
//! ```
//!
//! This module is read-only on purpose, like the other
//! introspection modules. The layout is undocumented Claude Code
//! internal state (observed against CLI 2.1.219) and can change
//! across CLI versions, so parsing is permissive: `name`,
//! `description`, and the `type` under `metadata:` are typed
//! (as plain strings); every other frontmatter key lands in
//! [`Memory::extra`] verbatim.
//!
//! YAML block scalars (`>`, `>-`, `>+`, `|`, `|-`, `|+`) are
//! supported for any key, which is how multi-line descriptions are
//! usually written. `>` folds the block into one line, `|` preserves
//! the line breaks, and the chomping indicator controls the trailing
//! newline. Continuation lines are part of the value even when they
//! contain a colon.
//!
//! Three levels of granularity:
//!
//! - [`MemoryRoot::list_projects_with_memory`] -- which projects
//!   have a memory directory at all.
//! - [`MemoryRoot::list`] -- summaries of one project's memory
//!   files.
//! - [`MemoryRoot::get`] -- one memory's full record including the
//!   body; [`MemoryRoot::index`] -- the raw `MEMORY.md`.
//!
//! # Example
//!
//! ```no_run
//! use claude_wrapper::memory::MemoryRoot;
//!
//! # fn example() -> claude_wrapper::Result<()> {
//! let root = MemoryRoot::home()?;
//! for project in root.list_projects_with_memory()? {
//!     println!("{}: {} memories", project.slug, project.entry_count);
//!     for m in root.list(&project.slug)? {
//!         println!("  {}: {}", m.name, m.description.as_deref().unwrap_or(""));
//!     }
//! }
//! # Ok(()) }
//! ```

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::artifacts::{frontmatter_entries, split_frontmatter};
use crate::error::{Error, Result};

/// Root directory of Claude Code's per-project state. Defaults to
/// `~/.claude/projects` (memory directories live under each project
/// slug); override with [`MemoryRoot::at`] for tests or non-default
/// installs.
#[derive(Debug, Clone)]
pub struct MemoryRoot {
    path: PathBuf,
}

impl MemoryRoot {
    /// Resolve the default `~/.claude/projects`. Errors if `$HOME`
    /// (or the platform-specific user home) cannot be determined.
    pub fn home() -> Result<Self> {
        let home = home_dir().ok_or_else(|| Error::Artifacts {
            message: "could not determine user home directory".to_string(),
        })?;
        Ok(Self {
            path: home.join(".claude").join("projects"),
        })
    }

    /// Use a specific path as the projects root. Useful for tests
    /// (point at a tempdir) and for non-default installs.
    pub fn at(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    /// The configured root directory.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// List every project slug that has a `memory/` directory,
    /// sorted by slug. Projects without one are omitted; a missing
    /// root returns an empty vec.
    pub fn list_projects_with_memory(&self) -> Result<Vec<ProjectMemorySummary>> {
        let entries = match fs::read_dir(&self.path) {
            Ok(it) => it,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(e) => return Err(e.into()),
        };
        let mut out = Vec::new();
        for entry in entries.flatten() {
            let project_dir = entry.path();
            if !project_dir.is_dir() {
                continue;
            }
            let Some(slug) = project_dir.file_name().and_then(|s| s.to_str()) else {
                continue;
            };
            let memory_dir = project_dir.join("memory");
            if !memory_dir.is_dir() {
                continue;
            }
            let entry_count = memory_files(&memory_dir).len();
            let has_index = memory_dir.join("MEMORY.md").is_file();
            out.push(ProjectMemorySummary {
                slug: slug.to_string(),
                memory_dir,
                entry_count,
                has_index,
            });
        }
        out.sort_by(|a, b| a.slug.cmp(&b.slug));
        Ok(out)
    }

    /// List one project's memory files, sorted by file stem.
    /// `MEMORY.md` (the index) is excluded; read it with
    /// [`Self::index`]. A project without a memory directory (or an
    /// unknown slug) returns an empty vec. Files that fail to read
    /// contribute a tracing warning and are skipped.
    pub fn list(&self, slug: &str) -> Result<Vec<MemorySummary>> {
        let memory_dir = self.path.join(slug).join("memory");
        let mut out = Vec::new();
        for path in memory_files(&memory_dir) {
            match parse_memory_file(&path) {
                Ok(memory) => out.push(MemorySummary::from_memory(&memory)),
                Err(e) => tracing::warn!(?path, "skipping memory file: {e}"),
            }
        }
        out.sort_by(|a, b| a.file_stem.cmp(&b.file_stem));
        Ok(out)
    }

    /// Read one memory by file stem (the basename of `<stem>.md`
    /// under the project's memory directory). Errors if no such
    /// file exists.
    pub fn get(&self, slug: &str, file_stem: &str) -> Result<Memory> {
        let path = self
            .path
            .join(slug)
            .join("memory")
            .join(format!("{file_stem}.md"));
        if !path.is_file() {
            return Err(Error::Artifacts {
                message: format!("no memory at {}", path.display()),
            });
        }
        parse_memory_file(&path)
    }

    /// The raw `MEMORY.md` index content for one project, or `None`
    /// when the project has no memory directory or no index file.
    pub fn index(&self, slug: &str) -> Result<Option<String>> {
        let path = self.path.join(slug).join("memory").join("MEMORY.md");
        if !path.is_file() {
            return Ok(None);
        }
        Ok(Some(fs::read_to_string(&path)?))
    }
}

/// One project that has a memory directory, returned by
/// [`MemoryRoot::list_projects_with_memory`].
#[derive(Debug, Clone, Serialize)]
pub struct ProjectMemorySummary {
    /// Project slug (the encoded-path directory name).
    pub slug: String,
    /// Absolute path of the `memory/` directory.
    pub memory_dir: PathBuf,
    /// Number of memory files (excluding `MEMORY.md`).
    pub entry_count: usize,
    /// Whether a `MEMORY.md` index is present.
    pub has_index: bool,
}

/// Lightweight metadata for one memory file, returned by
/// [`MemoryRoot::list`]. Strips the body to keep listings cheap.
#[derive(Debug, Clone, Serialize)]
pub struct MemorySummary {
    /// File stem (the basename of `<stem>.md`). The canonical
    /// handle for [`MemoryRoot::get`].
    pub file_stem: String,
    /// Frontmatter `name` if present; falls back to `file_stem`.
    pub name: String,
    /// Frontmatter `description` if present.
    pub description: Option<String>,
    /// The `type` recorded under `metadata:` (`user`, `feedback`,
    /// `project`, `reference`, or anything future), carried as a
    /// plain string.
    pub memory_type: Option<String>,
    /// Absolute path to the source `.md`.
    pub file_path: PathBuf,
    /// File size in bytes.
    pub size_bytes: u64,
}

impl MemorySummary {
    fn from_memory(m: &Memory) -> Self {
        let size_bytes = fs::metadata(&m.file_path)
            .map(|meta| meta.len())
            .unwrap_or_default();
        Self {
            file_stem: m.file_stem.clone(),
            name: m.name.clone(),
            description: m.description.clone(),
            memory_type: m.memory_type.clone(),
            file_path: m.file_path.clone(),
            size_bytes,
        }
    }
}

/// Full memory record returned by [`MemoryRoot::get`].
#[derive(Debug, Clone, Serialize)]
pub struct Memory {
    /// File stem (the basename of `<stem>.md`). The canonical
    /// handle for lookup.
    pub file_stem: String,
    /// Frontmatter `name` if present; falls back to `file_stem`.
    pub name: String,
    /// Frontmatter `description` if present.
    pub description: Option<String>,
    /// The `type` recorded under `metadata:`, as a plain string.
    pub memory_type: Option<String>,
    /// Absolute path to the source `.md`.
    pub file_path: PathBuf,
    /// Markdown body after the frontmatter block (trimmed of
    /// leading/trailing blank lines). `[[wiki-style]]` links are
    /// left verbatim.
    pub body: String,
    /// Frontmatter keys other than the typed ones, flattened line
    /// by line (nested YAML keys appear under their own names).
    /// Preserves unknown future fields verbatim as raw strings.
    pub extra: BTreeMap<String, String>,
}

/// Memory fact files in a directory: direct children matching
/// `*.md`, excluding the `MEMORY.md` index. Missing or unreadable
/// directories yield an empty list.
fn memory_files(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    if let Ok(entries) = fs::read_dir(dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_file() {
                continue;
            }
            if path.extension().and_then(|s| s.to_str()) != Some("md") {
                continue;
            }
            if path.file_name().and_then(|s| s.to_str()) == Some("MEMORY.md") {
                continue;
            }
            out.push(path);
        }
    }
    out
}

fn parse_memory_file(file_path: &Path) -> Result<Memory> {
    let file_stem = file_path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or_default()
        .to_string();
    let raw = fs::read_to_string(file_path)?;
    let (frontmatter, body) = split_frontmatter(&raw);

    let mut name = file_stem.clone();
    let mut description = None;
    let mut memory_type = None;
    let mut extra = BTreeMap::new();

    if let Some(fm) = frontmatter {
        for (key, value) in frontmatter_entries(fm) {
            let value = unquote(&value).to_string();
            match key.as_str() {
                "name" if !value.is_empty() => name = value,
                "description" if !value.is_empty() => description = Some(value),
                // The line-based parse flattens the nested
                // `metadata:` block, so its `type:` arrives as a
                // bare key.
                "type" if !value.is_empty() => memory_type = Some(value),
                _ if !value.is_empty() => {
                    extra.insert(key, value);
                }
                _ => {}
            }
        }
    }

    Ok(Memory {
        file_stem,
        name,
        description,
        memory_type,
        file_path: file_path.to_path_buf(),
        body: body.trim().to_string(),
        extra,
    })
}

/// Strip one pair of matching surrounding double quotes, if
/// present. Frontmatter values are sometimes written quoted (e.g.
/// descriptions containing punctuation).
fn unquote(value: &str) -> &str {
    value
        .strip_prefix('"')
        .and_then(|v| v.strip_suffix('"'))
        .unwrap_or(value)
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
    use std::io::Write;

    fn write_memory(root: &Path, slug: &str, stem: &str, contents: &str) -> PathBuf {
        let dir = root.join(slug).join("memory");
        fs::create_dir_all(&dir).expect("create memory dir");
        let path = dir.join(format!("{stem}.md"));
        let mut f = fs::File::create(&path).expect("create memory file");
        f.write_all(contents.as_bytes()).expect("write memory file");
        path
    }

    fn fixture_root() -> tempfile::TempDir {
        let tmp = tempfile::tempdir().expect("tempdir");
        write_memory(
            tmp.path(),
            "-Users-me-Code-projA",
            "user-name",
            "---\nname: user-name\ndescription: \"preferred name - quoted\"\nmetadata:\n  type: user\n---\n\nThe user goes by Zed. See [[other-memory]].\n",
        );
        write_memory(
            tmp.path(),
            "-Users-me-Code-projA",
            "no-frontmatter",
            "Just a body.\n",
        );
        fs::write(
            tmp.path()
                .join("-Users-me-Code-projA")
                .join("memory")
                .join("MEMORY.md"),
            "# Memory index\n\n- [User name](user-name.md)\n",
        )
        .unwrap();
        // A project without a memory directory.
        fs::create_dir_all(tmp.path().join("-Users-me-Code-projB")).unwrap();
        tmp
    }

    #[test]
    fn list_projects_with_memory_omits_projects_without() {
        let tmp = fixture_root();
        let root = MemoryRoot::at(tmp.path());
        let projects = root.list_projects_with_memory().expect("list");
        assert_eq!(projects.len(), 1);
        assert_eq!(projects[0].slug, "-Users-me-Code-projA");
        assert_eq!(projects[0].entry_count, 2);
        assert!(projects[0].has_index);
    }

    #[test]
    fn list_projects_missing_root_returns_empty() {
        let tmp = tempfile::tempdir().unwrap();
        let root = MemoryRoot::at(tmp.path().join("does-not-exist"));
        assert!(root.list_projects_with_memory().expect("ok").is_empty());
    }

    #[test]
    fn list_excludes_index_and_parses_metadata() {
        let tmp = fixture_root();
        let root = MemoryRoot::at(tmp.path());
        let memories = root.list("-Users-me-Code-projA").expect("list");
        let stems: Vec<&str> = memories.iter().map(|m| m.file_stem.as_str()).collect();
        assert_eq!(stems, ["no-frontmatter", "user-name"]);
        let m = memories
            .iter()
            .find(|m| m.file_stem == "user-name")
            .unwrap();
        assert_eq!(m.name, "user-name");
        assert_eq!(m.description.as_deref(), Some("preferred name - quoted"));
        assert_eq!(m.memory_type.as_deref(), Some("user"));
        assert!(m.size_bytes > 0);
    }

    #[test]
    fn list_unknown_slug_returns_empty() {
        let tmp = fixture_root();
        let root = MemoryRoot::at(tmp.path());
        assert!(root.list("nope").expect("ok").is_empty());
        assert!(root.list("-Users-me-Code-projB").expect("ok").is_empty());
    }

    #[test]
    fn get_returns_body_and_falls_back_to_stem() {
        let tmp = fixture_root();
        let root = MemoryRoot::at(tmp.path());
        let m = root.get("-Users-me-Code-projA", "user-name").expect("get");
        assert!(m.body.contains("[[other-memory]]"));
        let nf = root
            .get("-Users-me-Code-projA", "no-frontmatter")
            .expect("get");
        assert_eq!(nf.name, "no-frontmatter");
        assert_eq!(nf.memory_type, None);
        assert_eq!(nf.body, "Just a body.");
    }

    #[test]
    fn get_unknown_stem_errors() {
        let tmp = fixture_root();
        let root = MemoryRoot::at(tmp.path());
        let err = root.get("-Users-me-Code-projA", "nope").unwrap_err();
        assert!(err.to_string().contains("no memory at"));
    }

    #[test]
    fn index_reads_memory_md_or_none() {
        let tmp = fixture_root();
        let root = MemoryRoot::at(tmp.path());
        let idx = root.index("-Users-me-Code-projA").expect("ok");
        assert!(idx.expect("present").contains("# Memory index"));
        assert!(root.index("-Users-me-Code-projB").expect("ok").is_none());
        assert!(root.index("nope").expect("ok").is_none());
    }

    #[test]
    fn unknown_frontmatter_keys_land_in_extra() {
        let tmp = tempfile::tempdir().unwrap();
        write_memory(
            tmp.path(),
            "-slug",
            "weird",
            "---\nname: weird\nmetadata:\n  type: reference\n  originSessionId: abc\ncustom: kept\n---\nbody\n",
        );
        let root = MemoryRoot::at(tmp.path());
        let m = root.get("-slug", "weird").expect("get");
        assert_eq!(m.memory_type.as_deref(), Some("reference"));
        assert_eq!(
            m.extra.get("originSessionId").map(String::as_str),
            Some("abc")
        );
        assert_eq!(m.extra.get("custom").map(String::as_str), Some("kept"));
        // The bare `metadata:` container line has no value; dropped.
        assert!(!m.extra.contains_key("metadata"));
    }

    #[test]
    fn folded_description_with_colons_is_one_value() {
        let tmp = tempfile::tempdir().unwrap();
        write_memory(
            tmp.path(),
            "-slug",
            "folded",
            concat!(
                "---\n",
                "name: folded\n",
                "description: >-\n",
                "  Restarting as its own repo: MCP server plus CLI over one router,\n",
                "  SQLite persistence.\n",
                "metadata:\n",
                "  type: project\n",
                "---\n\nBody.\n",
            ),
        );
        let root = MemoryRoot::at(tmp.path());
        let m = root.get("-slug", "folded").expect("get");
        assert_eq!(
            m.description.as_deref(),
            Some(
                "Restarting as its own repo: MCP server plus CLI over one router, \
                 SQLite persistence."
            )
        );
        // The nested `metadata:` block still flattens to a bare `type`.
        assert_eq!(m.memory_type.as_deref(), Some("project"));
        assert!(m.extra.is_empty(), "extra: {:?}", m.extra);
        assert_eq!(m.body, "Body.");
    }
}
