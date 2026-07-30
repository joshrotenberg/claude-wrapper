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
//! # Session subdirectories
//!
//! Next to a session's `.jsonl`, Claude Code may write a directory
//! named after the session id holding sidechain state:
//!
//! ```text
//! projects/<slug>/<session_id>/
//!   subagents/agent-<id>.jsonl       subagent transcript
//!   subagents/agent-<id>.meta.json   per-agent metadata
//!   workflows/wf_<id>.json           workflow journal
//!   workflows/scripts/<name>-wf_<id>.js
//!   tool-results/<tool_use_id>.txt   spilled large tool output
//! ```
//!
//! [`HistoryRoot::list_subagents`] / [`HistoryRoot::read_subagent`]
//! parse sidechain transcripts with the same entry machinery as
//! [`HistoryRoot::read_session`]; [`HistoryRoot::list_tool_results`],
//! [`HistoryRoot::read_tool_result`], [`HistoryRoot::list_workflows`],
//! and [`HistoryRoot::read_workflow`] expose the rest. A session
//! without the subdirectory is the common case: the list methods
//! return empty vectors, not errors. This layout is undocumented
//! Claude Code internal state (observed against CLI 2.1.219) and can
//! change across CLI versions.
//!
//! # Global prompt history
//!
//! Separate from per-session transcripts, Claude Code appends one
//! record per typed prompt (across all projects) to
//! `~/.claude/history.jsonl`, a sibling of the projects root.
//! [`HistoryRoot::prompt_history`] reads it as typed
//! [`PromptHistoryEntry`] values: a pre-filtered stream of human
//! input with project and session join keys. Its retention differs
//! from session transcripts; transcripts remain the ground truth
//! for attribution.
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

/// Sort order for [`HistoryRoot::list_projects_with`] /
/// [`HistoryRoot::list_sessions_with`].
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum ListSort {
    /// Sort by the on-disk identifier alphabetically: slug for
    /// projects, session id for sessions. This is the default for
    /// the zero-arg [`HistoryRoot::list_projects`] /
    /// [`HistoryRoot::list_sessions`] methods to preserve the
    /// historical behavior of the pre-pagination API.
    #[default]
    NameAsc,
    /// Sort by most-recent activity, descending. For projects this
    /// is `last_modified` (filesystem mtime). For sessions this is
    /// `last_timestamp` (the last JSONL entry's `timestamp` field,
    /// compared lexicographically -- which matches chronological
    /// order for the ISO-8601 UTC strings Claude Code writes).
    /// Items with `None` last-time end up at the tail.
    RecencyDesc,
}

/// Filter + sort + paginate options for the listing methods.
///
/// `Default::default()` preserves the historical zero-arg behavior:
/// no limit, no offset, name-ascending sort, and **`include_empty
/// = true`** (everything is returned). Callers wanting paginated
/// or filtered output -- the typical case for the new `_with`
/// methods -- override the relevant fields explicitly.
#[derive(Debug, Clone)]
pub struct ListOptions {
    /// Max items to return after sorting + offset. `None` = no cap.
    pub limit: Option<usize>,
    /// Skip the first N items after sorting. Used with `limit` for
    /// pagination. `0` means "start from the first item."
    pub offset: usize,
    /// When `false`, drop entries with no real activity -- for
    /// projects, `session_count == 0`; for sessions, `message_count
    /// == 0` (the orphan stub files Claude Code sometimes leaves
    /// behind when a session never produced a turn). Default `true`
    /// so the zero-arg [`HistoryRoot::list_projects`] /
    /// [`HistoryRoot::list_sessions`] methods preserve their
    /// pre-pagination "include everything" behavior. New paginated
    /// callers (e.g. an MCP tool layer) should set this to `false`
    /// to hide orphan stub sessions and empty project directories.
    pub include_empty: bool,
    /// Sort order. See [`ListSort`].
    pub sort: ListSort,
}

impl Default for ListOptions {
    fn default() -> Self {
        Self {
            limit: None,
            offset: 0,
            include_empty: true,
            sort: ListSort::default(),
        }
    }
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

    /// List every project directory at the root, sorted by slug.
    ///
    /// Convenience wrapper around [`Self::list_projects_with`] with
    /// [`ListOptions::default`] (no limit, no offset, name-ascending
    /// sort, includes empty projects). Existing callers keep their
    /// behavior; new callers wanting pagination or recency sort
    /// should use [`Self::list_projects_with`].
    ///
    /// Returns an empty vec if the root directory doesn't exist.
    pub fn list_projects(&self) -> Result<Vec<ProjectSummary>> {
        self.list_projects_with(&ListOptions::default())
    }

    /// List project directories with filter / sort / pagination.
    ///
    /// Reads every direct child directory of the root, summarizes
    /// each, then applies (in order):
    ///
    /// 1. Filter out empty projects (`session_count == 0`) when
    ///    `opts.include_empty` is `false`.
    /// 2. Sort by `opts.sort` ([`ListSort::NameAsc`] by default,
    ///    [`ListSort::RecencyDesc`] for "most recent first").
    /// 3. Skip the first `opts.offset` items.
    /// 4. Truncate to `opts.limit` items.
    ///
    /// Returns an empty vec if the root directory doesn't exist.
    pub fn list_projects_with(&self, opts: &ListOptions) -> Result<Vec<ProjectSummary>> {
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
            if !opts.include_empty && summary.session_count == 0 {
                continue;
            }
            out.push(summary);
        }
        match opts.sort {
            ListSort::NameAsc => out.sort_by(|a, b| a.slug.cmp(&b.slug)),
            ListSort::RecencyDesc => out.sort_by(|a, b| {
                // None at the tail.
                match (a.last_modified, b.last_modified) {
                    (Some(am), Some(bm)) => bm.cmp(&am),
                    (Some(_), None) => std::cmp::Ordering::Less,
                    (None, Some(_)) => std::cmp::Ordering::Greater,
                    (None, None) => a.slug.cmp(&b.slug),
                }
            }),
        }
        apply_offset_limit(&mut out, opts);
        Ok(out)
    }

    /// List sessions, optionally filtered to one project's `slug`,
    /// sorted by session id.
    ///
    /// Convenience wrapper around [`Self::list_sessions_with`] with
    /// [`ListOptions::default`].
    pub fn list_sessions(&self, slug: Option<&str>) -> Result<Vec<SessionSummary>> {
        self.list_sessions_with(slug, &ListOptions::default())
    }

    /// List sessions with filter / sort / pagination.
    ///
    /// When `slug` is `Some`, only that project is walked. When
    /// `None`, every project directory is unioned. The options
    /// pipeline is the same as [`Self::list_projects_with`]:
    /// filter empty (`message_count == 0`) sessions unless
    /// `opts.include_empty`, sort, then offset + limit.
    pub fn list_sessions_with(
        &self,
        slug: Option<&str>,
        opts: &ListOptions,
    ) -> Result<Vec<SessionSummary>> {
        // Project enumeration here always wants every project (no
        // pagination), because we'll paginate the merged sessions.
        let enumerate_opts = ListOptions {
            include_empty: true,
            ..ListOptions::default()
        };
        let project_dirs = match slug {
            Some(s) => vec![self.path.join(s)],
            None => self
                .list_projects_with(&enumerate_opts)?
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
                    if !opts.include_empty && summary.message_count == 0 {
                        continue;
                    }
                    out.push(summary);
                }
            }
        }
        match opts.sort {
            ListSort::NameAsc => out.sort_by(|a, b| a.session_id.cmp(&b.session_id)),
            ListSort::RecencyDesc => out.sort_by(|a, b| {
                // ISO 8601 UTC strings sort lexicographically by time.
                // None at the tail.
                match (a.last_timestamp.as_deref(), b.last_timestamp.as_deref()) {
                    (Some(at), Some(bt)) => bt.cmp(at),
                    (Some(_), None) => std::cmp::Ordering::Less,
                    (None, Some(_)) => std::cmp::Ordering::Greater,
                    (None, None) => a.session_id.cmp(&b.session_id),
                }
            }),
        }
        apply_offset_limit(&mut out, opts);
        Ok(out)
    }

    /// Derive claude's project-directory slug for a filesystem path,
    /// matching the CLI exactly: the path is **canonicalized**
    /// (resolving symlinks -- e.g. `/var` -> `/private/var` on macOS,
    /// `/tmp` on Linux) and then every `/` and `.` is encoded as `-`.
    ///
    /// This is the forward complement of
    /// [`ProjectSummary::decoded_path`] and the reliable way to locate
    /// the project directory for a working directory -- see
    /// [`Self::sessions_for_path`]. Without the canonicalization and
    /// the `.`-encoding, a cwd under a symlinked root, or containing a
    /// `.` in a path segment, derives a slug that doesn't match what
    /// claude wrote, so enumeration finds nothing.
    ///
    /// Falls back to the path as given when it cannot be canonicalized
    /// (e.g. it does not exist on disk).
    #[must_use]
    pub fn project_slug(path: impl AsRef<Path>) -> String {
        let path = path.as_ref();
        let canonical = fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
        encode_path_slug(&canonical.to_string_lossy())
    }

    /// List sessions for a specific working directory, deriving its
    /// project slug via [`Self::project_slug`].
    ///
    /// This is the current-project enumeration entry point: it
    /// canonicalizes and encodes the cwd exactly as claude does, so
    /// sessions written from symlinked roots (`/tmp`, `/var`) or dotted
    /// path segments are found. Convenience over
    /// `list_sessions(Some(&HistoryRoot::project_slug(cwd)))`.
    pub fn sessions_for_path(&self, cwd: impl AsRef<Path>) -> Result<Vec<SessionSummary>> {
        self.sessions_for_path_with(cwd, &ListOptions::default())
    }

    /// [`Self::sessions_for_path`] with explicit [`ListOptions`].
    pub fn sessions_for_path_with(
        &self,
        cwd: impl AsRef<Path>,
        opts: &ListOptions,
    ) -> Result<Vec<SessionSummary>> {
        let slug = Self::project_slug(cwd);
        self.list_sessions_with(Some(&slug), opts)
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

    /// List the subagent (sidechain) transcripts recorded under a
    /// session's subdirectory, sorted by agent id.
    ///
    /// Returns an empty vector when the session has no subdirectory
    /// or no `subagents/` entries -- the common case. Errors with
    /// [`Error::History`] if the session id itself is unknown.
    pub fn list_subagents(&self, session_id: &str) -> Result<Vec<SubagentSummary>> {
        let Some(dir) = self.session_dir(session_id)? else {
            return Ok(Vec::new());
        };
        let mut out = Vec::new();
        for path in list_files_with_extension(&dir.join("subagents"), "jsonl") {
            // `agent-<id>.jsonl`; tolerate files without the prefix.
            let stem = match path.file_stem().and_then(|s| s.to_str()) {
                Some(s) => s,
                None => continue,
            };
            let agent_id = stem.strip_prefix("agent-").unwrap_or(stem).to_string();
            let meta = read_subagent_meta(&path);
            out.push(SubagentSummary {
                agent_id,
                path,
                meta,
            });
        }
        out.sort_by(|a, b| a.agent_id.cmp(&b.agent_id));
        Ok(out)
    }

    /// Parse one subagent transcript into typed [`HistoryEntry`]
    /// values, with the same liberal line-level parsing as
    /// [`Self::read_session`]. Subagent records carry the extra
    /// fields `agentId` and `isSidechain: true`, reachable through
    /// [`HistoryEntry::field`] / [`HistoryEntry::is_sidechain`].
    ///
    /// Errors with [`Error::History`] if the session id or agent id
    /// is unknown.
    pub fn read_subagent(&self, session_id: &str, agent_id: &str) -> Result<Vec<HistoryEntry>> {
        let found = self
            .list_subagents(session_id)?
            .into_iter()
            .find(|s| s.agent_id == agent_id)
            .ok_or_else(|| Error::History {
                message: format!("no subagent with id `{agent_id}` for session `{session_id}`"),
            })?;
        parse_jsonl_entries(&found.path)
    }

    /// List the spilled tool-result files recorded under a session's
    /// subdirectory, sorted by tool-use id.
    ///
    /// Claude Code spills large tool outputs to
    /// `tool-results/<tool_use_id>.txt` and references them from the
    /// transcript. Returns an empty vector when there are none.
    /// Errors with [`Error::History`] if the session id is unknown.
    pub fn list_tool_results(&self, session_id: &str) -> Result<Vec<ToolResultSummary>> {
        let Some(dir) = self.session_dir(session_id)? else {
            return Ok(Vec::new());
        };
        let mut out = Vec::new();
        for path in list_files_with_extension(&dir.join("tool-results"), "txt") {
            let Some(stem) = path.file_stem().and_then(|s| s.to_str()) else {
                continue;
            };
            let size_bytes = fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
            out.push(ToolResultSummary {
                tool_use_id: stem.to_string(),
                path,
                size_bytes,
            });
        }
        out.sort_by(|a, b| a.tool_use_id.cmp(&b.tool_use_id));
        Ok(out)
    }

    /// Read one spilled tool result's content.
    ///
    /// Errors with [`Error::History`] if the session id or tool-use
    /// id is unknown.
    pub fn read_tool_result(&self, session_id: &str, tool_use_id: &str) -> Result<String> {
        let found = self
            .list_tool_results(session_id)?
            .into_iter()
            .find(|t| t.tool_use_id == tool_use_id)
            .ok_or_else(|| Error::History {
                message: format!(
                    "no tool result with id `{tool_use_id}` for session `{session_id}`"
                ),
            })?;
        Ok(fs::read_to_string(&found.path)?)
    }

    /// List the workflow journals recorded under a session's
    /// subdirectory, sorted by workflow id.
    ///
    /// Each journal is a `workflows/wf_<id>.json` file; when a
    /// matching script (`workflows/scripts/<name>-wf_<id>.js`)
    /// exists its path is included. Returns an empty vector when
    /// there are none. Errors with [`Error::History`] if the
    /// session id is unknown.
    pub fn list_workflows(&self, session_id: &str) -> Result<Vec<WorkflowSummary>> {
        let Some(dir) = self.session_dir(session_id)? else {
            return Ok(Vec::new());
        };
        let workflows_dir = dir.join("workflows");
        let scripts: Vec<PathBuf> = list_files_with_extension(&workflows_dir.join("scripts"), "js");
        let mut out = Vec::new();
        for path in list_files_with_extension(&workflows_dir, "json") {
            let Some(stem) = path.file_stem().and_then(|s| s.to_str()) else {
                continue;
            };
            let workflow_id = stem.to_string();
            let script_suffix = format!("-{workflow_id}.js");
            let script_path = scripts
                .iter()
                .find(|p| {
                    p.file_name()
                        .and_then(|n| n.to_str())
                        .is_some_and(|n| n.ends_with(&script_suffix))
                })
                .cloned();
            let size_bytes = fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
            out.push(WorkflowSummary {
                workflow_id,
                path,
                script_path,
                size_bytes,
            });
        }
        out.sort_by(|a, b| a.workflow_id.cmp(&b.workflow_id));
        Ok(out)
    }

    /// Read one workflow journal as raw JSON.
    ///
    /// The journal shape is Claude Code internal state; it is
    /// returned as a [`serde_json::Value`] rather than typed.
    /// Errors with [`Error::History`] if the session id or workflow
    /// id is unknown.
    pub fn read_workflow(&self, session_id: &str, workflow_id: &str) -> Result<Value> {
        let found = self
            .list_workflows(session_id)?
            .into_iter()
            .find(|w| w.workflow_id == workflow_id)
            .ok_or_else(|| Error::History {
                message: format!("no workflow with id `{workflow_id}` for session `{session_id}`"),
            })?;
        let content = fs::read_to_string(&found.path)?;
        serde_json::from_str(&content).map_err(|e| Error::History {
            message: format!(
                "workflow journal `{}` is not valid JSON: {e}",
                found.path.display()
            ),
        })
    }

    /// Read the global prompt history: one entry per typed prompt,
    /// across all projects, in file (chronological) order.
    ///
    /// Claude Code appends to `~/.claude/history.jsonl`, a sibling
    /// of the projects root, each time the user submits a typed
    /// prompt. Only typed prompts land there, so this is a
    /// pre-filtered human-input stream with project and session
    /// join keys. Retention differs from session transcripts, and
    /// transcripts remain the ground truth for attribution.
    ///
    /// Returns an empty vector when the file does not exist.
    pub fn prompt_history(&self) -> Result<Vec<PromptHistoryEntry>> {
        self.prompt_history_with(&ListOptions::default())
    }

    /// [`Self::prompt_history`] with pagination. Only `offset` and
    /// `limit` apply; the other [`ListOptions`] fields are ignored.
    /// Malformed lines are skipped with a tracing warning.
    pub fn prompt_history_with(&self, opts: &ListOptions) -> Result<Vec<PromptHistoryEntry>> {
        let Some(parent) = self.path.parent() else {
            return Ok(Vec::new());
        };
        let file_path = parent.join("history.jsonl");
        if !file_path.is_file() {
            return Ok(Vec::new());
        }
        let file = fs::File::open(&file_path)?;
        let reader = BufReader::new(file);
        let mut entries = Vec::new();
        for (lineno, line) in reader.lines().enumerate() {
            let line = match line {
                Ok(l) => l,
                Err(e) => {
                    tracing::warn!(
                        path = %file_path.display(),
                        line = lineno + 1,
                        error = %e,
                        "history: skipping unreadable prompt-history line",
                    );
                    continue;
                }
            };
            let trimmed = line.trim();
            if trimmed.is_empty() {
                continue;
            }
            match parse_prompt_history_line(trimmed) {
                Ok(entry) => entries.push(entry),
                Err(e) => {
                    tracing::warn!(
                        path = %file_path.display(),
                        line = lineno + 1,
                        error = %e,
                        "history: skipping malformed prompt-history line",
                    );
                }
            }
        }
        apply_offset_limit(&mut entries, opts);
        Ok(entries)
    }

    /// The session subdirectory (`projects/<slug>/<session_id>/`),
    /// or `None` when the session exists but has no subdirectory.
    /// Errors with [`Error::History`] when the session id is
    /// unknown, so subdirectory lookups distinguish "session has no
    /// sidechain state" from "no such session".
    fn session_dir(&self, session_id: &str) -> Result<Option<PathBuf>> {
        let (jsonl_path, _slug) = self
            .find_session(session_id)?
            .ok_or_else(|| Error::History {
                message: format!(
                    "no session with id `{session_id}` under {}",
                    self.path.display()
                ),
            })?;
        let dir = match jsonl_path.parent() {
            Some(parent) => parent.join(session_id),
            None => return Ok(None),
        };
        Ok(dir.is_dir().then_some(dir))
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
    /// Whether `decoded_path` was verified against the real filesystem.
    ///
    /// `true` when the slug was disambiguated by checking `path.exists()` at
    /// each segment boundary. `false` when no filesystem path matched during
    /// decoding and the result is a naive `-`-to-`/` replacement.
    ///
    /// # Example
    ///
    /// ```rust
    /// # use claude_wrapper::history::ProjectSummary;
    /// // A real project: slug round-trips via filesystem check
    /// // is_decode_verified == true when the actual directory exists
    /// // is_decode_verified == false when decoding a slug for a path
    /// //   that no longer exists on disk
    /// ```
    pub is_decode_verified: bool,
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
    /// The session id (the `.jsonl` file stem).
    pub session_id: String,
    /// Slug of the project the session belongs to.
    pub project_slug: String,
    /// Every parsed entry, in file order.
    pub entries: Vec<HistoryEntry>,
}

/// One subagent (sidechain) transcript found under a session's
/// subdirectory.
#[derive(Debug, Clone, Serialize)]
pub struct SubagentSummary {
    /// Agent id (the file stem with its `agent-` prefix stripped).
    /// Matches the `agentId` field on the transcript's records.
    pub agent_id: String,
    /// Path to the transcript `.jsonl`.
    pub path: PathBuf,
    /// Parsed sibling `.meta.json`, when present and parseable.
    pub meta: Option<SubagentMeta>,
}

/// Metadata from a subagent's `.meta.json`.
#[derive(Debug, Clone, Serialize)]
pub struct SubagentMeta {
    /// Agent type (e.g. `general-purpose`), when present.
    pub agent_type: Option<String>,
    /// Human-readable task description, when present.
    pub description: Option<String>,
    /// The tool-use id of the spawning Agent/Task call, when present.
    pub tool_use_id: Option<String>,
    /// Nesting depth of the spawn, when present.
    pub spawn_depth: Option<u64>,
    /// Any additional fields, keyed as Claude Code wrote them.
    #[serde(flatten)]
    pub rest: serde_json::Map<String, Value>,
}

/// One spilled tool-result file under a session's subdirectory.
#[derive(Debug, Clone, Serialize)]
pub struct ToolResultSummary {
    /// The tool-use id (the file stem, e.g. `toolu_01...`), matching
    /// `tool_use_id` references in the transcript.
    pub tool_use_id: String,
    /// Path to the spill file.
    pub path: PathBuf,
    /// File size in bytes.
    pub size_bytes: u64,
}

/// One workflow journal under a session's subdirectory.
#[derive(Debug, Clone, Serialize)]
pub struct WorkflowSummary {
    /// The workflow run id (the journal file stem, e.g.
    /// `wf_a9a2673f-c5e`).
    pub workflow_id: String,
    /// Path to the journal `.json`.
    pub path: PathBuf,
    /// Path to the matching script under `workflows/scripts/`, when
    /// one exists.
    pub script_path: Option<PathBuf>,
    /// Journal file size in bytes.
    pub size_bytes: u64,
}

/// One record from the global prompt history file
/// (`~/.claude/history.jsonl`): a prompt the user typed, in any
/// project. See [`HistoryRoot::prompt_history`].
#[derive(Debug, Clone, Serialize)]
pub struct PromptHistoryEntry {
    /// The prompt text as displayed.
    pub display: Option<String>,
    /// Epoch milliseconds when the prompt was submitted. Note this
    /// file uses numeric timestamps, unlike the ISO-8601 strings in
    /// session transcripts.
    pub timestamp_ms: Option<u64>,
    /// Absolute path of the project the prompt was typed in.
    pub project: Option<String>,
    /// Session the prompt belongs to.
    pub session_id: Option<String>,
    /// Any additional fields (e.g. `pastedContents`), keyed as
    /// Claude Code wrote them.
    #[serde(flatten)]
    pub rest: serde_json::Map<String, Value>,
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
    /// A `user` entry: a prompt turn written by the user.
    User {
        /// Entry uuid, when present.
        uuid: Option<String>,
        /// ISO-8601 timestamp, when present.
        timestamp: Option<String>,
        /// Working directory recorded for the turn, when present.
        cwd: Option<String>,
        /// Git branch recorded for the turn, when present.
        git_branch: Option<String>,
        /// The raw `message` payload as Claude Code wrote it.
        message: Value,
        /// Every remaining field of the record, keyed as Claude Code
        /// wrote it (camelCase, e.g. `promptSource`). See
        /// [`HistoryEntry::field`] and the typed accessors for common
        /// lookups.
        #[serde(flatten)]
        rest: serde_json::Map<String, Value>,
    },
    /// An `assistant` entry: a model response turn.
    Assistant {
        /// Entry uuid, when present.
        uuid: Option<String>,
        /// ISO-8601 timestamp, when present.
        timestamp: Option<String>,
        /// The raw `message` payload as Claude Code wrote it.
        message: Value,
        /// Every remaining field of the record, keyed as Claude Code
        /// wrote it (camelCase, e.g. `requestId`). See
        /// [`HistoryEntry::field`] and the typed accessors for common
        /// lookups.
        #[serde(flatten)]
        rest: serde_json::Map<String, Value>,
    },
    /// Any other entry type, carried as raw JSON for caller inspection.
    Other {
        /// The `type` field as Claude Code wrote it.
        type_tag: String,
        /// The full raw entry.
        raw: Value,
    },
}

impl HistoryEntry {
    /// Parse a single transcript line into a typed entry.
    ///
    /// The per-line counterpart to [`HistoryRoot::read_session`]: use
    /// this when you already hold the line -- typically a consumer
    /// tailing append-only transcript files by byte offset, reading
    /// only the new bytes on each pass instead of re-reading whole
    /// session files.
    ///
    /// Parsing is liberal, matching `read_session`: entry types other
    /// than `user` and `assistant` come through as [`Self::Other`]
    /// carrying the raw value. A line that is not valid JSON is the
    /// one `Err` case -- the same lines `read_session` skips with a
    /// warning, left to the caller here.
    ///
    /// ```
    /// use claude_wrapper::history::HistoryEntry;
    ///
    /// let line = r#"{"type":"user","uuid":"u1","cwd":"/work/repo","promptSource":"typed","message":{"role":"user"}}"#;
    /// let entry = HistoryEntry::from_line(line)?;
    /// match &entry {
    ///     HistoryEntry::User { cwd, .. } => {
    ///         assert_eq!(cwd.as_deref(), Some("/work/repo"));
    ///     }
    ///     _ => unreachable!(),
    /// }
    /// assert_eq!(entry.prompt_source(), Some("typed"));
    /// # Ok::<(), serde_json::Error>(())
    /// ```
    pub fn from_line(line: &str) -> std::result::Result<Self, serde_json::Error> {
        parse_entry(line)
    }

    /// Raw access to a field by its name as Claude Code wrote it
    /// (camelCase, e.g. `"permissionMode"`).
    ///
    /// For [`Self::User`] and [`Self::Assistant`] this resolves
    /// against `rest`, so the fields promoted to struct fields
    /// (`uuid`, `timestamp`, `cwd`, `gitBranch`, `message`) are not
    /// reachable here. For [`Self::Other`] it resolves against the
    /// full raw entry.
    ///
    /// These records are Claude Code's data, not this crate's: a CLI
    /// update can add, drop, or retype any field, which is why this
    /// returns raw JSON and the typed accessors below are all
    /// `Option`-shaped.
    pub fn field(&self, key: &str) -> Option<&Value> {
        match self {
            Self::User { rest, .. } | Self::Assistant { rest, .. } => rest.get(key),
            Self::Other { raw, .. } => raw.get(key),
        }
    }

    /// How the prompt reached the session (`typed`, `queued`, `sdk`,
    /// `system`, `suggestion_accepted`), when recorded. Only user
    /// entries carrying an actual prompt have this; tool-result user
    /// turns generally do not.
    pub fn prompt_source(&self) -> Option<&str> {
        self.field("promptSource").and_then(Value::as_str)
    }

    /// The surface the session was driven from (`cli`,
    /// `claude-desktop`, `sdk-cli`), when recorded.
    pub fn entrypoint(&self) -> Option<&str> {
        self.field("entrypoint").and_then(Value::as_str)
    }

    /// Whether the entry is harness-injected context rather than a
    /// real turn. `None` when the field is absent.
    pub fn is_meta(&self) -> Option<bool> {
        self.field("isMeta").and_then(Value::as_bool)
    }

    /// Whether the entry belongs to a subagent sidechain. `None`
    /// when the field is absent.
    pub fn is_sidechain(&self) -> Option<bool> {
        self.field("isSidechain").and_then(Value::as_bool)
    }

    /// The session id recorded on the entry, when present.
    pub fn session_id(&self) -> Option<&str> {
        self.field("sessionId").and_then(Value::as_str)
    }

    /// The uuid of the parent entry, when present. Root entries carry
    /// `parentUuid: null`, which also reads as `None` here.
    pub fn parent_uuid(&self) -> Option<&str> {
        self.field("parentUuid").and_then(Value::as_str)
    }
}

// -- helpers --------------------------------------------------------

/// Apply offset + limit in-place to a sorted vec. Pulled out so the
/// project and session list paths share the same pagination logic.
fn apply_offset_limit<T>(items: &mut Vec<T>, opts: &ListOptions) {
    if opts.offset >= items.len() {
        items.clear();
        return;
    }
    if opts.offset > 0 {
        items.drain(..opts.offset);
    }
    if let Some(lim) = opts.limit
        && items.len() > lim
    {
        items.truncate(lim);
    }
}

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
    let (decoded_path, is_decode_verified) = decode_slug_anchored(&slug);
    ProjectSummary {
        decoded_path,
        is_decode_verified,
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
                // Claude Code writes this field as `aiTitle` (camelCase),
                // not `title`. Read both for resilience against future
                // renames -- whichever is present and non-empty wins.
                let candidate = v
                    .get("aiTitle")
                    .and_then(Value::as_str)
                    .or_else(|| v.get("title").and_then(Value::as_str));
                if let Some(t) = candidate
                    && !t.is_empty()
                {
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
    } else {
        let arr = content.as_array()?;
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
    let entries = parse_jsonl_entries(path)?;
    Ok(SessionLog {
        session_id,
        project_slug,
        entries,
    })
}

/// Parse a transcript `.jsonl` (mainline or subagent) into entries,
/// skipping unreadable and malformed lines with a tracing warning.
fn parse_jsonl_entries(path: &Path) -> Result<Vec<HistoryEntry>> {
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
    Ok(entries)
}

/// Non-recursive listing of files in `dir` with the given extension.
/// A missing or unreadable directory yields an empty list.
fn list_files_with_extension(dir: &Path, ext: &str) -> Vec<PathBuf> {
    let mut out = Vec::new();
    if let Ok(entries) = fs::read_dir(dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_file() && path.extension().and_then(|s| s.to_str()) == Some(ext) {
                out.push(path);
            }
        }
    }
    out
}

fn parse_prompt_history_line(
    line: &str,
) -> std::result::Result<PromptHistoryEntry, serde_json::Error> {
    let value: Value = serde_json::from_str(line)?;
    let mut rest = into_map(value);
    let timestamp_ms = match rest.remove("timestamp") {
        Some(v) => {
            let n = v.as_u64();
            if n.is_none() {
                rest.insert("timestamp".to_string(), v);
            }
            n
        }
        None => None,
    };
    Ok(PromptHistoryEntry {
        display: take_string(&mut rest, "display"),
        timestamp_ms,
        project: take_string(&mut rest, "project"),
        session_id: take_string(&mut rest, "sessionId"),
        rest,
    })
}

/// Read and parse the `.meta.json` next to a subagent transcript.
/// Absent or malformed metadata is `None`, not an error: the
/// transcript is the primary artifact.
fn read_subagent_meta(transcript_path: &Path) -> Option<SubagentMeta> {
    let meta_path = transcript_path.with_extension("meta.json");
    let content = fs::read_to_string(&meta_path).ok()?;
    let value: Value = serde_json::from_str(&content).ok()?;
    let mut rest = into_map(value);
    let spawn_depth = match rest.remove("spawnDepth") {
        Some(v) => {
            let n = v.as_u64();
            if n.is_none() {
                rest.insert("spawnDepth".to_string(), v);
            }
            n
        }
        None => None,
    };
    Some(SubagentMeta {
        agent_type: take_string(&mut rest, "agentType"),
        description: take_string(&mut rest, "description"),
        tool_use_id: take_string(&mut rest, "toolUseId"),
        spawn_depth,
        rest,
    })
}

fn parse_entry(line: &str) -> std::result::Result<HistoryEntry, serde_json::Error> {
    let value: Value = serde_json::from_str(line)?;
    let ty = value
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    match ty.as_str() {
        "user" => {
            let mut rest = into_map(value);
            rest.remove("type");
            Ok(HistoryEntry::User {
                uuid: take_string(&mut rest, "uuid"),
                timestamp: take_string(&mut rest, "timestamp"),
                cwd: take_string(&mut rest, "cwd"),
                git_branch: take_string(&mut rest, "gitBranch"),
                message: rest.remove("message").unwrap_or(Value::Null),
                rest,
            })
        }
        "assistant" => {
            let mut rest = into_map(value);
            rest.remove("type");
            Ok(HistoryEntry::Assistant {
                uuid: take_string(&mut rest, "uuid"),
                timestamp: take_string(&mut rest, "timestamp"),
                message: rest.remove("message").unwrap_or(Value::Null),
                rest,
            })
        }
        other => Ok(HistoryEntry::Other {
            type_tag: other.to_string(),
            raw: value,
        }),
    }
}

/// Unwrap a JSON value into its object map. The typed arms of
/// [`parse_entry`] only see objects (a non-object has no `type`
/// field and lands in `Other`), but stay defensive anyway.
fn into_map(value: Value) -> serde_json::Map<String, Value> {
    match value {
        Value::Object(map) => map,
        _ => serde_json::Map::new(),
    }
}

/// Remove `key` from the map when it holds a string. A value of any
/// other type is left in place so it surfaces through `rest` instead
/// of being silently dropped.
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

/// Decode a project slug back to a filesystem path, anchoring on the
/// real filesystem to disambiguate literal hyphens in directory names.
///
/// Claude Code encodes an absolute path by replacing each
/// non-alphanumeric character with `-` (see [`encode_path_slug`]). The
/// naive inverse (replace every `-` with `/`) is ambiguous: a `-` in the
/// slug could have been a `/`, `.`, `_`, space, or a literal hyphen in a
/// directory name -- like `claude-wrapper` -- making it indistinguishable
/// from a `/` boundary. This walks the slug left to right and, at each segment
/// boundary, checks the filesystem to decide whether the boundary is a
/// `/` (slash form) or a literal `-` (hyphen form).
///
/// Returns `(decoded_path, is_decode_verified)`. `is_decode_verified`
/// is `true` when every boundary was resolved against an existing path
/// and `false` when at least one boundary matched nothing on disk and
/// fell back to the naive split.
///
/// Tiebreak: when both forms exist, the deeper hyphenated form wins.
fn decode_slug_anchored(slug: &str) -> (PathBuf, bool) {
    let body = slug.strip_prefix('-').unwrap_or(slug);
    let mut segments = body.split('-');
    let mut built_path = PathBuf::from("/");
    let mut is_decode_verified = true;

    // First segment seeds the current component. An empty slug yields
    // an empty component and falls straight through to the final push.
    let mut current_component = segments.next().unwrap_or("").to_string();

    for next_segment in segments {
        let hyphen_component = format!("{current_component}-{next_segment}");
        let slash_exists = built_path.join(&current_component).exists();
        let hyphen_exists = built_path.join(&hyphen_component).exists();

        // Prefer the hyphen form whenever it exists (covers both the
        // hyphen-only case and the both-exist tiebreak). Otherwise take
        // the slash form, marking the decode unverified when neither
        // form is backed by a real path.
        if hyphen_exists {
            current_component = hyphen_component;
        } else {
            if !slash_exists {
                is_decode_verified = false;
            }
            built_path.push(&current_component);
            current_component = next_segment.to_string();
        }
    }

    built_path.push(&current_component);
    (built_path, is_decode_verified)
}

/// Encode an absolute filesystem path into claude's project-directory
/// slug: every non-alphanumeric character becomes `-` (so
/// `/private/var/T/tmp.X` becomes `-private-var-T-tmp-X`, and
/// `/Users/me/claude_wrapper` becomes `-Users-me-claude-wrapper`; the
/// leading `/` yields the leading `-`). This matches the Claude Code
/// CLI, which replaces every non-alphanumeric char -- including `_`,
/// spaces, and other separators -- when building the project-directory
/// name under `~/.claude/projects/`. Does not canonicalize -- see
/// [`HistoryRoot::project_slug`], which canonicalizes first.
fn encode_path_slug(path: &str) -> String {
    path.chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect()
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

    // Set the file mtime explicitly so recency-sort tests don't depend
    // on filesystem mtime granularity (Linux ext4 ticks at 1s by
    // default, so fixtures written back-to-back end up with identical
    // mtimes and the sort is non-deterministic).
    fn set_mtime(path: &Path, secs_since_epoch: u64) {
        let f = fs::OpenOptions::new()
            .write(true)
            .open(path)
            .expect("reopen for mtime");
        let when = SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(secs_since_epoch);
        f.set_modified(when).expect("set mtime");
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
                r#"{"type":"ai-title","aiTitle":"hello world"}"#,
            ],
        );
        write_session(
            &a,
            "session-bbb",
            &[
                r#"{"type":"user","uuid":"u2","timestamp":"2026-01-02T00:00:00Z","message":{"role":"user","content":"second"}}"#,
            ],
        );
        // session-aaa also has a session subdirectory with sidechain
        // state; session-bbb deliberately has none.
        let sub = a.join("session-aaa");
        fs::create_dir_all(sub.join("subagents")).unwrap();
        write_session(
            &sub.join("subagents"),
            "agent-abc123",
            &[
                r#"{"type":"user","uuid":"su1","agentId":"abc123","isSidechain":true,"message":{"role":"user","content":"subtask"}}"#,
                r#"{"type":"assistant","uuid":"sa1","agentId":"abc123","isSidechain":true,"message":{"role":"assistant","content":"done"}}"#,
            ],
        );
        fs::write(
            sub.join("subagents").join("agent-abc123.meta.json"),
            r#"{"agentType":"general-purpose","description":"audit the crate","toolUseId":"toolu_spawn1","spawnDepth":1,"futureField":"kept"}"#,
        )
        .unwrap();
        fs::create_dir_all(sub.join("tool-results")).unwrap();
        fs::write(
            sub.join("tool-results").join("toolu_r1.txt"),
            "spilled tool output",
        )
        .unwrap();
        fs::create_dir_all(sub.join("workflows").join("scripts")).unwrap();
        fs::write(
            sub.join("workflows").join("wf_run1.json"),
            r#"{"runId":"wf_run1","script":"export const meta = {}"}"#,
        )
        .unwrap();
        fs::write(
            sub.join("workflows")
                .join("scripts")
                .join("my-task-wf_run1.js"),
            "export const meta = {}",
        )
        .unwrap();
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
    fn parse_entry_populates_rest_with_unmodeled_fields() {
        let line = r#"{"type":"user","uuid":"u1","timestamp":"2026-01-01T00:00:00Z","cwd":"/w","gitBranch":"main","message":{"role":"user","content":"hi"},"promptSource":"typed","entrypoint":"cli","isSidechain":false,"sessionId":"s1","parentUuid":null,"permissionMode":"default","version":"2.1.0"}"#;
        let entry = parse_entry(line).expect("parse");
        match &entry {
            HistoryEntry::User { rest, .. } => {
                assert_eq!(rest["promptSource"], "typed");
                assert_eq!(rest["permissionMode"], "default");
                assert_eq!(rest["version"], "2.1.0");
                // Consumed fields must not reappear in rest.
                for consumed in ["type", "uuid", "timestamp", "cwd", "gitBranch", "message"] {
                    assert!(!rest.contains_key(consumed), "{consumed} leaked into rest");
                }
            }
            other => panic!("expected User entry, got {other:?}"),
        }
    }

    #[test]
    fn parse_entry_keeps_mistyped_field_in_rest() {
        // A `uuid` that isn't a string can't fill the typed field, but
        // it must survive in rest rather than being dropped.
        let line = r#"{"type":"assistant","uuid":42,"message":{"role":"assistant","content":"y"}}"#;
        let entry = parse_entry(line).expect("parse");
        match &entry {
            HistoryEntry::Assistant { uuid, rest, .. } => {
                assert_eq!(uuid.as_deref(), None);
                assert_eq!(rest["uuid"], 42);
            }
            other => panic!("expected Assistant entry, got {other:?}"),
        }
    }

    #[test]
    fn from_line_is_the_public_face_of_parse_entry() {
        // Typed, unknown-type, and malformed lines all behave exactly
        // as they do through read_session's internal parser.
        let user = r#"{"type":"user","uuid":"u1","cwd":"/w","message":{"role":"user"}}"#;
        match HistoryEntry::from_line(user).expect("parse") {
            HistoryEntry::User { uuid, cwd, .. } => {
                assert_eq!(uuid.as_deref(), Some("u1"));
                assert_eq!(cwd.as_deref(), Some("/w"));
            }
            other => panic!("expected User entry, got {other:?}"),
        }

        let unknown = r#"{"type":"ai-title","title":"t"}"#;
        match HistoryEntry::from_line(unknown).expect("parse") {
            HistoryEntry::Other { type_tag, raw } => {
                assert_eq!(type_tag, "ai-title");
                assert_eq!(raw["title"], "t");
            }
            other => panic!("expected Other entry, got {other:?}"),
        }

        assert!(HistoryEntry::from_line("not json").is_err());
    }

    #[test]
    fn typed_accessors_resolve_from_rest() {
        let line = r#"{"type":"user","uuid":"u1","message":{},"promptSource":"sdk","entrypoint":"sdk-cli","isSidechain":true,"isMeta":true,"sessionId":"s1","parentUuid":"p1"}"#;
        let entry = parse_entry(line).expect("parse");
        assert_eq!(entry.prompt_source(), Some("sdk"));
        assert_eq!(entry.entrypoint(), Some("sdk-cli"));
        assert_eq!(entry.is_sidechain(), Some(true));
        assert_eq!(entry.is_meta(), Some(true));
        assert_eq!(entry.session_id(), Some("s1"));
        assert_eq!(entry.parent_uuid(), Some("p1"));
        assert_eq!(
            entry.field("promptSource").and_then(Value::as_str),
            Some("sdk")
        );
    }

    #[test]
    fn typed_accessors_return_none_when_fields_absent() {
        let line = r#"{"type":"assistant","uuid":"a1","message":{},"parentUuid":null}"#;
        let entry = parse_entry(line).expect("parse");
        assert_eq!(entry.prompt_source(), None);
        assert_eq!(entry.entrypoint(), None);
        assert_eq!(entry.is_meta(), None);
        assert_eq!(entry.is_sidechain(), None);
        assert_eq!(entry.session_id(), None);
        // parentUuid: null reads as None, same as absent.
        assert_eq!(entry.parent_uuid(), None);
        assert_eq!(entry.field("noSuchField"), None);
    }

    #[test]
    fn field_and_accessors_resolve_on_other_variant() {
        let line = r#"{"type":"queue-operation","operation":"enqueue","sessionId":"s9"}"#;
        let entry = parse_entry(line).expect("parse");
        assert_eq!(
            entry.field("operation").and_then(Value::as_str),
            Some("enqueue")
        );
        assert_eq!(entry.session_id(), Some("s9"));
        assert_eq!(entry.prompt_source(), None);
    }

    #[test]
    fn serialized_entry_reemits_rest_fields_at_top_level() {
        let line = r#"{"type":"user","uuid":"u1","message":{},"promptSource":"typed"}"#;
        let entry = parse_entry(line).expect("parse");
        let v = serde_json::to_value(&entry).expect("serialize");
        assert_eq!(v["kind"], "user");
        assert_eq!(v["promptSource"], "typed");
    }

    // Prompt-history fixture: `<tmp>/projects` is the root and
    // `<tmp>/history.jsonl` sits next to it, mirroring ~/.claude.
    fn prompt_history_fixture(lines: &[&str]) -> (tempfile::TempDir, HistoryRoot) {
        let tmp = tempfile::tempdir().expect("tempdir");
        let projects = tmp.path().join("projects");
        fs::create_dir_all(&projects).unwrap();
        let mut f = fs::File::create(tmp.path().join("history.jsonl")).unwrap();
        for line in lines {
            writeln!(f, "{line}").unwrap();
        }
        let root = HistoryRoot::at(projects);
        (tmp, root)
    }

    #[test]
    fn prompt_history_parses_fields_and_keeps_rest() {
        let (_tmp, root) = prompt_history_fixture(&[
            r#"{"display":"fix the tests","pastedContents":{"1":"snippet"},"timestamp":1781804723955,"project":"/Users/me/Code/projA","sessionId":"s1","futureField":true}"#,
        ]);
        let entries = root.prompt_history().expect("read");
        assert_eq!(entries.len(), 1);
        let e = &entries[0];
        assert_eq!(e.display.as_deref(), Some("fix the tests"));
        assert_eq!(e.timestamp_ms, Some(1781804723955));
        assert_eq!(e.project.as_deref(), Some("/Users/me/Code/projA"));
        assert_eq!(e.session_id.as_deref(), Some("s1"));
        assert_eq!(e.rest["pastedContents"]["1"], "snippet");
        assert_eq!(e.rest["futureField"], true);
        for consumed in ["display", "timestamp", "project", "sessionId"] {
            assert!(
                !e.rest.contains_key(consumed),
                "{consumed} leaked into rest"
            );
        }
    }

    #[test]
    fn prompt_history_keeps_mistyped_timestamp_in_rest() {
        let (_tmp, root) =
            prompt_history_fixture(&[r#"{"display":"x","timestamp":"not-a-number"}"#]);
        let entries = root.prompt_history().expect("read");
        assert_eq!(entries[0].timestamp_ms, None);
        assert_eq!(entries[0].rest["timestamp"], "not-a-number");
    }

    #[test]
    fn prompt_history_skips_malformed_lines_and_paginates() {
        let (_tmp, root) = prompt_history_fixture(&[
            r#"{"display":"one","timestamp":1}"#,
            r#"NOT JSON"#,
            r#"{"display":"two","timestamp":2}"#,
            r#"{"display":"three","timestamp":3}"#,
        ]);
        let all = root.prompt_history().expect("read");
        assert_eq!(all.len(), 3);
        // File (chronological) order preserved.
        assert_eq!(all[0].display.as_deref(), Some("one"));
        let page = root
            .prompt_history_with(&ListOptions {
                offset: 1,
                limit: Some(1),
                ..ListOptions::default()
            })
            .expect("read");
        assert_eq!(page.len(), 1);
        assert_eq!(page[0].display.as_deref(), Some("two"));
    }

    #[test]
    fn prompt_history_missing_file_is_empty() {
        let tmp = tempfile::tempdir().unwrap();
        let projects = tmp.path().join("projects");
        fs::create_dir_all(&projects).unwrap();
        // No history.jsonl next to the projects root.
        let root = HistoryRoot::at(projects);
        assert!(root.prompt_history().expect("ok").is_empty());
    }

    #[test]
    fn list_subagents_parses_ids_and_meta() {
        let tmp = fixture_root();
        let root = HistoryRoot::at(tmp.path());
        let subs = root.list_subagents("session-aaa").expect("list");
        assert_eq!(subs.len(), 1);
        let sub = &subs[0];
        assert_eq!(sub.agent_id, "abc123");
        let meta = sub.meta.as_ref().expect("meta parsed");
        assert_eq!(meta.agent_type.as_deref(), Some("general-purpose"));
        assert_eq!(meta.description.as_deref(), Some("audit the crate"));
        assert_eq!(meta.tool_use_id.as_deref(), Some("toolu_spawn1"));
        assert_eq!(meta.spawn_depth, Some(1));
        assert_eq!(meta.rest["futureField"], "kept");
    }

    #[test]
    fn read_subagent_entries_carry_sidechain_attribution() {
        let tmp = fixture_root();
        let root = HistoryRoot::at(tmp.path());
        let entries = root.read_subagent("session-aaa", "abc123").expect("read");
        assert_eq!(entries.len(), 2);
        assert!(matches!(entries[0], HistoryEntry::User { .. }));
        assert_eq!(entries[0].is_sidechain(), Some(true));
        assert_eq!(
            entries[0].field("agentId").and_then(Value::as_str),
            Some("abc123")
        );
    }

    #[test]
    fn read_subagent_unknown_agent_errors() {
        let tmp = fixture_root();
        let root = HistoryRoot::at(tmp.path());
        let err = root.read_subagent("session-aaa", "nope").unwrap_err();
        assert!(format!("{err}").contains("no subagent with id"));
    }

    #[test]
    fn session_without_subdirectory_lists_empty() {
        let tmp = fixture_root();
        let root = HistoryRoot::at(tmp.path());
        assert!(root.list_subagents("session-bbb").expect("ok").is_empty());
        assert!(
            root.list_tool_results("session-bbb")
                .expect("ok")
                .is_empty()
        );
        assert!(root.list_workflows("session-bbb").expect("ok").is_empty());
    }

    #[test]
    fn subdirectory_lookups_error_on_unknown_session() {
        let tmp = fixture_root();
        let root = HistoryRoot::at(tmp.path());
        let err = root.list_subagents("not-a-session").unwrap_err();
        assert!(matches!(err, Error::History { .. }));
        assert!(format!("{err}").contains("no session with id"));
    }

    #[test]
    fn tool_result_list_and_read_round_trip() {
        let tmp = fixture_root();
        let root = HistoryRoot::at(tmp.path());
        let results = root.list_tool_results("session-aaa").expect("list");
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].tool_use_id, "toolu_r1");
        assert_eq!(results[0].size_bytes, "spilled tool output".len() as u64);
        let content = root
            .read_tool_result("session-aaa", "toolu_r1")
            .expect("read");
        assert_eq!(content, "spilled tool output");
        let err = root
            .read_tool_result("session-aaa", "toolu_nope")
            .unwrap_err();
        assert!(format!("{err}").contains("no tool result with id"));
    }

    #[test]
    fn workflow_list_links_script_and_read_parses_json() {
        let tmp = fixture_root();
        let root = HistoryRoot::at(tmp.path());
        let flows = root.list_workflows("session-aaa").expect("list");
        assert_eq!(flows.len(), 1);
        assert_eq!(flows[0].workflow_id, "wf_run1");
        let script = flows[0].script_path.as_ref().expect("script linked");
        assert!(script.ends_with("my-task-wf_run1.js"));
        let journal = root.read_workflow("session-aaa", "wf_run1").expect("read");
        assert_eq!(journal["runId"], "wf_run1");
        let err = root.read_workflow("session-aaa", "wf_nope").unwrap_err();
        assert!(format!("{err}").contains("no workflow with id"));
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
    fn decode_slug_anchored_no_hyphens_in_components() {
        // Path with no literal hyphens -- both forms are structurally
        // identical at each boundary, so the algorithm picks the slash
        // (naive) form at each step. `is_decode_verified` depends on
        // whether /a/b/c/d exists; in CI it won't, so only assert shape.
        let (path, _verified) = decode_slug_anchored("-a-b-c-d");
        assert_eq!(path, PathBuf::from("/a/b/c/d"));
    }

    #[test]
    fn decode_slug_anchored_single_hyphenated_segment() {
        // Build a real dir: tmp/foo-bar, then construct its slug.
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("foo-bar");
        fs::create_dir_all(&dir).unwrap();
        let tmp_str = tmp.path().to_string_lossy();
        let tmp_encoded = tmp_str.trim_start_matches('/').replace('/', "-");
        let slug = format!("-{tmp_encoded}-foo-bar");
        let expected = tmp.path().join("foo-bar");
        let (decoded, is_verified) = decode_slug_anchored(&slug);
        assert_eq!(decoded, expected);
        assert!(is_verified);
    }

    #[test]
    fn decode_slug_anchored_multiple_hyphenated_segments() {
        // Build: tmp/foo-bar/baz-qux
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("foo-bar").join("baz-qux");
        fs::create_dir_all(&dir).unwrap();
        let tmp_str = tmp.path().to_string_lossy();
        let tmp_encoded = tmp_str.trim_start_matches('/').replace('/', "-");
        let slug = format!("-{tmp_encoded}-foo-bar-baz-qux");
        let expected = tmp.path().join("foo-bar").join("baz-qux");
        let (decoded, is_verified) = decode_slug_anchored(&slug);
        assert_eq!(decoded, expected);
        assert!(is_verified);
    }

    #[test]
    fn decode_slug_anchored_fallback_when_nothing_exists() {
        // No filesystem paths exist for this slug -- falls back to naive.
        let (path, verified) = decode_slug_anchored("-nonexistent-xyz-abc-def");
        assert_eq!(path, PathBuf::from("/nonexistent/xyz/abc/def"));
        assert!(!verified);
    }

    #[test]
    fn decode_slug_anchored_real_world_issue_example() {
        // The exact real-world shape from issue #607: a hyphenated leaf
        // directory (claude-wrapper) under a non-hyphenated parent. The
        // naive decode would split it into .../claude/wrapper; anchoring
        // on disk keeps it whole.
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("rust").join("claude-wrapper");
        fs::create_dir_all(&dir).unwrap();
        let tmp_str = tmp.path().to_string_lossy();
        let tmp_encoded = tmp_str.trim_start_matches('/').replace('/', "-");
        let slug = format!("-{tmp_encoded}-rust-claude-wrapper");
        let expected = tmp.path().join("rust").join("claude-wrapper");
        let (decoded, is_verified) = decode_slug_anchored(&slug);
        assert_eq!(decoded, expected);
        assert!(is_verified);
    }

    // -- ListOptions / pagination -----------------------------------

    /// Build a fixture with five projects of varying activity so
    /// recency sort and pagination have meaningful inputs.
    fn paginated_fixture() -> tempfile::TempDir {
        let tmp = tempfile::tempdir().unwrap();
        // Two empty projects (no .jsonl files), three with one each.
        for stem in ["-zzz-empty1", "-aaa-empty2"] {
            fs::create_dir_all(tmp.path().join(stem)).unwrap();
        }
        for (stem, ts, mtime) in [
            ("-bbb-proj", "2026-03-01T00:00:00Z", 1_700_000_000),
            ("-ccc-proj", "2026-04-01T00:00:00Z", 1_700_001_000),
            ("-ddd-proj", "2026-05-01T00:00:00Z", 1_700_002_000),
        ] {
            let dir = tmp.path().join(stem);
            fs::create_dir_all(&dir).unwrap();
            let session_path = write_session(
                &dir,
                "s1",
                &[&format!(
                    r#"{{"type":"user","uuid":"u","timestamp":"{ts}","message":{{"role":"user","content":"x"}}}}"#
                )],
            );
            set_mtime(&session_path, mtime);
        }
        tmp
    }

    #[test]
    fn list_projects_with_include_empty_false_filters_them_out() {
        let tmp = paginated_fixture();
        let root = HistoryRoot::at(tmp.path());
        let projects = root
            .list_projects_with(&ListOptions {
                include_empty: false,
                ..Default::default()
            })
            .expect("list");
        let slugs: Vec<&str> = projects.iter().map(|p| p.slug.as_str()).collect();
        // Empty projects (-zzz-empty1 / -aaa-empty2) filtered out.
        assert_eq!(slugs, ["-bbb-proj", "-ccc-proj", "-ddd-proj"]);
    }

    #[test]
    fn list_projects_with_default_includes_empty_for_bc() {
        // Default::default() must preserve legacy "include everything"
        // semantics so zero-arg list_projects() doesn't change behavior.
        let tmp = paginated_fixture();
        let root = HistoryRoot::at(tmp.path());
        let projects = root
            .list_projects_with(&ListOptions::default())
            .expect("list");
        assert_eq!(projects.len(), 5);
    }

    #[test]
    fn list_projects_zero_arg_preserves_legacy_inclusion() {
        // The original list_projects() returned everything in slug order;
        // we must NOT regress that contract for existing callers.
        let tmp = paginated_fixture();
        let root = HistoryRoot::at(tmp.path());
        let projects = root.list_projects().expect("list");
        assert_eq!(projects.len(), 5);
        let slugs: Vec<&str> = projects.iter().map(|p| p.slug.as_str()).collect();
        assert_eq!(
            slugs,
            [
                "-aaa-empty2",
                "-bbb-proj",
                "-ccc-proj",
                "-ddd-proj",
                "-zzz-empty1",
            ]
        );
    }

    #[test]
    fn list_projects_with_limit_caps_results() {
        let tmp = paginated_fixture();
        let root = HistoryRoot::at(tmp.path());
        let projects = root
            .list_projects_with(&ListOptions {
                limit: Some(2),
                include_empty: true,
                ..Default::default()
            })
            .expect("list");
        assert_eq!(projects.len(), 2);
    }

    #[test]
    fn list_projects_with_offset_skips() {
        let tmp = paginated_fixture();
        let root = HistoryRoot::at(tmp.path());
        let projects = root
            .list_projects_with(&ListOptions {
                offset: 3,
                include_empty: true,
                ..Default::default()
            })
            .expect("list");
        // NameAsc default; skipping 3 from [aaa, bbb, ccc, ddd, zzz]
        // leaves [ddd, zzz].
        let slugs: Vec<&str> = projects.iter().map(|p| p.slug.as_str()).collect();
        assert_eq!(slugs, ["-ddd-proj", "-zzz-empty1"]);
    }

    #[test]
    fn list_projects_with_offset_past_end_returns_empty() {
        let tmp = paginated_fixture();
        let root = HistoryRoot::at(tmp.path());
        let projects = root
            .list_projects_with(&ListOptions {
                offset: 99,
                include_empty: true,
                ..Default::default()
            })
            .expect("list");
        assert!(projects.is_empty());
    }

    #[test]
    fn list_projects_with_recency_desc_sort() {
        let tmp = paginated_fixture();
        let root = HistoryRoot::at(tmp.path());
        // -ddd-proj has the newest session (May 2026), then -ccc, then -bbb.
        // The fixture writes them in order so filesystem mtimes also
        // progress. Filter empties so the tail isn't a no-mtime project.
        let projects = root
            .list_projects_with(&ListOptions {
                sort: ListSort::RecencyDesc,
                include_empty: false,
                ..Default::default()
            })
            .expect("list");
        let slugs: Vec<&str> = projects.iter().map(|p| p.slug.as_str()).collect();
        assert_eq!(slugs, ["-ddd-proj", "-ccc-proj", "-bbb-proj"]);
    }

    #[test]
    fn list_sessions_with_include_empty_false_filters_zero_message() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("-proj");
        fs::create_dir_all(&dir).unwrap();
        // One real session.
        write_session(
            &dir,
            "real",
            &[
                r#"{"type":"user","uuid":"u","timestamp":"2026-05-01T00:00:00Z","message":{"role":"user","content":"x"}}"#,
            ],
        );
        // One orphan: just a queue-op, no user/assistant.
        write_session(
            &dir,
            "orphan",
            &[
                r#"{"type":"queue-operation","operation":"enqueue","timestamp":"2026-05-01T00:00:00Z"}"#,
            ],
        );
        let root = HistoryRoot::at(tmp.path());
        let sessions = root
            .list_sessions_with(
                Some("-proj"),
                &ListOptions {
                    include_empty: false,
                    ..Default::default()
                },
            )
            .expect("list");
        let ids: Vec<&str> = sessions.iter().map(|s| s.session_id.as_str()).collect();
        assert_eq!(ids, ["real"]);
    }

    #[test]
    fn list_sessions_with_default_returns_orphans_for_bc() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("-proj");
        fs::create_dir_all(&dir).unwrap();
        write_session(
            &dir,
            "orphan",
            &[
                r#"{"type":"queue-operation","operation":"enqueue","timestamp":"2026-05-01T00:00:00Z"}"#,
            ],
        );
        let root = HistoryRoot::at(tmp.path());
        let sessions = root
            .list_sessions_with(Some("-proj"), &ListOptions::default())
            .expect("list");
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].message_count, 0);
    }

    #[test]
    fn list_sessions_with_recency_desc_sort() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("-proj");
        fs::create_dir_all(&dir).unwrap();
        let old_p = write_session(
            &dir,
            "old",
            &[
                r#"{"type":"user","uuid":"u","timestamp":"2026-01-01T00:00:00Z","message":{"role":"user","content":"x"}}"#,
            ],
        );
        let new_p = write_session(
            &dir,
            "new",
            &[
                r#"{"type":"user","uuid":"u","timestamp":"2026-12-01T00:00:00Z","message":{"role":"user","content":"x"}}"#,
            ],
        );
        let mid_p = write_session(
            &dir,
            "mid",
            &[
                r#"{"type":"user","uuid":"u","timestamp":"2026-06-01T00:00:00Z","message":{"role":"user","content":"x"}}"#,
            ],
        );
        set_mtime(&old_p, 1_700_000_000);
        set_mtime(&mid_p, 1_700_001_000);
        set_mtime(&new_p, 1_700_002_000);
        let root = HistoryRoot::at(tmp.path());
        let sessions = root
            .list_sessions_with(
                Some("-proj"),
                &ListOptions {
                    sort: ListSort::RecencyDesc,
                    ..Default::default()
                },
            )
            .expect("list");
        let ids: Vec<&str> = sessions.iter().map(|s| s.session_id.as_str()).collect();
        assert_eq!(ids, ["new", "mid", "old"]);
    }

    #[test]
    fn list_sessions_with_limit_and_offset_combine() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("-proj");
        fs::create_dir_all(&dir).unwrap();
        for i in 0..5 {
            write_session(
                &dir,
                &format!("s{i}"),
                &[&format!(
                    r#"{{"type":"user","uuid":"u","timestamp":"2026-01-0{i}T00:00:00Z","message":{{"role":"user","content":"x"}}}}"#
                )],
            );
        }
        let root = HistoryRoot::at(tmp.path());
        let sessions = root
            .list_sessions_with(
                Some("-proj"),
                &ListOptions {
                    offset: 1,
                    limit: Some(2),
                    ..Default::default()
                },
            )
            .expect("list");
        let ids: Vec<&str> = sessions.iter().map(|s| s.session_id.as_str()).collect();
        // NameAsc default: ids are s0..s4; skip 1, take 2 → ["s1","s2"].
        assert_eq!(ids, ["s1", "s2"]);
    }

    // -- aiTitle parsing bug fix ---------------------------------------

    #[test]
    fn session_summary_parses_ai_title_camelcase() {
        // Real claude-code writes the title under `aiTitle`, not
        // `title`. Regression test for the field-name bug.
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("-proj");
        fs::create_dir_all(&dir).unwrap();
        write_session(
            &dir,
            "real-shape",
            &[
                r#"{"type":"user","uuid":"u","timestamp":"2026-05-01T00:00:00Z","message":{"role":"user","content":"x"}}"#,
                r#"{"type":"ai-title","aiTitle":"My Session","sessionId":"real-shape"}"#,
            ],
        );
        let root = HistoryRoot::at(tmp.path());
        let sessions = root.list_sessions(Some("-proj")).expect("list");
        let s = sessions
            .iter()
            .find(|s| s.session_id == "real-shape")
            .unwrap();
        assert_eq!(s.title.as_deref(), Some("My Session"));
    }

    #[test]
    fn session_summary_legacy_title_field_still_works() {
        // Older fixtures used `title`; we still accept it as a fallback.
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("-proj");
        fs::create_dir_all(&dir).unwrap();
        write_session(
            &dir,
            "legacy",
            &[
                r#"{"type":"user","uuid":"u","timestamp":"2026-05-01T00:00:00Z","message":{"role":"user","content":"x"}}"#,
                r#"{"type":"ai-title","title":"Legacy Form"}"#,
            ],
        );
        let root = HistoryRoot::at(tmp.path());
        let sessions = root.list_sessions(Some("-proj")).expect("list");
        let s = sessions.iter().find(|s| s.session_id == "legacy").unwrap();
        assert_eq!(s.title.as_deref(), Some("Legacy Form"));
    }

    // -- forward slug derivation / sessions_for_path (#642) ----------

    #[test]
    fn encode_path_slug_encodes_slash_and_dot() {
        assert_eq!(
            encode_path_slug("/Users/josh/Code/projA"),
            "-Users-josh-Code-projA"
        );
        // The #642 gap: a `.` in a path segment is encoded too.
        assert_eq!(
            encode_path_slug("/private/var/folders/T/tmp.AbC"),
            "-private-var-folders-T-tmp-AbC"
        );
        // The #649 gap: every non-alphanumeric char is encoded,
        // including `_`, spaces, and other separators -- matching the
        // CLI's project-dir naming.
        assert_eq!(
            encode_path_slug("/Users/me/genagent/claude_wrapper_ex"),
            "-Users-me-genagent-claude-wrapper-ex"
        );
        assert_eq!(
            encode_path_slug("/Users/me/My Project (v2)"),
            "-Users-me-My-Project--v2-"
        );
    }

    #[test]
    fn project_slug_canonicalizes_and_encodes_dot() {
        let work = tempfile::tempdir().unwrap();
        let cwd = work.path().join("my.proj");
        fs::create_dir_all(&cwd).unwrap();

        let slug = HistoryRoot::project_slug(&cwd);
        assert!(
            slug.contains("my-proj"),
            "dotted segment must encode '.' -> '-', got {slug}"
        );
        assert!(
            !slug.contains('.'),
            "no '.' may survive in the slug: {slug}"
        );
        assert!(
            !slug.contains('/'),
            "no '/' may survive in the slug: {slug}"
        );
    }

    #[test]
    fn project_slug_canonicalizes_and_encodes_underscore() {
        // #649: an `_` in a path segment must encode to `-`, matching
        // the CLI's project-dir naming.
        let work = tempfile::tempdir().unwrap();
        let cwd = work.path().join("claude_wrapper_ex");
        fs::create_dir_all(&cwd).unwrap();

        let slug = HistoryRoot::project_slug(&cwd);
        assert!(
            slug.contains("claude-wrapper-ex"),
            "underscored segment must encode '_' -> '-', got {slug}"
        );
        assert!(
            !slug.contains('_'),
            "no '_' may survive in the slug: {slug}"
        );
    }

    #[test]
    fn sessions_for_path_finds_session_under_dotted_symlinked_cwd() {
        // Repro for #642. On macOS tempdirs live under /var -> /private/var
        // (a symlink), and the cwd here also has a '.' segment. claude
        // writes the session under the canonicalized, dot-encoded slug;
        // sessions_for_path must derive the same slug and find it.
        let projects = tempfile::tempdir().unwrap();
        let work = tempfile::tempdir().unwrap();
        let cwd = work.path().join("tmp.XYZ");
        fs::create_dir_all(&cwd).unwrap();

        // Build the project dir using claude's derivation directly
        // (canonicalize + encode), independent of the method under test,
        // so a project_slug that skipped either step would find nothing.
        let canonical = fs::canonicalize(&cwd).unwrap();
        let expected_slug = encode_path_slug(&canonical.to_string_lossy());
        let proj_dir = projects.path().join(&expected_slug);
        fs::create_dir_all(&proj_dir).unwrap();
        write_session(
            &proj_dir,
            "sess-dot",
            &[
                r#"{"type":"user","uuid":"u1","timestamp":"2026-01-01T00:00:00Z","cwd":"x","message":{"role":"user","content":"hi"}}"#,
            ],
        );

        let root = HistoryRoot::at(projects.path());
        let sessions = root.sessions_for_path(&cwd).expect("enumerate");
        assert_eq!(
            sessions.len(),
            1,
            "should find the session for the dotted/symlinked cwd"
        );
        assert_eq!(sessions[0].session_id, "sess-dot");
    }
}
