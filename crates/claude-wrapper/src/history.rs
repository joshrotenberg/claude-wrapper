//! Read-side access to Claude Code's on-disk session history.
//!
//! Claude Code stores per-project session logs as line-delimited
//! JSON under `~/.claude/projects/<slug>/<session_id>.jsonl`, with
//! one JSON object per line. This module gives a typed Rust API
//! over those logs without prescribing a representation for the
//! conversation -- consumers (UIs, MCP servers, tools) can render
//! however they want.
//!
//! Three levels of granularity:
//!
//! - [`HistoryRoot::list_projects`] -- enumerate project directories
//!   with summary metadata (session count, latest activity).
//! - [`HistoryRoot::list_sessions`] -- enumerate session files for
//!   one project (or all projects), with summary metadata
//!   (message count, first/last timestamps, optional auto-title).
//! - [`HistoryRoot::read_session`] -- parse a session into typed
//!   [`HistoryEntry`] values.
//!
//! # Liberal parsing
//!
//! Each line is parsed independently; malformed lines are skipped
//! (with a tracing warning) rather than failing the whole session.
//! Unknown entry types come through as [`HistoryEntry::Other`]
//! carrying the raw [`serde_json::Value`] so callers can inspect
//! them. The shape Claude Code writes today includes at least
//! `user`, `assistant`, `queue-operation`, `attachment`, `ai-title`,
//! `last-prompt` -- only `user` and `assistant` get typed variants;
//! the rest land in [`HistoryEntry::Other`].
//!
//! # Slug encoding
//!
//! Project directory names are filesystem-safe encodings of an
//! absolute path -- e.g. `/Users/josh/Code/foo` becomes
//! `-Users-josh-Code-foo`. [`ProjectSummary::decoded_path`] is a
//! best-effort decode (replace leading dash with `/` and remaining
//! dashes with `/`); it round-trips for paths that contain no
//! literal dashes in directory names. For uncertain cases keep the
//! `slug` and treat the decoded form as a hint.
//!
//! # Example
//!
//! ```no_run
//! use claude_wrapper::history::HistoryRoot;
//!
//! # fn example() -> claude_wrapper::Result<()> {
//! let root = HistoryRoot::home()?;
//! for project in root.list_projects()? {
//!     println!("{}: {} sessions", project.slug, project.session_count);
//!     for session in root.list_sessions(Some(&project.slug))? {
//!         println!("  {} ({} msgs)", session.session_id, session.message_count);
//!     }
//! }
//! # Ok(()) }
//! ```

use std::fs;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use serde::Serialize;
use serde_json::Value;

use crate::error::{Error, Result};

/// Root directory of Claude Code's on-disk history. Defaults to
/// `~/.claude/projects`; override with [`HistoryRoot::at`] for
/// tests or non-default installs.
#[derive(Debug, Clone)]
pub struct HistoryRoot {
    path: PathBuf,
}

impl HistoryRoot {
    /// Resolve the default `~/.claude/projects`. Errors if `$HOME`
    /// (or the platform-specific user home) cannot be determined.
    pub fn home() -> Result<Self> {
        let home = home_dir().ok_or_else(|| Error::History {
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

    /// List every project directory at the root.
    ///
    /// Returns an empty vec if the root directory doesn't exist
    /// (a fresh Claude Code install hasn't created `~/.claude/projects`
    /// yet). Errors only on filesystem failures other than "not found."
    pub fn list_projects(&self) -> Result<Vec<ProjectSummary>> {
        let entries = match fs::read_dir(&self.path) {
            Ok(it) => it,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(e) => return Err(e.into()),
        };

        let mut out = Vec::new();
        for entry in entries.flatten() {
            let ft = match entry.file_type() {
                Ok(ft) => ft,
                Err(_) => continue,
            };
            if !ft.is_dir() {
                continue;
            }
            let slug = entry.file_name().to_string_lossy().into_owned();
            let summary = summarize_project(&entry.path(), slug);
            out.push(summary);
        }
        out.sort_by(|a, b| a.slug.cmp(&b.slug));
        Ok(out)
    }

    /// List sessions, optionally filtered to one project's `slug`.
    ///
    /// When `slug` is `None`, walks every project directory and
    /// returns the union, sorted by session id.
    pub fn list_sessions(&self, slug: Option<&str>) -> Result<Vec<SessionSummary>> {
        let project_dirs = match slug {
            Some(s) => vec![self.path.join(s)],
            None => self
                .list_projects()?
                .into_iter()
                .map(|p| self.path.join(&p.slug))
                .collect(),
        };

        let mut out = Vec::new();
        for dir in project_dirs {
            let project_slug = dir
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            let entries = match fs::read_dir(&dir) {
                Ok(it) => it,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
                Err(e) => return Err(e.into()),
            };
            for entry in entries.flatten() {
                let path = entry.path();
                if path.extension().and_then(|s| s.to_str()) != Some("jsonl") {
                    continue;
                }
                let Some(session_id) = path
                    .file_stem()
                    .and_then(|s| s.to_str())
                    .map(str::to_string)
                else {
                    continue;
                };
                if let Some(summary) = summarize_session(&path, session_id, project_slug.clone()) {
                    out.push(summary);
                }
            }
        }
        out.sort_by(|a, b| a.session_id.cmp(&b.session_id));
        Ok(out)
    }

    /// Read one session's full entry log.
    ///
    /// Walks every project directory looking for `<session_id>.jsonl`.
    /// Errors with [`Error::History`] if no session file matches.
    /// Malformed lines are skipped with a tracing warning.
    pub fn read_session(&self, session_id: &str) -> Result<SessionLog> {
        let (path, project_slug) =
            self.find_session(session_id)?
                .ok_or_else(|| Error::History {
                    message: format!(
                        "no session with id `{session_id}` under {}",
                        self.path.display()
                    ),
                })?;
        parse_session(&path, session_id.to_string(), project_slug)
    }

    /// Locate the on-disk path for a session id, plus its project
    /// slug. Returns `Ok(None)` if no such session exists. Useful
    /// when a caller wants to read with non-default semantics
    /// (streaming, tailing, etc.) without going through
    /// [`Self::read_session`].
    pub fn find_session(&self, session_id: &str) -> Result<Option<(PathBuf, String)>> {
        for project in self.list_projects()? {
            let candidate = self
                .path
                .join(&project.slug)
                .join(format!("{session_id}.jsonl"));
            if candidate.is_file() {
                return Ok(Some((candidate, project.slug)));
            }
        }
        Ok(None)
    }
}

/// Summary of one project directory.
#[derive(Debug, Clone, Serialize)]
pub struct ProjectSummary {
    /// On-disk directory name (the encoded path).
    pub slug: String,
    /// Best-effort decode of the slug back to a filesystem path.
    /// See module docs for caveats.
    pub decoded_path: PathBuf,
    /// Number of `*.jsonl` files in the directory.
    pub session_count: usize,
    /// Latest filesystem modification time across the project's
    /// session files. None if the directory is empty or stats fail.
    pub last_modified: Option<SystemTime>,
}

/// Summary of one session's `.jsonl` file.
#[derive(Debug, Clone, Serialize)]
pub struct SessionSummary {
    /// Filename stem -- the session UUID Claude Code assigned.
    pub session_id: String,
    /// The owning project's slug (directory name).
    pub project_slug: String,
    /// Count of `user` + `assistant` entries (excludes
    /// queue-operation, attachment, ai-title, last-prompt, etc.).
    pub message_count: usize,
    /// First timestamp seen in the file (any entry type), as the
    /// raw string Claude Code wrote.
    pub first_timestamp: Option<String>,
    /// Last timestamp seen.
    pub last_timestamp: Option<String>,
    /// Auto-generated title if Claude Code emitted an `ai-title`
    /// entry; None otherwise.
    pub title: Option<String>,
    /// First ~160 chars of the first user message's text content,
    /// flattened to a single line. Useful as a fallback display name
    /// when `title` is None (which is most sessions today since
    /// claude-code only writes ai-titles intermittently). None when
    /// the session has no readable user message.
    pub first_user_preview: Option<String>,
    /// Sum of `message.usage.total_cost_usd` across every assistant
    /// entry. Always None on current claude-code (the field is written
    /// as `null`); kept in the shape so we can plumb it through if the
    /// upstream behavior changes. Use `total_tokens` for a usage proxy.
    pub total_cost_usd: Option<f64>,
    /// Sum of input + output + cache tokens across every assistant
    /// entry. None when the session has no assistant entries. Cheap to
    /// derive from `message.usage`, which claude-code DOES write.
    pub total_tokens: Option<u64>,
    /// File size in bytes.
    pub size_bytes: u64,
}

/// Full parsed session.
#[derive(Debug, Clone, Serialize)]
pub struct SessionLog {
    pub session_id: String,
    pub project_slug: String,
    pub entries: Vec<HistoryEntry>,
}

/// One parsed line from a session `.jsonl`.
///
/// Only `user` and `assistant` entry types get typed variants;
/// everything else (`queue-operation`, `attachment`, `ai-title`,
/// `last-prompt`, future types) lands in [`Self::Other`] with the
/// raw JSON for caller inspection.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum HistoryEntry {
    User {
        uuid: Option<String>,
        timestamp: Option<String>,
        cwd: Option<String>,
        git_branch: Option<String>,
        message: Value,
        #[serde(flatten)]
        rest: serde_json::Map<String, Value>,
    },
    Assistant {
        uuid: Option<String>,
        timestamp: Option<String>,
        message: Value,
        #[serde(flatten)]
        rest: serde_json::Map<String, Value>,
    },
    Other {
        /// The `type` field as Claude Code wrote it.
        type_tag: String,
        /// The full raw entry.
        raw: Value,
    },
}

// -- helpers --------------------------------------------------------

fn summarize_project(dir: &Path, slug: String) -> ProjectSummary {
    let mut session_count = 0usize;
    let mut last_modified: Option<SystemTime> = None;
    if let Ok(entries) = fs::read_dir(dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|s| s.to_str()) == Some("jsonl") {
                session_count += 1;
                if let Ok(meta) = entry.metadata()
                    && let Ok(mtime) = meta.modified()
                {
                    last_modified = Some(match last_modified {
                        Some(prev) if prev > mtime => prev,
                        _ => mtime,
                    });
                }
            }
        }
    }
    ProjectSummary {
        decoded_path: decode_slug(&slug),
        slug,
        session_count,
        last_modified,
    }
}

fn summarize_session(
    path: &Path,
    session_id: String,
    project_slug: String,
) -> Option<SessionSummary> {
    let meta = fs::metadata(path).ok()?;
    let size_bytes = meta.len();

    let file = fs::File::open(path).ok()?;
    let reader = BufReader::new(file);

    let mut message_count = 0usize;
    let mut first_timestamp = None;
    let mut last_timestamp = None;
    let mut title = None;
    let mut first_user_preview: Option<String> = None;
    let mut total_cost_usd: Option<f64> = None;
    let mut total_tokens: Option<u64> = None;

    for line in reader.lines().map_while(std::io::Result::ok) {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let v: Value = match serde_json::from_str(trimmed) {
            Ok(v) => v,
            Err(_) => continue,
        };
        let ty = v.get("type").and_then(Value::as_str).unwrap_or("");
        match ty {
            "user" => {
                message_count += 1;
                if first_user_preview.is_none()
                    && let Some(p) = extract_user_text_preview(&v, 160)
                {
                    first_user_preview = Some(p);
                }
            }
            "assistant" => {
                message_count += 1;
                if let Some(c) = v
                    .get("message")
                    .and_then(|m| m.get("usage"))
                    .and_then(|u| u.get("total_cost_usd"))
                    .and_then(Value::as_f64)
                {
                    *total_cost_usd.get_or_insert(0.0) += c;
                }
                if let Some(usage) = v.get("message").and_then(|m| m.get("usage")) {
                    // Sum every token bucket so cache + non-cache both count.
                    let mut t = 0u64;
                    for k in [
                        "input_tokens",
                        "output_tokens",
                        "cache_creation_input_tokens",
                        "cache_read_input_tokens",
                    ] {
                        if let Some(n) = usage.get(k).and_then(Value::as_u64) {
                            t += n;
                        }
                    }
                    if t > 0 {
                        *total_tokens.get_or_insert(0) += t;
                    }
                }
            }
            "ai-title" => {
                if let Some(t) = v.get("title").and_then(Value::as_str) {
                    title = Some(t.to_string());
                }
            }
            _ => {}
        }
        if let Some(ts) = v.get("timestamp").and_then(Value::as_str) {
            if first_timestamp.is_none() {
                first_timestamp = Some(ts.to_string());
            }
            last_timestamp = Some(ts.to_string());
        }
    }

    Some(SessionSummary {
        session_id,
        project_slug,
        message_count,
        first_timestamp,
        last_timestamp,
        title,
        first_user_preview,
        total_cost_usd,
        total_tokens,
        size_bytes,
    })
}

/// Pull a single-line, truncated preview out of a user-entry JSON.
/// Accepts both `message.content: "string"` and the structured form
/// `message.content: [{type:"text", text:"..."}, ...]`. Skips entries
/// where the first user "message" is actually a tool_result (those
/// happen when claude-code resumes a session that was mid-tool).
fn extract_user_text_preview(entry: &Value, max_chars: usize) -> Option<String> {
    let content = entry.get("message")?.get("content")?;
    let raw = if let Some(s) = content.as_str() {
        s.to_string()
    } else if let Some(arr) = content.as_array() {
        let mut buf = String::new();
        for block in arr {
            let ty = block.get("type").and_then(Value::as_str).unwrap_or("");
            if ty == "text"
                && let Some(t) = block.get("text").and_then(Value::as_str)
            {
                if !buf.is_empty() {
                    buf.push(' ');
                }
                buf.push_str(t);
            }
        }
        buf
    } else {
        return None;
    };
    let one_line = raw
        .split('\n')
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .collect::<Vec<_>>()
        .join(" ");
    if one_line.is_empty() {
        return None;
    }
    let truncated: String = one_line.chars().take(max_chars).collect();
    if truncated.len() < one_line.len() {
        Some(format!("{truncated}..."))
    } else {
        Some(truncated)
    }
}

fn parse_session(path: &Path, session_id: String, project_slug: String) -> Result<SessionLog> {
    let file = fs::File::open(path)?;
    let reader = BufReader::new(file);

    let mut entries = Vec::new();
    for (lineno, line) in reader.lines().enumerate() {
        let line = match line {
            Ok(l) => l,
            Err(e) => {
                tracing::warn!(
                    path = %path.display(),
                    line = lineno + 1,
                    error = %e,
                    "history: skipping unreadable line",
                );
                continue;
            }
        };
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        match parse_entry(trimmed) {
            Ok(entry) => entries.push(entry),
            Err(e) => {
                tracing::warn!(
                    path = %path.display(),
                    line = lineno + 1,
                    error = %e,
                    "history: skipping malformed line",
                );
            }
        }
    }
    Ok(SessionLog {
        session_id,
        project_slug,
        entries,
    })
}

fn parse_entry(line: &str) -> std::result::Result<HistoryEntry, serde_json::Error> {
    let mut value: Value = serde_json::from_str(line)?;
    let ty = value
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    match ty.as_str() {
        "user" => Ok(HistoryEntry::User {
            uuid: value.get("uuid").and_then(Value::as_str).map(String::from),
            timestamp: value
                .get("timestamp")
                .and_then(Value::as_str)
                .map(String::from),
            cwd: value.get("cwd").and_then(Value::as_str).map(String::from),
            git_branch: value
                .get("gitBranch")
                .and_then(Value::as_str)
                .map(String::from),
            message: value.get("message").cloned().unwrap_or(Value::Null),
            rest: take_object(&mut value),
        }),
        "assistant" => Ok(HistoryEntry::Assistant {
            uuid: value.get("uuid").and_then(Value::as_str).map(String::from),
            timestamp: value
                .get("timestamp")
                .and_then(Value::as_str)
                .map(String::from),
            message: value.get("message").cloned().unwrap_or(Value::Null),
            rest: take_object(&mut value),
        }),
        other => Ok(HistoryEntry::Other {
            type_tag: other.to_string(),
            raw: value,
        }),
    }
}

fn take_object(_value: &mut Value) -> serde_json::Map<String, Value> {
    // Currently we don't bother carrying "everything else" through;
    // callers needing the full raw form can re-read via Other or
    // file-level access. Keeps the typed surface small. Reserved
    // for future use if a typed-with-all-fields shape is wanted.
    serde_json::Map::new()
}

fn decode_slug(slug: &str) -> PathBuf {
    // Claude Code encodes paths by replacing `/` with `-`. The
    // decode is best-effort: drop a leading `-` and replace
    // remaining `-` with `/`. Paths containing literal dashes in
    // directory names won't round-trip; that's a known limitation.
    let body = slug.strip_prefix('-').unwrap_or(slug);
    PathBuf::from(format!("/{}", body.replace('-', "/")))
}

fn home_dir() -> Option<PathBuf> {
    // Avoid pulling the home crate just for this. $HOME on Unix,
    // %USERPROFILE% on Windows -- both honored by std::env::var.
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

    fn write_session(dir: &Path, session_id: &str, lines: &[&str]) -> PathBuf {
        let path = dir.join(format!("{session_id}.jsonl"));
        let mut f = fs::File::create(&path).expect("create jsonl");
        for line in lines {
            writeln!(f, "{line}").unwrap();
        }
        path
    }

    fn fixture_root() -> tempfile::TempDir {
        let tmp = tempfile::tempdir().expect("tempdir");
        // Project A: two sessions
        let a = tmp.path().join("-Users-josh-Code-projA");
        fs::create_dir_all(&a).unwrap();
        write_session(
            &a,
            "session-aaa",
            &[
                r#"{"type":"user","uuid":"u1","timestamp":"2026-01-01T00:00:00Z","cwd":"/Users/josh/Code/projA","gitBranch":"main","message":{"role":"user","content":"hello"}}"#,
                r#"{"type":"assistant","uuid":"a1","timestamp":"2026-01-01T00:00:01Z","message":{"role":"assistant","content":"hi"}}"#,
                r#"{"type":"queue-operation","operation":"enqueue","timestamp":"2026-01-01T00:00:02Z"}"#,
                r#"{"type":"ai-title","title":"hello world"}"#,
            ],
        );
        write_session(
            &a,
            "session-bbb",
            &[
                r#"{"type":"user","uuid":"u2","timestamp":"2026-01-02T00:00:00Z","message":{"role":"user","content":"second"}}"#,
            ],
        );
        // Project B: one session, with one malformed line we'll skip
        let b = tmp.path().join("-private-tmp-projB");
        fs::create_dir_all(&b).unwrap();
        write_session(
            &b,
            "session-ccc",
            &[
                r#"{"type":"user","uuid":"u3","timestamp":"2026-02-01T00:00:00Z","message":{"role":"user","content":"x"}}"#,
                r#"NOT VALID JSON"#,
                r#"{"type":"assistant","uuid":"a3","timestamp":"2026-02-01T00:00:01Z","message":{"role":"assistant","content":"y"}}"#,
            ],
        );
        tmp
    }

    #[test]
    fn list_projects_returns_directories_sorted_by_slug() {
        let tmp = fixture_root();
        let root = HistoryRoot::at(tmp.path());
        let projects = root.list_projects().expect("list projects");
        let slugs: Vec<&str> = projects.iter().map(|p| p.slug.as_str()).collect();
        assert_eq!(slugs, ["-Users-josh-Code-projA", "-private-tmp-projB"]);
    }

    #[test]
    fn list_projects_counts_sessions() {
        let tmp = fixture_root();
        let root = HistoryRoot::at(tmp.path());
        let projects = root.list_projects().expect("list");
        let a = projects.iter().find(|p| p.slug.contains("projA")).unwrap();
        let b = projects.iter().find(|p| p.slug.contains("projB")).unwrap();
        assert_eq!(a.session_count, 2);
        assert_eq!(b.session_count, 1);
    }

    #[test]
    fn list_projects_decodes_slug_to_filesystem_path() {
        let tmp = fixture_root();
        let root = HistoryRoot::at(tmp.path());
        let projects = root.list_projects().expect("list");
        let a = projects.iter().find(|p| p.slug.contains("projA")).unwrap();
        assert_eq!(a.decoded_path, PathBuf::from("/Users/josh/Code/projA"));
    }

    #[test]
    fn list_projects_returns_empty_when_root_missing() {
        let tmp = tempfile::tempdir().unwrap();
        let root = HistoryRoot::at(tmp.path().join("does-not-exist"));
        let projects = root.list_projects().expect("ok");
        assert!(projects.is_empty());
    }

    #[test]
    fn list_sessions_filtered_by_slug() {
        let tmp = fixture_root();
        let root = HistoryRoot::at(tmp.path());
        let sessions = root
            .list_sessions(Some("-Users-josh-Code-projA"))
            .expect("list");
        let ids: Vec<&str> = sessions.iter().map(|s| s.session_id.as_str()).collect();
        assert_eq!(ids, ["session-aaa", "session-bbb"]);
        assert!(
            sessions
                .iter()
                .all(|s| s.project_slug == "-Users-josh-Code-projA")
        );
    }

    #[test]
    fn list_sessions_unfiltered_returns_union() {
        let tmp = fixture_root();
        let root = HistoryRoot::at(tmp.path());
        let sessions = root.list_sessions(None).expect("list");
        assert_eq!(sessions.len(), 3);
    }

    #[test]
    fn session_summary_counts_only_user_and_assistant() {
        let tmp = fixture_root();
        let root = HistoryRoot::at(tmp.path());
        let sessions = root.list_sessions(Some("-Users-josh-Code-projA")).unwrap();
        let aaa = sessions
            .iter()
            .find(|s| s.session_id == "session-aaa")
            .unwrap();
        // 2 message entries (user + assistant); queue-operation and ai-title don't count.
        assert_eq!(aaa.message_count, 2);
        assert_eq!(aaa.title.as_deref(), Some("hello world"));
        assert_eq!(aaa.first_timestamp.as_deref(), Some("2026-01-01T00:00:00Z"));
    }

    #[test]
    fn read_session_returns_typed_entries_and_skips_malformed_lines() {
        let tmp = fixture_root();
        let root = HistoryRoot::at(tmp.path());
        let log = root.read_session("session-ccc").expect("read");
        assert_eq!(log.session_id, "session-ccc");
        assert_eq!(log.project_slug, "-private-tmp-projB");
        // 3 lines in the file; 1 is malformed; expect 2 entries.
        assert_eq!(log.entries.len(), 2);
        assert!(matches!(log.entries[0], HistoryEntry::User { .. }));
        assert!(matches!(log.entries[1], HistoryEntry::Assistant { .. }));
    }

    #[test]
    fn read_session_user_entry_carries_metadata() {
        let tmp = fixture_root();
        let root = HistoryRoot::at(tmp.path());
        let log = root.read_session("session-aaa").expect("read");
        match &log.entries[0] {
            HistoryEntry::User {
                uuid,
                timestamp,
                cwd,
                git_branch,
                ..
            } => {
                assert_eq!(uuid.as_deref(), Some("u1"));
                assert_eq!(timestamp.as_deref(), Some("2026-01-01T00:00:00Z"));
                assert_eq!(cwd.as_deref(), Some("/Users/josh/Code/projA"));
                assert_eq!(git_branch.as_deref(), Some("main"));
            }
            other => panic!("expected User entry, got {other:?}"),
        }
    }

    #[test]
    fn read_session_other_entry_preserves_type_tag_and_raw() {
        let tmp = fixture_root();
        let root = HistoryRoot::at(tmp.path());
        let log = root.read_session("session-aaa").expect("read");
        // Find the queue-operation entry.
        let queue_op = log
            .entries
            .iter()
            .find(|e| matches!(e, HistoryEntry::Other { type_tag, .. } if type_tag == "queue-operation"))
            .expect("queue-operation entry");
        if let HistoryEntry::Other { raw, .. } = queue_op {
            assert_eq!(raw["operation"], "enqueue");
        }
    }

    #[test]
    fn read_session_unknown_id_errors() {
        let tmp = fixture_root();
        let root = HistoryRoot::at(tmp.path());
        let err = root.read_session("not-a-real-session").unwrap_err();
        assert!(matches!(err, Error::History { .. }));
        assert!(format!("{err}").contains("no session with id"));
    }

    #[test]
    fn find_session_returns_none_for_unknown_id() {
        let tmp = fixture_root();
        let root = HistoryRoot::at(tmp.path());
        let found = root.find_session("nope").expect("ok");
        assert!(found.is_none());
    }

    #[test]
    fn find_session_locates_real_session() {
        let tmp = fixture_root();
        let root = HistoryRoot::at(tmp.path());
        let (path, slug) = root
            .find_session("session-ccc")
            .expect("ok")
            .expect("found");
        assert!(path.ends_with("session-ccc.jsonl"));
        assert_eq!(slug, "-private-tmp-projB");
    }

    #[test]
    fn decode_slug_round_trips_simple_paths() {
        assert_eq!(
            decode_slug("-Users-josh-Code-foo"),
            PathBuf::from("/Users/josh/Code/foo")
        );
        assert_eq!(decode_slug("-tmp-bar"), PathBuf::from("/tmp/bar"));
    }
}
