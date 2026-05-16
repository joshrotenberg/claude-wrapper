//! Read-side access to Claude Code's on-disk **agent** definitions.
//!
//! Claude Code resolves user-level agents from
//! `~/.claude/agents/<name>.md`. Each file is plain markdown with a
//! YAML-style frontmatter block delimited by `---` lines. The
//! frontmatter carries the agent's metadata (name, description,
//! optional tool allow-list, optional model); the body is the agent's
//! system prompt.
//!
//! This module is read-only on purpose -- mutations (create / update
//! / delete) are tracked separately so consumers that only want to
//! introspect the agent set don't need to opt into write semantics.
//!
//! Two levels of granularity:
//!
//! - [`AgentsRoot::list`] -- enumerate every agent at the root with
//!   summary metadata (name, description, tools, model, file path).
//! - [`AgentsRoot::get`] -- read one agent's full record including
//!   the prompt body.
//!
//! # Frontmatter format
//!
//! Real-world agents look like:
//!
//! ```text
//! ---
//! name: rust-qa
//! description: Use PROACTIVELY before declaring Rust work done...
//! tools: Read, Grep, Glob, Bash
//! model: sonnet
//! ---
//!
//! You are a Rust quality gate. ...
//! ```
//!
//! The parser is permissive: only `name`, `description`, `tools`, and
//! `model` are typed. `tools` is a comma-separated list. Any other
//! `key: value` pairs land in [`Agent::extra`] so unknown future keys
//! survive a round trip. Frontmatter is optional -- a body-only file
//! parses fine, with `name` defaulting to the file stem.
//!
//! # Example
//!
//! ```no_run
//! use claude_wrapper::artifacts::AgentsRoot;
//!
//! # fn example() -> claude_wrapper::Result<()> {
//! let root = AgentsRoot::home()?;
//! for summary in root.list()? {
//!     println!("{}: {}", summary.name, summary.description.as_deref().unwrap_or(""));
//! }
//! let agent = root.get("rust-qa")?;
//! println!("{}", agent.body);
//! # Ok(()) }
//! ```
//!
//! # Slug, name, file stem
//!
//! By convention an agent's `name` matches its filename stem:
//! `rust-qa.md` carries `name: rust-qa`. The two can diverge -- the
//! parser keeps both. [`AgentsRoot::get`] looks up by file stem
//! (because that's what the filesystem indexes), not by the
//! frontmatter `name`.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::error::{Error, Result};

/// Root directory of Claude Code's user-level agent definitions.
/// Defaults to `~/.claude/agents`; override with [`AgentsRoot::at`]
/// for tests or non-default installs.
#[derive(Debug, Clone)]
pub struct AgentsRoot {
    path: PathBuf,
}

impl AgentsRoot {
    /// Resolve the default `~/.claude/agents`. Errors if `$HOME`
    /// (or the platform-specific user home) cannot be determined.
    pub fn home() -> Result<Self> {
        let home = home_dir().ok_or_else(|| Error::Artifacts {
            message: "could not determine user home directory".to_string(),
        })?;
        Ok(Self {
            path: home.join(".claude").join("agents"),
        })
    }

    /// Use a specific path as the agents root. Useful for tests
    /// (point at a tempdir) and for non-default installs.
    pub fn at(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    /// The configured root directory.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// List every `*.md` agent at the root, sorted by file stem.
    ///
    /// Returns an empty vec if the root directory doesn't exist (a
    /// fresh Claude Code install with no user agents). Files that
    /// fail to parse contribute a tracing warning and are skipped
    /// rather than failing the whole listing.
    pub fn list(&self) -> Result<Vec<AgentSummary>> {
        let entries = match fs::read_dir(&self.path) {
            Ok(it) => it,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(e) => return Err(e.into()),
        };

        let mut out = Vec::new();
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|s| s.to_str()) != Some("md") {
                continue;
            }
            let stem = match path.file_stem().and_then(|s| s.to_str()) {
                Some(s) => s.to_string(),
                None => continue,
            };
            match parse_agent_file(&path, &stem) {
                Ok(agent) => out.push(AgentSummary::from_agent(&agent)),
                Err(e) => tracing::warn!(?path, "skipping agent: {e}"),
            }
        }
        out.sort_by(|a, b| a.file_stem.cmp(&b.file_stem));
        Ok(out)
    }

    /// Read one agent by file stem (i.e. the basename of `<stem>.md`
    /// under the root). Errors if no such file exists or it fails
    /// to parse.
    pub fn get(&self, file_stem: &str) -> Result<Agent> {
        let path = self.path.join(format!("{file_stem}.md"));
        if !path.exists() {
            return Err(Error::Artifacts {
                message: format!("no agent at {}", path.display()),
            });
        }
        parse_agent_file(&path, file_stem)
    }
}

/// Lightweight metadata for one agent, returned by
/// [`AgentsRoot::list`]. Strips the body to keep listings cheap.
#[derive(Debug, Clone, Serialize)]
pub struct AgentSummary {
    /// Filename stem (`<stem>.md`). The canonical handle for lookup.
    pub file_stem: String,
    /// Frontmatter `name` if present; falls back to `file_stem`.
    pub name: String,
    /// Frontmatter `description` if present.
    pub description: Option<String>,
    /// Frontmatter `tools` parsed as a comma-separated list.
    pub tools: Vec<String>,
    /// Frontmatter `model` if present.
    pub model: Option<String>,
    /// Absolute path to the source file.
    pub file_path: PathBuf,
    /// File size in bytes; useful for cheap UI hints.
    pub size_bytes: u64,
}

impl AgentSummary {
    fn from_agent(a: &Agent) -> Self {
        let size_bytes = fs::metadata(&a.file_path)
            .map(|m| m.len())
            .unwrap_or_default();
        Self {
            file_stem: a.file_stem.clone(),
            name: a.name.clone(),
            description: a.description.clone(),
            tools: a.tools.clone(),
            model: a.model.clone(),
            file_path: a.file_path.clone(),
            size_bytes,
        }
    }
}

/// Full agent record returned by [`AgentsRoot::get`].
#[derive(Debug, Clone, Serialize)]
pub struct Agent {
    /// Filename stem (`<stem>.md`). The canonical handle for lookup.
    pub file_stem: String,
    /// Frontmatter `name` if present; falls back to `file_stem`.
    pub name: String,
    /// Frontmatter `description` if present.
    pub description: Option<String>,
    /// Frontmatter `tools` parsed as a comma-separated list.
    pub tools: Vec<String>,
    /// Frontmatter `model` if present.
    pub model: Option<String>,
    /// Absolute path to the source file.
    pub file_path: PathBuf,
    /// Markdown body after the frontmatter block (trimmed of
    /// leading/trailing blank lines).
    pub body: String,
    /// Frontmatter keys other than the typed ones. Preserves
    /// unknown future fields verbatim as raw strings.
    pub extra: BTreeMap<String, String>,
}

fn parse_agent_file(path: &Path, file_stem: &str) -> Result<Agent> {
    let raw = fs::read_to_string(path)?;
    let (frontmatter, body) = split_frontmatter(&raw);

    let mut name = file_stem.to_string();
    let mut description = None;
    let mut tools = Vec::new();
    let mut model = None;
    let mut extra = BTreeMap::new();

    if let Some(fm) = frontmatter {
        for line in fm.lines() {
            let trimmed = line.trim();
            if trimmed.is_empty() {
                continue;
            }
            let Some((k, v)) = trimmed.split_once(':') else {
                continue;
            };
            let key = k.trim();
            let value = v.trim().to_string();
            match key {
                "name" if !value.is_empty() => name = value,
                "description" if !value.is_empty() => description = Some(value),
                "tools" if !value.is_empty() => {
                    tools = value
                        .split(',')
                        .map(|t| t.trim().to_string())
                        .filter(|t| !t.is_empty())
                        .collect();
                }
                "model" if !value.is_empty() => model = Some(value),
                _ if !key.is_empty() => {
                    extra.insert(key.to_string(), value);
                }
                _ => {}
            }
        }
    }

    Ok(Agent {
        file_stem: file_stem.to_string(),
        name,
        description,
        tools,
        model,
        file_path: path.to_path_buf(),
        body: body.trim().to_string(),
        extra,
    })
}

/// Split a markdown file into (optional frontmatter body, content
/// after the frontmatter). Frontmatter is delimited by a leading
/// `---` line and a closing `---` line. Anything else returns
/// `(None, full_text)`.
fn split_frontmatter(raw: &str) -> (Option<&str>, &str) {
    let mut lines = raw.split_inclusive('\n');
    let Some(first) = lines.next() else {
        return (None, raw);
    };
    if first.trim_end_matches(['\n', '\r']) != "---" {
        return (None, raw);
    }
    let after_first = first.len();
    let mut cursor = after_first;
    for line in lines {
        let len = line.len();
        if line.trim_end_matches(['\n', '\r']) == "---" {
            let fm = &raw[after_first..cursor];
            let body_start = cursor + len;
            let body = &raw[body_start..];
            return (Some(fm), body);
        }
        cursor += len;
    }
    (None, raw)
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

    fn write_agent(dir: &Path, file_stem: &str, contents: &str) -> PathBuf {
        let path = dir.join(format!("{file_stem}.md"));
        let mut f = fs::File::create(&path).expect("create md");
        f.write_all(contents.as_bytes()).expect("write md");
        path
    }

    fn fixture_root() -> tempfile::TempDir {
        let tmp = tempfile::tempdir().expect("tempdir");
        write_agent(
            tmp.path(),
            "rust-qa",
            "---\nname: rust-qa\ndescription: Rust quality gate\ntools: Read, Grep, Bash\nmodel: sonnet\n---\n\nYou are a Rust quality gate.\n",
        );
        write_agent(
            tmp.path(),
            "no-frontmatter",
            "Just a body, no frontmatter at all.\n",
        );
        write_agent(
            tmp.path(),
            "minimal",
            "---\nname: minimal\ndescription: Minimal agent\n---\nBody here.\n",
        );
        // A file with an unknown extra key should round-trip.
        write_agent(
            tmp.path(),
            "weird",
            "---\nname: weird\ndescription: has extras\ncustom_key: custom_value\n---\nbody\n",
        );
        // Non-md file should be ignored by list().
        let other = tmp.path().join("README.txt");
        fs::write(&other, "ignore me").expect("write txt");
        tmp
    }

    #[test]
    fn list_returns_only_md_files_sorted() {
        let tmp = fixture_root();
        let root = AgentsRoot::at(tmp.path());
        let agents = root.list().expect("list");
        let stems: Vec<&str> = agents.iter().map(|a| a.file_stem.as_str()).collect();
        assert_eq!(stems, ["minimal", "no-frontmatter", "rust-qa", "weird"]);
    }

    #[test]
    fn list_missing_root_returns_empty() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let root = AgentsRoot::at(tmp.path().join("does-not-exist"));
        let agents = root.list().expect("list");
        assert!(agents.is_empty());
    }

    #[test]
    fn list_typed_metadata() {
        let tmp = fixture_root();
        let root = AgentsRoot::at(tmp.path());
        let agents = root.list().expect("list");
        let rust_qa = agents
            .iter()
            .find(|a| a.file_stem == "rust-qa")
            .expect("rust-qa");
        assert_eq!(rust_qa.name, "rust-qa");
        assert_eq!(rust_qa.description.as_deref(), Some("Rust quality gate"));
        assert_eq!(rust_qa.tools, vec!["Read", "Grep", "Bash"]);
        assert_eq!(rust_qa.model.as_deref(), Some("sonnet"));
        assert!(rust_qa.size_bytes > 0);
    }

    #[test]
    fn list_no_frontmatter_falls_back_to_stem() {
        let tmp = fixture_root();
        let root = AgentsRoot::at(tmp.path());
        let agents = root.list().expect("list");
        let nf = agents
            .iter()
            .find(|a| a.file_stem == "no-frontmatter")
            .expect("no-frontmatter");
        assert_eq!(nf.name, "no-frontmatter");
        assert_eq!(nf.description, None);
        assert!(nf.tools.is_empty());
        assert!(nf.model.is_none());
    }

    #[test]
    fn get_returns_full_agent_with_body() {
        let tmp = fixture_root();
        let root = AgentsRoot::at(tmp.path());
        let agent = root.get("rust-qa").expect("get rust-qa");
        assert_eq!(agent.name, "rust-qa");
        assert_eq!(agent.body, "You are a Rust quality gate.");
    }

    #[test]
    fn get_no_frontmatter_returns_full_body() {
        let tmp = fixture_root();
        let root = AgentsRoot::at(tmp.path());
        let agent = root.get("no-frontmatter").expect("get");
        assert_eq!(agent.body, "Just a body, no frontmatter at all.");
        assert_eq!(agent.name, "no-frontmatter");
        assert!(agent.tools.is_empty());
    }

    #[test]
    fn get_unknown_id_errors() {
        let tmp = fixture_root();
        let root = AgentsRoot::at(tmp.path());
        let err = root.get("nope").unwrap_err();
        assert!(err.to_string().to_lowercase().contains("no agent"));
    }

    #[test]
    fn extra_keys_round_trip_as_strings() {
        let tmp = fixture_root();
        let root = AgentsRoot::at(tmp.path());
        let agent = root.get("weird").expect("get weird");
        assert_eq!(
            agent.extra.get("custom_key").map(String::as_str),
            Some("custom_value")
        );
    }

    #[test]
    fn split_frontmatter_with_block() {
        let raw = "---\nname: x\n---\nbody text\n";
        let (fm, body) = split_frontmatter(raw);
        assert_eq!(fm, Some("name: x\n"));
        assert_eq!(body, "body text\n");
    }

    #[test]
    fn split_frontmatter_no_block() {
        let raw = "no frontmatter here\nsecond line\n";
        let (fm, body) = split_frontmatter(raw);
        assert_eq!(fm, None);
        assert_eq!(body, raw);
    }

    #[test]
    fn split_frontmatter_open_no_close_returns_full() {
        // An opening --- with no matching close shouldn't swallow
        // the file. Conservative behavior: treat as no frontmatter.
        let raw = "---\nname: x\nstill no close here\n";
        let (fm, body) = split_frontmatter(raw);
        assert_eq!(fm, None);
        assert_eq!(body, raw);
    }

    #[test]
    fn empty_value_keys_dont_overwrite_defaults() {
        let tmp = tempfile::tempdir().expect("tempdir");
        write_agent(
            tmp.path(),
            "empty-name",
            "---\nname:\ndescription: keeps stem as name\n---\nbody\n",
        );
        let root = AgentsRoot::at(tmp.path());
        let agent = root.get("empty-name").expect("get");
        assert_eq!(agent.name, "empty-name");
    }
}
