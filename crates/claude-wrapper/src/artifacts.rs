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

    /// Write (create or overwrite) an agent at `<file_stem>.md`.
    ///
    /// Atomic: writes to a temp file in the same directory and
    /// renames into place, so a crash mid-write can't leave a
    /// partially-written file. Creates the agents root directory
    /// if it doesn't exist.
    ///
    /// `file_stem` is validated for path traversal and reserved
    /// names (empty, `.`, `..`, embedded slashes / NUL bytes).
    /// To fail when the agent already exists instead of overwriting,
    /// use [`Self::write_new`].
    pub fn write(&self, file_stem: &str, input: AgentWriteInput) -> Result<()> {
        self.write_inner(file_stem, input, true)
    }

    /// Like [`Self::write`] but errors if the agent already exists.
    /// Useful for "create only" flows where overwriting an existing
    /// agent would be a bug.
    pub fn write_new(&self, file_stem: &str, input: AgentWriteInput) -> Result<()> {
        self.write_inner(file_stem, input, false)
    }

    fn write_inner(
        &self,
        file_stem: &str,
        input: AgentWriteInput,
        allow_overwrite: bool,
    ) -> Result<()> {
        validate_stem(file_stem)?;
        fs::create_dir_all(&self.path)?;
        let path = self.path.join(format!("{file_stem}.md"));
        if !allow_overwrite && path.exists() {
            return Err(Error::Artifacts {
                message: format!("agent already exists at {}", path.display()),
            });
        }

        let markdown = render_agent_markdown(file_stem, &input);

        // Atomic write: tempfile in same dir, then rename. Same-dir
        // tempfile keeps the rename a single inode operation on most
        // filesystems.
        let tmp = self.path.join(format!(".{file_stem}.md.tmp"));
        fs::write(&tmp, markdown)?;
        if let Err(e) = fs::rename(&tmp, &path) {
            // Best-effort cleanup; the rename failure is the real error.
            let _ = fs::remove_file(&tmp);
            return Err(e.into());
        }
        Ok(())
    }

    /// Remove the `<file_stem>.md` agent. Errors if no such file
    /// exists.
    pub fn delete(&self, file_stem: &str) -> Result<()> {
        validate_stem(file_stem)?;
        let path = self.path.join(format!("{file_stem}.md"));
        if !path.exists() {
            return Err(Error::Artifacts {
                message: format!("no agent at {}", path.display()),
            });
        }
        fs::remove_file(&path)?;
        Ok(())
    }
}

/// Input to [`AgentsRoot::write`] / [`AgentsRoot::write_new`].
///
/// Mirrors the parsed [`Agent`] minus the derived bits
/// (`file_stem` and `file_path` are determined by where the agent
/// is being written). `body` is required; everything else is
/// optional and omitted from the rendered frontmatter when empty.
#[derive(Debug, Clone, Default)]
pub struct AgentWriteInput {
    /// Frontmatter `name`. Defaults to the `file_stem` argument
    /// when absent.
    pub name: Option<String>,
    /// Frontmatter `description`. Omitted when None.
    pub description: Option<String>,
    /// Frontmatter `tools` as a list; rendered comma-joined.
    /// Empty list omits the key entirely.
    pub tools: Vec<String>,
    /// Frontmatter `model`. Omitted when None.
    pub model: Option<String>,
    /// Body of the agent prompt. Trimmed of surrounding whitespace
    /// before write.
    pub body: String,
    /// Additional frontmatter key/value pairs preserved verbatim.
    /// Iterated in sorted order for deterministic output.
    pub extra: BTreeMap<String, String>,
}

fn render_agent_markdown(file_stem: &str, input: &AgentWriteInput) -> String {
    let name = input.name.as_deref().unwrap_or(file_stem);
    let mut out = String::from("---\n");
    out.push_str(&format!("name: {name}\n"));
    if let Some(desc) = &input.description {
        out.push_str(&format!("description: {desc}\n"));
    }
    if !input.tools.is_empty() {
        out.push_str(&format!("tools: {}\n", input.tools.join(", ")));
    }
    if let Some(model) = &input.model {
        out.push_str(&format!("model: {model}\n"));
    }
    for (k, v) in &input.extra {
        out.push_str(&format!("{k}: {v}\n"));
    }
    out.push_str("---\n\n");
    out.push_str(input.body.trim());
    out.push('\n');
    out
}

fn validate_stem(stem: &str) -> Result<()> {
    if stem.is_empty() {
        return Err(Error::Artifacts {
            message: "file_stem cannot be empty".into(),
        });
    }
    if stem == "." || stem == ".." {
        return Err(Error::Artifacts {
            message: format!("file_stem cannot be {stem:?}"),
        });
    }
    if stem.contains('/') || stem.contains('\\') || stem.contains('\0') {
        return Err(Error::Artifacts {
            message: format!("file_stem contains invalid characters: {stem:?}"),
        });
    }
    Ok(())
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
pub(crate) fn split_frontmatter(raw: &str) -> (Option<&str>, &str) {
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

    // -- write / write_new / delete -----------------------------------

    fn input_with_body(body: &str) -> AgentWriteInput {
        AgentWriteInput {
            body: body.into(),
            ..Default::default()
        }
    }

    #[test]
    fn write_creates_new_agent_round_trips_via_get() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let root = AgentsRoot::at(tmp.path());
        let input = AgentWriteInput {
            name: Some("my-agent".into()),
            description: Some("does the thing".into()),
            tools: vec!["Read".into(), "Bash".into()],
            model: Some("sonnet".into()),
            body: "You are an agent.".into(),
            extra: BTreeMap::new(),
        };
        root.write("my-agent", input).expect("write");

        let agent = root.get("my-agent").expect("get");
        assert_eq!(agent.name, "my-agent");
        assert_eq!(agent.description.as_deref(), Some("does the thing"));
        assert_eq!(agent.tools, vec!["Read", "Bash"]);
        assert_eq!(agent.model.as_deref(), Some("sonnet"));
        assert_eq!(agent.body, "You are an agent.");
    }

    #[test]
    fn write_overwrites_existing_agent() {
        let tmp = fixture_root();
        let root = AgentsRoot::at(tmp.path());
        // rust-qa exists in the fixture.
        let input = AgentWriteInput {
            description: Some("rewritten".into()),
            body: "new body".into(),
            ..Default::default()
        };
        root.write("rust-qa", input).expect("overwrite");
        let agent = root.get("rust-qa").expect("get");
        assert_eq!(agent.description.as_deref(), Some("rewritten"));
        assert_eq!(agent.body, "new body");
        // tools/model from the original should be gone -- write
        // replaces the whole file.
        assert!(agent.tools.is_empty(), "tools: {:?}", agent.tools);
        assert!(agent.model.is_none());
    }

    #[test]
    fn write_new_errors_when_already_exists() {
        let tmp = fixture_root();
        let root = AgentsRoot::at(tmp.path());
        let err = root
            .write_new("rust-qa", input_with_body("body"))
            .unwrap_err();
        assert!(err.to_string().contains("already exists"), "err: {err}");
    }

    #[test]
    fn write_new_succeeds_for_fresh_stem() {
        let tmp = fixture_root();
        let root = AgentsRoot::at(tmp.path());
        root.write_new("brand-new", input_with_body("hello"))
            .expect("write_new");
        let agent = root.get("brand-new").expect("get");
        assert_eq!(agent.body, "hello");
    }

    #[test]
    fn write_creates_root_directory_if_missing() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let root = AgentsRoot::at(tmp.path().join("does-not-exist-yet"));
        root.write("foo", input_with_body("body")).expect("write");
        let agent = root.get("foo").expect("get");
        assert_eq!(agent.body, "body");
    }

    #[test]
    fn write_defaults_name_to_file_stem_when_absent() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let root = AgentsRoot::at(tmp.path());
        root.write("my-stem", input_with_body("b")).expect("write");
        let agent = root.get("my-stem").expect("get");
        assert_eq!(agent.name, "my-stem");
    }

    #[test]
    fn write_preserves_extra_keys() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let root = AgentsRoot::at(tmp.path());
        let mut extra = BTreeMap::new();
        extra.insert("custom_key".into(), "custom_value".into());
        let input = AgentWriteInput {
            body: "b".into(),
            extra,
            ..Default::default()
        };
        root.write("ex", input).expect("write");
        let agent = root.get("ex").expect("get");
        assert_eq!(
            agent.extra.get("custom_key").map(String::as_str),
            Some("custom_value")
        );
    }

    #[test]
    fn write_omits_optional_keys_when_unset() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let root = AgentsRoot::at(tmp.path());
        root.write("min", input_with_body("body only"))
            .expect("write");
        let raw = std::fs::read_to_string(tmp.path().join("min.md")).unwrap();
        assert!(!raw.contains("description:"), "raw: {raw}");
        assert!(!raw.contains("tools:"), "raw: {raw}");
        assert!(!raw.contains("model:"), "raw: {raw}");
    }

    #[test]
    fn write_rejects_path_traversal() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let root = AgentsRoot::at(tmp.path());
        for bad in ["", ".", "..", "a/b", "a\\b", "a\0b"] {
            let err = root.write(bad, input_with_body("b")).unwrap_err();
            assert!(
                err.to_string().to_lowercase().contains("file_stem"),
                "bad stem {bad:?} not rejected: {err}"
            );
        }
    }

    #[test]
    fn delete_removes_file() {
        let tmp = fixture_root();
        let root = AgentsRoot::at(tmp.path());
        assert!(root.get("rust-qa").is_ok());
        root.delete("rust-qa").expect("delete");
        let err = root.get("rust-qa").unwrap_err();
        assert!(err.to_string().contains("no agent"), "err: {err}");
    }

    #[test]
    fn delete_unknown_stem_errors() {
        let tmp = fixture_root();
        let root = AgentsRoot::at(tmp.path());
        let err = root.delete("nope").unwrap_err();
        assert!(err.to_string().contains("no agent"), "err: {err}");
    }

    #[test]
    fn delete_rejects_path_traversal() {
        let tmp = fixture_root();
        let root = AgentsRoot::at(tmp.path());
        for bad in ["", ".", "..", "a/b", "a\\b"] {
            let err = root.delete(bad).unwrap_err();
            assert!(
                err.to_string().to_lowercase().contains("file_stem"),
                "bad stem {bad:?} not rejected: {err}"
            );
        }
    }

    #[test]
    fn render_orders_canonical_keys_before_extras() {
        let mut extra = BTreeMap::new();
        extra.insert("zzz_last".into(), "v".into());
        extra.insert("aaa_first".into(), "v".into());
        let input = AgentWriteInput {
            name: Some("n".into()),
            description: Some("d".into()),
            tools: vec!["t1".into(), "t2".into()],
            model: Some("haiku".into()),
            body: "body".into(),
            extra,
        };
        let md = render_agent_markdown("stem", &input);
        let lines: Vec<&str> = md.lines().collect();
        // Header
        assert_eq!(lines[0], "---");
        // Canonical order: name, description, tools, model, then sorted extras.
        assert_eq!(lines[1], "name: n");
        assert_eq!(lines[2], "description: d");
        assert_eq!(lines[3], "tools: t1, t2");
        assert_eq!(lines[4], "model: haiku");
        assert_eq!(lines[5], "aaa_first: v");
        assert_eq!(lines[6], "zzz_last: v");
        assert_eq!(lines[7], "---");
    }
}
