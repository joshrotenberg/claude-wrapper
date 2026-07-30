//! Read-side access to Claude Code's on-disk **task-tool** state.
//!
//! The task tools (TaskCreate / TaskUpdate) persist one JSON file
//! per task item under `~/.claude/tasks/<session-id>/<n>.json`:
//!
//! ```json
//! {
//!   "id": "1",
//!   "subject": "Untrack the local config artifact",
//!   "description": "...",
//!   "activeForm": "Untracking the local config artifact",
//!   "status": "completed",
//!   "blocks": [],
//!   "blockedBy": []
//! }
//! ```
//!
//! This module lists and parses them; it is read-only on purpose,
//! like the other introspection modules. The layout is undocumented
//! Claude Code internal state (observed against CLI 2.1.219) and
//! can change across CLI versions, so parsing is defensive: every
//! typed field is `Option`-shaped, a field with an unexpected JSON
//! type stays in [`Task::rest`], and unknown fields land there too.
//!
//! - [`TasksRoot::list_sessions`] -- which sessions have task
//!   lists, with counts.
//! - [`TasksRoot::list`] -- one session's tasks in numeric file
//!   order.
//!
//! # Example
//!
//! ```no_run
//! use claude_wrapper::tasks::TasksRoot;
//!
//! # fn example() -> claude_wrapper::Result<()> {
//! let root = TasksRoot::home()?;
//! for list in root.list_sessions()? {
//!     println!("{}: {} tasks", list.session_id, list.task_count);
//! }
//! # Ok(()) }
//! ```

use std::fs;
use std::path::{Path, PathBuf};

use serde::Serialize;
use serde_json::Value;

use crate::error::{Error, Result};

/// Root directory of Claude Code's task-tool state. Defaults to
/// `~/.claude/tasks`; override with [`TasksRoot::at`] for tests or
/// non-default installs.
#[derive(Debug, Clone)]
pub struct TasksRoot {
    path: PathBuf,
}

impl TasksRoot {
    /// Resolve the default `~/.claude/tasks`. Errors if `$HOME`
    /// (or the platform-specific user home) cannot be determined.
    pub fn home() -> Result<Self> {
        let home = home_dir().ok_or_else(|| Error::Artifacts {
            message: "could not determine user home directory".to_string(),
        })?;
        Ok(Self {
            path: home.join(".claude").join("tasks"),
        })
    }

    /// Use a specific path as the tasks root. Useful for tests
    /// (point at a tempdir) and for non-default installs.
    pub fn at(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    /// The configured root directory.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// List every session directory at the root, sorted by session
    /// id, with the number of task files in each. A missing root
    /// returns an empty vec.
    pub fn list_sessions(&self) -> Result<Vec<TaskListSummary>> {
        let entries = match fs::read_dir(&self.path) {
            Ok(it) => it,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(e) => return Err(e.into()),
        };
        let mut out = Vec::new();
        for entry in entries.flatten() {
            let dir = entry.path();
            if !dir.is_dir() {
                continue;
            }
            let Some(session_id) = dir.file_name().and_then(|s| s.to_str()) else {
                continue;
            };
            let task_count = task_files(&dir).len();
            out.push(TaskListSummary {
                session_id: session_id.to_string(),
                path: dir,
                task_count,
            });
        }
        out.sort_by(|a, b| a.session_id.cmp(&b.session_id));
        Ok(out)
    }

    /// List one session's tasks, sorted numerically by file stem
    /// (`1.json`, `2.json`, ..., `10.json`). A session without a
    /// task directory (or an unknown session id) returns an empty
    /// vec. Malformed files are skipped with a tracing warning.
    pub fn list(&self, session_id: &str) -> Result<Vec<Task>> {
        let dir = self.path.join(session_id);
        let mut files = task_files(&dir);
        files.sort_by_key(|p| {
            let stem = p
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or_default()
                .to_string();
            (stem.parse::<u64>().unwrap_or(u64::MAX), stem)
        });
        let mut out = Vec::new();
        for path in files {
            match parse_task_file(&path) {
                Ok(task) => out.push(task),
                Err(e) => tracing::warn!(?path, "skipping task file: {e}"),
            }
        }
        Ok(out)
    }
}

/// One session that has task-tool state, returned by
/// [`TasksRoot::list_sessions`].
#[derive(Debug, Clone, Serialize)]
pub struct TaskListSummary {
    /// The session id (the directory name).
    pub session_id: String,
    /// Absolute path of the session's task directory.
    pub path: PathBuf,
    /// Number of task files in the directory.
    pub task_count: usize,
}

/// One task item parsed from `<n>.json`.
#[derive(Debug, Clone, Serialize)]
pub struct Task {
    /// Task id, when present. Usually matches the file stem.
    pub id: Option<String>,
    /// Short imperative subject, when present.
    pub subject: Option<String>,
    /// Full description, when present.
    pub description: Option<String>,
    /// Present-continuous display form, when present.
    pub active_form: Option<String>,
    /// Status (`pending`, `in_progress`, `completed`, or anything
    /// future), carried as a plain string.
    pub status: Option<String>,
    /// Ids of tasks this one blocks, when present and well-formed.
    pub blocks: Option<Vec<String>>,
    /// Ids of tasks blocking this one, when present and well-formed.
    pub blocked_by: Option<Vec<String>>,
    /// Absolute path to the source `.json`.
    pub file_path: PathBuf,
    /// Any additional fields, keyed as Claude Code wrote them. A
    /// typed field with an unexpected JSON type also lands here.
    pub rest: serde_json::Map<String, Value>,
}

/// Task item files in a directory: direct children matching
/// `*.json`. Missing or unreadable directories yield an empty list.
fn task_files(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    if let Ok(entries) = fs::read_dir(dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_file() && path.extension().and_then(|s| s.to_str()) == Some("json") {
                out.push(path);
            }
        }
    }
    out
}

fn parse_task_file(path: &Path) -> Result<Task> {
    let content = fs::read_to_string(path)?;
    let value: Value = serde_json::from_str(&content).map_err(|e| Error::Artifacts {
        message: format!("task file {} is not valid JSON: {e}", path.display()),
    })?;
    let mut rest = match value {
        Value::Object(map) => map,
        _ => {
            return Err(Error::Artifacts {
                message: format!("task file {} is not a JSON object", path.display()),
            });
        }
    };
    Ok(Task {
        id: take_string(&mut rest, "id"),
        subject: take_string(&mut rest, "subject"),
        description: take_string(&mut rest, "description"),
        active_form: take_string(&mut rest, "activeForm"),
        status: take_string(&mut rest, "status"),
        blocks: take_string_array(&mut rest, "blocks"),
        blocked_by: take_string_array(&mut rest, "blockedBy"),
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

/// Remove `key` when it holds an array of strings; any other shape
/// stays in the map so it surfaces through `rest`.
fn take_string_array(map: &mut serde_json::Map<String, Value>, key: &str) -> Option<Vec<String>> {
    match map.remove(key) {
        Some(Value::Array(arr)) if arr.iter().all(Value::is_string) => Some(
            arr.into_iter()
                .filter_map(|v| match v {
                    Value::String(s) => Some(s),
                    _ => None,
                })
                .collect(),
        ),
        Some(other) => {
            map.insert(key.to_string(), other);
            None
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

    fn write_task(root: &Path, session: &str, stem: &str, contents: &str) {
        let dir = root.join(session);
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join(format!("{stem}.json")), contents).unwrap();
    }

    fn fixture_root() -> tempfile::TempDir {
        let tmp = tempfile::tempdir().expect("tempdir");
        write_task(
            tmp.path(),
            "session-x",
            "1",
            r#"{"id":"1","subject":"First","description":"d1","activeForm":"Doing first","status":"completed","blocks":["2"],"blockedBy":[],"futureField":7}"#,
        );
        write_task(
            tmp.path(),
            "session-x",
            "10",
            r#"{"id":"10","subject":"Tenth","status":"pending","blocks":"not-an-array"}"#,
        );
        write_task(
            tmp.path(),
            "session-x",
            "2",
            r#"{"id":"2","subject":"Second"}"#,
        );
        write_task(tmp.path(), "session-y", "1", r#"NOT JSON"#);
        tmp
    }

    #[test]
    fn list_sessions_counts_task_files() {
        let tmp = fixture_root();
        let root = TasksRoot::at(tmp.path());
        let sessions = root.list_sessions().expect("list");
        let ids: Vec<&str> = sessions.iter().map(|s| s.session_id.as_str()).collect();
        assert_eq!(ids, ["session-x", "session-y"]);
        assert_eq!(sessions[0].task_count, 3);
    }

    #[test]
    fn list_sorts_numerically_and_parses_fields() {
        let tmp = fixture_root();
        let root = TasksRoot::at(tmp.path());
        let tasks = root.list("session-x").expect("list");
        let ids: Vec<Option<&str>> = tasks.iter().map(|t| t.id.as_deref()).collect();
        // Numeric order: 1, 2, 10 (not lexical 1, 10, 2).
        assert_eq!(ids, [Some("1"), Some("2"), Some("10")]);
        let first = &tasks[0];
        assert_eq!(first.subject.as_deref(), Some("First"));
        assert_eq!(first.active_form.as_deref(), Some("Doing first"));
        assert_eq!(first.status.as_deref(), Some("completed"));
        assert_eq!(first.blocks.as_deref(), Some(["2".to_string()].as_slice()));
        assert_eq!(first.blocked_by.as_deref(), Some([].as_slice()));
        assert_eq!(first.rest["futureField"], 7);
    }

    #[test]
    fn mistyped_array_stays_in_rest() {
        let tmp = fixture_root();
        let root = TasksRoot::at(tmp.path());
        let tasks = root.list("session-x").expect("list");
        let tenth = tasks
            .iter()
            .find(|t| t.id.as_deref() == Some("10"))
            .unwrap();
        assert_eq!(tenth.blocks, None);
        assert_eq!(tenth.rest["blocks"], "not-an-array");
    }

    #[test]
    fn malformed_files_are_skipped() {
        let tmp = fixture_root();
        let root = TasksRoot::at(tmp.path());
        assert!(root.list("session-y").expect("ok").is_empty());
    }

    #[test]
    fn missing_root_and_unknown_session_read_empty() {
        let tmp = tempfile::tempdir().unwrap();
        let root = TasksRoot::at(tmp.path().join("does-not-exist"));
        assert!(root.list_sessions().expect("ok").is_empty());
        assert!(root.list("nope").expect("ok").is_empty());
    }
}
