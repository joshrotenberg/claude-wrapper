//! Read-side access to Claude Code's **live session registry**.
//!
//! Each running Claude Code process registers itself as
//! `~/.claude/sessions/<pid>.json`:
//!
//! ```json
//! {
//!   "pid": 31546,
//!   "sessionId": "a2000338-8786-49e7-be7b-3ffff9ce15e4",
//!   "cwd": "/path/to/project",
//!   "startedAt": 1785430795867,
//!   "version": "2.1.219",
//!   "kind": "interactive",
//!   "entrypoint": "claude-desktop",
//!   "name": "claude-wrapper-85"
//! }
//! ```
//!
//! The `sessionId` joins a running process to its transcript in
//! [`crate::history`]. This module is read-only on purpose, like
//! the other introspection modules. The layout is undocumented
//! Claude Code internal state (observed against CLI 2.1.219) and
//! can change across CLI versions, so parsing is defensive: every
//! typed field is `Option`-shaped, mistyped values stay in
//! [`LiveSession::rest`], and unknown fields land there too.
//!
//! # Staleness
//!
//! Registry files can outlive their process (a crash skips
//! cleanup), so an entry is evidence a session *was* running, not
//! proof it still is. Liveness is deliberately left to the caller:
//! check [`LiveSession::pid`] with a platform-appropriate probe if
//! it matters.
//!
//! # Example
//!
//! ```no_run
//! use claude_wrapper::sessions::SessionsRoot;
//!
//! # fn example() -> claude_wrapper::Result<()> {
//! let root = SessionsRoot::home()?;
//! for s in root.list()? {
//!     println!(
//!         "{} {} {}",
//!         s.pid.unwrap_or(0),
//!         s.name.as_deref().unwrap_or("?"),
//!         s.cwd.as_deref().unwrap_or("?"),
//!     );
//! }
//! # Ok(()) }
//! ```

use std::fs;
use std::path::{Path, PathBuf};

use serde::Serialize;
use serde_json::Value;

use crate::error::{Error, Result};

/// Root directory of Claude Code's live session registry. Defaults
/// to `~/.claude/sessions`; override with [`SessionsRoot::at`] for
/// tests or non-default installs.
#[derive(Debug, Clone)]
pub struct SessionsRoot {
    path: PathBuf,
}

impl SessionsRoot {
    /// Resolve the default `~/.claude/sessions`. Errors if `$HOME`
    /// (or the platform-specific user home) cannot be determined.
    pub fn home() -> Result<Self> {
        let home = home_dir().ok_or_else(|| Error::Artifacts {
            message: "could not determine user home directory".to_string(),
        })?;
        Ok(Self {
            path: home.join(".claude").join("sessions"),
        })
    }

    /// Use a specific path as the sessions root. Useful for tests
    /// (point at a tempdir) and for non-default installs.
    pub fn at(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    /// The configured root directory.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// List every registry entry, newest first (by `startedAt`,
    /// ties broken by pid). A missing root returns an empty vec;
    /// malformed files are skipped with a tracing warning. Entries
    /// may be stale -- see the module docs.
    pub fn list(&self) -> Result<Vec<LiveSession>> {
        let entries = match fs::read_dir(&self.path) {
            Ok(it) => it,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(e) => return Err(e.into()),
        };
        let mut out = Vec::new();
        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_file() || path.extension().and_then(|s| s.to_str()) != Some("json") {
                continue;
            }
            match parse_session_file(&path) {
                Ok(session) => out.push(session),
                Err(e) => tracing::warn!(?path, "skipping session registry file: {e}"),
            }
        }
        out.sort_by(|a, b| {
            b.started_at_ms
                .cmp(&a.started_at_ms)
                .then_with(|| a.pid.cmp(&b.pid))
        });
        Ok(out)
    }
}

/// One entry from the live session registry.
#[derive(Debug, Clone, Serialize)]
pub struct LiveSession {
    /// Process id, when present. May refer to an exited process --
    /// see the module docs on staleness.
    pub pid: Option<u64>,
    /// The session id; joins to [`crate::history`] transcripts.
    pub session_id: Option<String>,
    /// Working directory of the session.
    pub cwd: Option<String>,
    /// Epoch milliseconds when the session started.
    pub started_at_ms: Option<u64>,
    /// Claude Code version string.
    pub version: Option<String>,
    /// Session kind (e.g. `interactive`), when present.
    pub kind: Option<String>,
    /// The surface the session was started from (e.g. `cli`,
    /// `claude-desktop`), when present.
    pub entrypoint: Option<String>,
    /// Display name (e.g. `claude-wrapper-85`), when present.
    pub name: Option<String>,
    /// Absolute path to the source `.json`.
    pub file_path: PathBuf,
    /// Any additional fields, keyed as Claude Code wrote them. A
    /// typed field with an unexpected JSON type also lands here.
    pub rest: serde_json::Map<String, Value>,
}

fn parse_session_file(path: &Path) -> Result<LiveSession> {
    let content = fs::read_to_string(path)?;
    let value: Value = serde_json::from_str(&content).map_err(|e| Error::Artifacts {
        message: format!("session registry {} is not valid JSON: {e}", path.display()),
    })?;
    let mut rest = match value {
        Value::Object(map) => map,
        _ => {
            return Err(Error::Artifacts {
                message: format!("session registry {} is not a JSON object", path.display()),
            });
        }
    };
    Ok(LiveSession {
        pid: take_u64(&mut rest, "pid"),
        session_id: take_string(&mut rest, "sessionId"),
        cwd: take_string(&mut rest, "cwd"),
        started_at_ms: take_u64(&mut rest, "startedAt"),
        version: take_string(&mut rest, "version"),
        kind: take_string(&mut rest, "kind"),
        entrypoint: take_string(&mut rest, "entrypoint"),
        name: take_string(&mut rest, "name"),
        file_path: path.to_path_buf(),
        rest,
    })
}

/// Remove `key` when it holds a string; any other type stays in the
/// map so it surfaces through `rest`.
fn take_string(map: &mut serde_json::Map<String, Value>, key: &str) -> Option<String> {
    match map.remove(key) {
        Some(Value::String(s)) => Some(s),
        Some(other) => {
            map.insert(key.to_string(), other);
            None
        }
        None => None,
    }
}

/// Remove `key` when it holds an unsigned integer; any other type
/// stays in the map so it surfaces through `rest`.
fn take_u64(map: &mut serde_json::Map<String, Value>, key: &str) -> Option<u64> {
    match map.remove(key) {
        Some(v) => {
            let n = v.as_u64();
            if n.is_none() {
                map.insert(key.to_string(), v);
            }
            n
        }
        None => None,
    }
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

    fn write_entry(root: &Path, stem: &str, contents: &str) {
        fs::create_dir_all(root).unwrap();
        fs::write(root.join(format!("{stem}.json")), contents).unwrap();
    }

    fn fixture_root() -> tempfile::TempDir {
        let tmp = tempfile::tempdir().expect("tempdir");
        write_entry(
            tmp.path(),
            "100",
            r#"{"pid":100,"sessionId":"s-old","cwd":"/a","startedAt":1000,"version":"2.1.0","kind":"interactive","entrypoint":"cli","name":"old-1","peerProtocol":1}"#,
        );
        write_entry(
            tmp.path(),
            "200",
            r#"{"pid":200,"sessionId":"s-new","cwd":"/b","startedAt":2000,"entrypoint":"claude-desktop"}"#,
        );
        write_entry(tmp.path(), "bad", r#"[1,2,3]"#);
        tmp
    }

    #[test]
    fn list_sorts_newest_first_and_parses_fields() {
        let tmp = fixture_root();
        let root = SessionsRoot::at(tmp.path());
        let sessions = root.list().expect("list");
        assert_eq!(sessions.len(), 2);
        assert_eq!(sessions[0].session_id.as_deref(), Some("s-new"));
        let old = &sessions[1];
        assert_eq!(old.pid, Some(100));
        assert_eq!(old.cwd.as_deref(), Some("/a"));
        assert_eq!(old.started_at_ms, Some(1000));
        assert_eq!(old.kind.as_deref(), Some("interactive"));
        assert_eq!(old.entrypoint.as_deref(), Some("cli"));
        assert_eq!(old.name.as_deref(), Some("old-1"));
        assert_eq!(old.rest["peerProtocol"], 1);
    }

    #[test]
    fn non_object_entries_are_skipped() {
        let tmp = fixture_root();
        let root = SessionsRoot::at(tmp.path());
        // The "bad" entry is an array; only the two objects survive.
        assert_eq!(root.list().expect("list").len(), 2);
    }

    #[test]
    fn missing_root_reads_empty() {
        let tmp = tempfile::tempdir().unwrap();
        let root = SessionsRoot::at(tmp.path().join("does-not-exist"));
        assert!(root.list().expect("ok").is_empty());
    }

    #[test]
    fn mistyped_pid_stays_in_rest() {
        let tmp = tempfile::tempdir().unwrap();
        write_entry(tmp.path(), "1", r#"{"pid":"not-a-number","sessionId":"s"}"#);
        let root = SessionsRoot::at(tmp.path());
        let sessions = root.list().expect("list");
        assert_eq!(sessions[0].pid, None);
        assert_eq!(sessions[0].rest["pid"], "not-a-number");
    }
}
