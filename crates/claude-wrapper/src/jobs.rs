//! Read-side access to Claude Code's on-disk **background-job** state.
//!
//! Claude Code 2.1.x ships a supervisor daemon (`claude daemon run`)
//! that orchestrates background agent tasks launched via the
//! `claude agents` TUI. Per-task state lives under
//! `~/.claude/jobs/<short-id>/`:
//!
//! - `state.json` -- current snapshot: state, intent (original
//!   prompt), session id, link to the session JSONL, timestamps,
//!   auto-generated name, etc.
//! - `timeline.jsonl` -- append-only event log: at each state
//!   transition, the daemon writes a line carrying timestamp,
//!   new state, one-line detail, and (often) the full text body.
//!
//! The session content itself is a normal
//! `~/.claude/projects/<slug>/<session_id>.jsonl` -- the same
//! format [`crate::history`] already parses. Each job's
//! [`JobSummary::session_path`] points at it for direct cross-linking.
//!
//! This module is read-only on purpose. The dispatch protocol (how
//! the TUI launches new tasks) is undocumented and version-sensitive;
//! mirroring it would defeat the drift defenses we built. Hosts that
//! want to fire background work should keep using the agents TUI or
//! the wrapper's [`crate::duplex::DuplexSession`] machinery.
//!
//! # Example
//!
//! ```no_run
//! use claude_wrapper::jobs::JobsRoot;
//!
//! # fn example() -> claude_wrapper::Result<()> {
//! let root = JobsRoot::home()?;
//! for s in root.list()? {
//!     println!("{}  [{}]  {}", s.short_id, s.state, s.intent.as_deref().unwrap_or(""));
//! }
//! // Drill into one job's full timeline:
//! let job = root.get("90c961c7")?;
//! for event in &job.timeline {
//!     println!("{}  {:?}", event.at.as_deref().unwrap_or("?"), event.state);
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

/// Root directory of Claude Code's on-disk background-job state.
/// Defaults to `~/.claude/jobs`; override with [`JobsRoot::at`] for
/// tests or non-default installs.
#[derive(Debug, Clone)]
pub struct JobsRoot {
    path: PathBuf,
}

impl JobsRoot {
    /// Resolve the default `~/.claude/jobs`. Errors if the user
    /// home directory cannot be determined.
    pub fn home() -> Result<Self> {
        let home = home_dir().ok_or_else(|| Error::Artifacts {
            message: "could not determine user home directory".to_string(),
        })?;
        Ok(Self {
            path: home.join(".claude").join("jobs"),
        })
    }

    /// Use a specific path as the jobs root. Useful for tests
    /// (point at a tempdir) and for non-default installs.
    pub fn at(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    /// The configured root directory.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// List every job directory at the root, sorted by `short_id`.
    ///
    /// Returns an empty vec if the root directory doesn't exist (no
    /// background agents have been launched yet on this machine).
    /// Skips entries that aren't directories, that don't carry a
    /// `state.json`, or that fail to parse -- those contribute a
    /// tracing warning so silent skips are diagnosable.
    pub fn list(&self) -> Result<Vec<JobSummary>> {
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
                // Skip pins.json and any other top-level files.
                continue;
            }
            let short_id = entry.file_name().to_string_lossy().into_owned();
            let state_path = entry.path().join("state.json");
            if !state_path.exists() {
                // A spare worker dir without a task; skip silently.
                continue;
            }
            match parse_state(&state_path, &short_id) {
                Ok(summary) => out.push(summary),
                Err(e) => tracing::warn!(?state_path, "skipping job: {e}"),
            }
        }
        out.sort_by(|a, b| a.short_id.cmp(&b.short_id));
        Ok(out)
    }

    /// Read one job by short id (its `~/.claude/jobs/<short_id>/`
    /// directory name). Returns the full record including the
    /// parsed `timeline.jsonl`. Errors if no such directory exists
    /// or `state.json` is missing / malformed.
    pub fn get(&self, short_id: &str) -> Result<Job> {
        let dir = self.path.join(short_id);
        let state_path = dir.join("state.json");
        if !state_path.exists() {
            return Err(Error::Artifacts {
                message: format!("no job at {}", dir.display()),
            });
        }
        let summary = parse_state(&state_path, short_id)?;
        let timeline = parse_timeline(&dir.join("timeline.jsonl"));
        let raw_state =
            serde_json::from_str(&fs::read_to_string(&state_path)?).unwrap_or(Value::Null);
        Ok(Job {
            summary,
            timeline,
            raw_state,
        })
    }
}

/// Cheap metadata view of one background job, returned by
/// [`JobsRoot::list`]. Stripped of the timeline.
#[derive(Debug, Clone, Serialize)]
pub struct JobSummary {
    /// On-disk directory name (e.g. `"90c961c7"`). Canonical
    /// handle for [`JobsRoot::get`].
    pub short_id: String,
    /// Lifecycle state as reported by the daemon
    /// (`"running" | "done" | "killed" | "failed" | ...`).
    pub state: String,
    /// Daemon-assigned short id (typically matches `short_id` from
    /// the directory name, but kept separately because the daemon
    /// could in principle reorganize the directory layout).
    pub daemon_short: Option<String>,
    /// Backend kind (`"daemon"` for normal background agents). Free
    /// text -- expose as-is for forward-compat with future backends.
    pub backend: Option<String>,
    /// Auto-generated short title shown in the agents TUI
    /// (e.g. `"crow diet research"`). Optional; absent on freshly
    /// created jobs before the daemon names them.
    pub name: Option<String>,
    /// One-line summary the daemon writes at each state transition
    /// (often the result for terminal states).
    pub detail: Option<String>,
    /// Original prompt the user submitted (`"lets research the
    /// typical diet of crows"`). The most useful field for human
    /// scanning; absent only on weirdly malformed records.
    pub intent: Option<String>,
    /// Full Claude session ID. Used to look up the conversation
    /// JSONL via [`crate::history::HistoryRoot::read_session`].
    pub session_id: Option<String>,
    /// Absolute path to the session JSONL (`linkScanPath` in the
    /// raw record). Same file `claude_wrapper::history` parses.
    pub session_path: Option<PathBuf>,
    /// Working directory the agent ran in.
    pub cwd: Option<PathBuf>,
    /// Where the agent was originally dispatched from (may differ
    /// from `cwd` after the agent navigated).
    pub origin_cwd: Option<PathBuf>,
    /// ISO-8601 timestamp the job was created.
    pub created_at: Option<String>,
    /// ISO-8601 timestamp of the most recent state update.
    pub updated_at: Option<String>,
    /// ISO-8601 timestamp the job first reached a terminal state.
    /// `None` for still-running jobs.
    pub first_terminal_at: Option<String>,
    /// CLI version the worker reported running. Useful when
    /// debugging cross-version state issues.
    pub cli_version: Option<String>,
    /// Last filesystem modification time of `state.json`, as
    /// Unix-epoch seconds. Cheap fallback for sorting when
    /// `updated_at` is missing.
    pub state_mtime_secs: Option<u64>,
}

/// Full job record returned by [`JobsRoot::get`]. Carries the
/// summary, the parsed timeline, and the raw `state.json` value
/// for callers that want to drill into fields this module doesn't
/// type explicitly.
#[derive(Debug, Clone, Serialize)]
pub struct Job {
    /// Same shape as [`JobsRoot::list`] returns.
    pub summary: JobSummary,
    /// One entry per line in `timeline.jsonl`, in file order.
    pub timeline: Vec<JobEvent>,
    /// Verbatim parsed `state.json`. Use this to access fields not
    /// covered by [`JobSummary`] (e.g. `inFlight.tasks`,
    /// `respawnFlags`, `tempo`).
    pub raw_state: Value,
}

/// One timeline event. All fields optional because the daemon may
/// emit partial events (e.g. without `text`) and we'd rather pass
/// the structure through than fail the load.
#[derive(Debug, Clone, Serialize)]
pub struct JobEvent {
    /// ISO-8601 timestamp.
    pub at: Option<String>,
    /// State name at this point in the timeline.
    pub state: Option<String>,
    /// One-line detail (often the final result for terminal events).
    pub detail: Option<String>,
    /// Full text body (markdown, often quite long for `done`
    /// events). Distinct from `detail`: `detail` is a one-liner,
    /// `text` is the full content.
    pub text: Option<String>,
    /// Anything the daemon emits that doesn't fit the above. Lets
    /// future daemon fields show up without a wrapper update.
    pub extra: Value,
}

fn parse_state(path: &Path, short_id: &str) -> Result<JobSummary> {
    let raw = fs::read_to_string(path)?;
    let v: Value = serde_json::from_str(&raw).map_err(|e| Error::Artifacts {
        message: format!("parse {}: {e}", path.display()),
    })?;
    let state_mtime_secs = fs::metadata(path)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.duration_since(SystemTime::UNIX_EPOCH).ok())
        .map(|d| d.as_secs());
    Ok(JobSummary {
        short_id: short_id.to_string(),
        state: v
            .get("state")
            .and_then(Value::as_str)
            .unwrap_or("unknown")
            .to_string(),
        daemon_short: v
            .get("daemonShort")
            .and_then(Value::as_str)
            .map(str::to_string),
        backend: v.get("backend").and_then(Value::as_str).map(str::to_string),
        name: v.get("name").and_then(Value::as_str).map(str::to_string),
        detail: v.get("detail").and_then(Value::as_str).map(str::to_string),
        intent: v.get("intent").and_then(Value::as_str).map(str::to_string),
        session_id: v
            .get("sessionId")
            .and_then(Value::as_str)
            .map(str::to_string),
        session_path: v
            .get("linkScanPath")
            .and_then(Value::as_str)
            .map(PathBuf::from),
        cwd: v.get("cwd").and_then(Value::as_str).map(PathBuf::from),
        origin_cwd: v
            .get("originCwd")
            .and_then(Value::as_str)
            .map(PathBuf::from),
        created_at: v
            .get("createdAt")
            .and_then(Value::as_str)
            .map(str::to_string),
        updated_at: v
            .get("updatedAt")
            .and_then(Value::as_str)
            .map(str::to_string),
        first_terminal_at: v
            .get("firstTerminalAt")
            .and_then(Value::as_str)
            .map(str::to_string),
        cli_version: v
            .get("cliVersion")
            .and_then(Value::as_str)
            .map(str::to_string),
        state_mtime_secs,
    })
}

fn parse_timeline(path: &Path) -> Vec<JobEvent> {
    let Ok(file) = fs::File::open(path) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for (i, line) in BufReader::new(file).lines().enumerate() {
        let line = match line {
            Ok(s) => s,
            Err(e) => {
                tracing::warn!(?path, "timeline line {i}: read error: {e}");
                continue;
            }
        };
        if line.trim().is_empty() {
            continue;
        }
        match serde_json::from_str::<Value>(&line) {
            Ok(v) => out.push(JobEvent {
                at: v.get("at").and_then(Value::as_str).map(str::to_string),
                state: v.get("state").and_then(Value::as_str).map(str::to_string),
                detail: v.get("detail").and_then(Value::as_str).map(str::to_string),
                text: v.get("text").and_then(Value::as_str).map(str::to_string),
                extra: v,
            }),
            Err(e) => {
                tracing::warn!(?path, "timeline line {i}: parse error: {e}");
            }
        }
    }
    out
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

    /// Write a job dir at `root/<short_id>/` with the given state.json
    /// body and optional timeline.jsonl lines.
    fn write_job(root: &Path, short_id: &str, state_json: &str, timeline_lines: &[&str]) {
        let dir = root.join(short_id);
        fs::create_dir_all(&dir).expect("mkdir");
        fs::write(dir.join("state.json"), state_json).expect("write state.json");
        if !timeline_lines.is_empty() {
            let mut f = fs::File::create(dir.join("timeline.jsonl")).expect("create timeline");
            for line in timeline_lines {
                writeln!(f, "{line}").unwrap();
            }
        }
    }

    fn fixture_root() -> tempfile::TempDir {
        let tmp = tempfile::tempdir().expect("tempdir");
        // A done job with full state + timeline.
        write_job(
            tmp.path(),
            "aaaaaaaa",
            r#"{"state":"done","detail":"42","intent":"meaning of life",
                 "sessionId":"sess-aaa","linkScanPath":"/p/sess-aaa.jsonl",
                 "cwd":"/work","createdAt":"2026-05-15T01:00:00Z",
                 "updatedAt":"2026-05-15T01:01:00Z","firstTerminalAt":"2026-05-15T01:00:55Z",
                 "name":"meaning of life","backend":"daemon","cliVersion":"2.1.143",
                 "daemonShort":"aaaaaaaa","originCwd":"/work"}"#,
            &[
                r#"{"at":"2026-05-15T01:00:30Z","state":"running","detail":"thinking"}"#,
                r#"{"at":"2026-05-15T01:00:55Z","state":"done","detail":"42","text":"the answer is 42"}"#,
            ],
        );
        // A still-running job.
        write_job(
            tmp.path(),
            "bbbbbbbb",
            r#"{"state":"running","intent":"compute primes","sessionId":"sess-bbb"}"#,
            &[r#"{"at":"2026-05-15T02:00:00Z","state":"running","detail":"started"}"#],
        );
        // A job dir with no state.json (spare worker leftover); list() should skip.
        fs::create_dir_all(tmp.path().join("cccccccc")).unwrap();
        // A non-directory top-level file (the daemon's pins.json); list() should skip.
        fs::write(tmp.path().join("pins.json"), "[]").unwrap();
        // A job whose state.json is malformed; list() should skip with warn.
        write_job(tmp.path(), "deadbeef", "not valid json {{", &[]);
        tmp
    }

    #[test]
    fn list_returns_only_well_formed_jobs_sorted_by_short_id() {
        let tmp = fixture_root();
        let root = JobsRoot::at(tmp.path());
        let jobs = root.list().expect("list");
        let ids: Vec<&str> = jobs.iter().map(|j| j.short_id.as_str()).collect();
        assert_eq!(ids, ["aaaaaaaa", "bbbbbbbb"]);
    }

    #[test]
    fn list_missing_root_returns_empty() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let root = JobsRoot::at(tmp.path().join("does-not-exist"));
        assert!(root.list().expect("list").is_empty());
    }

    #[test]
    fn list_summary_carries_typed_fields() {
        let tmp = fixture_root();
        let root = JobsRoot::at(tmp.path());
        let jobs = root.list().expect("list");
        let s = jobs.iter().find(|j| j.short_id == "aaaaaaaa").unwrap();
        assert_eq!(s.state, "done");
        assert_eq!(s.intent.as_deref(), Some("meaning of life"));
        assert_eq!(s.session_id.as_deref(), Some("sess-aaa"));
        assert_eq!(s.session_path, Some(PathBuf::from("/p/sess-aaa.jsonl")));
        assert_eq!(s.cwd, Some(PathBuf::from("/work")));
        assert_eq!(s.name.as_deref(), Some("meaning of life"));
        assert_eq!(s.backend.as_deref(), Some("daemon"));
        assert_eq!(s.cli_version.as_deref(), Some("2.1.143"));
        assert_eq!(s.daemon_short.as_deref(), Some("aaaaaaaa"));
        assert_eq!(s.origin_cwd, Some(PathBuf::from("/work")));
        assert_eq!(s.created_at.as_deref(), Some("2026-05-15T01:00:00Z"));
        assert_eq!(s.updated_at.as_deref(), Some("2026-05-15T01:01:00Z"));
        assert_eq!(s.first_terminal_at.as_deref(), Some("2026-05-15T01:00:55Z"));
        assert!(s.state_mtime_secs.is_some());
    }

    #[test]
    fn list_running_job_has_no_first_terminal_at() {
        let tmp = fixture_root();
        let root = JobsRoot::at(tmp.path());
        let jobs = root.list().expect("list");
        let s = jobs.iter().find(|j| j.short_id == "bbbbbbbb").unwrap();
        assert_eq!(s.state, "running");
        assert!(s.first_terminal_at.is_none());
    }

    #[test]
    fn get_returns_full_record_with_timeline() {
        let tmp = fixture_root();
        let root = JobsRoot::at(tmp.path());
        let job = root.get("aaaaaaaa").expect("get");
        assert_eq!(job.summary.state, "done");
        assert_eq!(job.timeline.len(), 2);
        assert_eq!(job.timeline[0].state.as_deref(), Some("running"));
        assert_eq!(job.timeline[1].state.as_deref(), Some("done"));
        assert_eq!(job.timeline[1].text.as_deref(), Some("the answer is 42"));
        assert!(!job.raw_state.is_null());
    }

    #[test]
    fn get_no_timeline_returns_empty_vec() {
        // running job's timeline only has 1 line; spare leftover has none.
        // Build a fresh job with no timeline file.
        let tmp = tempfile::tempdir().expect("tempdir");
        write_job(
            tmp.path(),
            "ffffffff",
            r#"{"state":"queued","intent":"x","sessionId":"y"}"#,
            &[],
        );
        let root = JobsRoot::at(tmp.path());
        let job = root.get("ffffffff").expect("get");
        assert!(job.timeline.is_empty());
    }

    #[test]
    fn get_unknown_id_errors() {
        let tmp = fixture_root();
        let root = JobsRoot::at(tmp.path());
        let err = root.get("nope").unwrap_err();
        assert!(err.to_string().contains("no job"));
    }

    #[test]
    fn timeline_skips_malformed_lines_without_failing() {
        let tmp = tempfile::tempdir().expect("tempdir");
        write_job(
            tmp.path(),
            "mixed",
            r#"{"state":"done","intent":"x","sessionId":"y"}"#,
            &[
                r#"{"at":"t1","state":"running"}"#,
                r#"NOT VALID JSON"#,
                r#""#, // empty line
                r#"{"at":"t2","state":"done","text":"final"}"#,
            ],
        );
        let root = JobsRoot::at(tmp.path());
        let job = root.get("mixed").expect("get");
        assert_eq!(job.timeline.len(), 2);
        assert_eq!(job.timeline[0].at.as_deref(), Some("t1"));
        assert_eq!(job.timeline[1].at.as_deref(), Some("t2"));
        assert_eq!(job.timeline[1].text.as_deref(), Some("final"));
    }

    #[test]
    fn unknown_state_string_passes_through() {
        // Forward-compat: future daemon states shouldn't break us.
        let tmp = tempfile::tempdir().expect("tempdir");
        write_job(
            tmp.path(),
            "weirdstate",
            r#"{"state":"some-future-state","intent":"x","sessionId":"y"}"#,
            &[],
        );
        let root = JobsRoot::at(tmp.path());
        let job = root.get("weirdstate").expect("get");
        assert_eq!(job.summary.state, "some-future-state");
    }

    #[test]
    fn raw_state_preserves_unknown_fields() {
        let tmp = tempfile::tempdir().expect("tempdir");
        write_job(
            tmp.path(),
            "extras",
            r#"{"state":"done","intent":"x","sessionId":"y",
                 "futureField":{"nested":42},"tempo":"idle"}"#,
            &[],
        );
        let root = JobsRoot::at(tmp.path());
        let job = root.get("extras").expect("get");
        assert_eq!(job.raw_state["futureField"]["nested"], 42);
        assert_eq!(job.raw_state["tempo"], "idle");
    }

    #[test]
    fn missing_state_field_defaults_to_unknown() {
        let tmp = tempfile::tempdir().expect("tempdir");
        write_job(tmp.path(), "nostate", r#"{"intent":"x"}"#, &[]);
        let root = JobsRoot::at(tmp.path());
        let summary = &root.list().expect("list")[0];
        assert_eq!(summary.state, "unknown");
    }

    // -- live test against the real ~/.claude/jobs/ ----------------

    #[test]
    #[ignore = "reads the user's real ~/.claude/jobs; may be empty"]
    fn live_list_real_jobs_dir() {
        let root = JobsRoot::home().expect("home dir");
        // Just shape: no panics, returns a Vec, every entry has at
        // least a short_id and a state string.
        for s in root.list().expect("list") {
            assert!(!s.short_id.is_empty(), "empty short_id: {s:?}");
            assert!(!s.state.is_empty(), "empty state: {s:?}");
        }
    }
}
