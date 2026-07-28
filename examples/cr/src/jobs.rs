//! Detached background jobs (Tier 0).
//!
//! A job is a `claude -p --output-format stream-json` process that cr spawns
//! **detached** (its own process group, stdin nulled, stdout captured to a
//! journal file) so it keeps running after cr exits. No daemon: the "backend"
//! is the detached process plus a directory of files, mirroring the shape of
//! Claude Code's own `~/.claude/jobs/<id>/` (see `claude_wrapper::jobs`).
//!
//! ```text
//! ~/.config/cr/jobs/<id>/
//!     state.json      the JobRecord below
//!     journal.jsonl   the child's stdout (stream-json events)
//!     stderr.log      the child's stderr
//! ```
//!
//! The child also writes its normal session transcript to
//! `~/.claude/projects/<slug>/<session-id>.jsonl`, so a finished job is
//! resumable with `--resume <session-id>` and readable via
//! `claude_wrapper::history`.

use std::fs;
use std::io::{BufRead, Write};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::Context;
use claude_wrapper::Claude;
use nu_ansi_term::Color;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Persistent record of a launched job (`state.json`). Liveness/completion are
/// computed at read time (from the journal and the pid), not stored, so a
/// crashed writer never leaves a stale "running".
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct JobRecord {
    pub id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_name: Option<String>,
    pub prompt: String,
    pub cwd: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    pub pid: u32,
    pub created_secs: u64,
    /// A human note of the guardrail cap in force (e.g. "$2.00", "12 turns").
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cap: Option<String>,
}

/// Where a job is in its lifecycle, computed from the journal and pid.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Status {
    Running,
    Done {
        is_error: bool,
    },
    /// Process gone with no `result` event: crashed or was killed.
    Stopped,
}

impl Status {
    fn label(self) -> nu_ansi_term::AnsiString<'static> {
        match self {
            Status::Running => Color::Yellow.paint("running"),
            Status::Done { is_error: false } => Color::Green.paint("done"),
            Status::Done { is_error: true } => Color::Red.paint("error"),
            Status::Stopped => Color::Red.paint("stopped"),
        }
    }
}

/// `~/.config/cr/jobs` (honours `XDG_CONFIG_HOME`).
pub fn jobs_dir() -> anyhow::Result<PathBuf> {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
        .context("no XDG_CONFIG_HOME or HOME to locate the cr job store")?;
    Ok(base.join("cr").join("jobs"))
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// A short, collision-resistant job id from the current time and pid.
fn new_id() -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let mix = nanos ^ ((std::process::id() as u128) << 17);
    format!("cr-{:07x}", (mix as u64) & 0xfff_ffff)
}

/// Launch a detached job. `argv` is the full `claude` subcommand argv (from
/// `QueryCommand::args()`), already carrying `-p --output-format stream-json`
/// and the resolved settings. Returns the written record.
pub fn launch(
    claude: &Claude,
    argv: Vec<String>,
    cwd: Option<&Path>,
    prompt: &str,
    session_name: Option<String>,
    model: Option<String>,
    cap: Option<String>,
) -> anyhow::Result<JobRecord> {
    let id = new_id();
    let dir = jobs_dir()?.join(&id);
    fs::create_dir_all(&dir).with_context(|| format!("creating job dir {}", dir.display()))?;

    let journal = fs::File::create(dir.join("journal.jsonl"))?;
    let stderr = fs::File::create(dir.join("stderr.log"))?;

    let run_cwd = cwd
        .map(Path::to_path_buf)
        .or_else(|| claude.working_dir().map(Path::to_path_buf))
        .or_else(|| std::env::current_dir().ok())
        .unwrap_or_else(|| PathBuf::from("."));

    let mut command = std::process::Command::new(claude.binary());
    command
        .args(&argv)
        .current_dir(&run_cwd)
        // Same hygiene the library's exec layer applies, so the child does not
        // believe it is running inside another Claude Code invocation.
        .env_remove("CLAUDECODE")
        .env_remove("CLAUDE_CODE_ENTRYPOINT")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::from(journal))
        .stderr(std::process::Stdio::from(stderr));
    detach(&mut command);

    let child = command
        .spawn()
        .with_context(|| format!("spawning detached claude ({})", claude.binary().display()))?;

    let record = JobRecord {
        id,
        session_name,
        prompt: prompt.to_string(),
        cwd: run_cwd.display().to_string(),
        model,
        pid: child.id(),
        created_secs: now_secs(),
        cap,
    };
    write_state(&dir, &record)?;
    Ok(record)
}

/// Put the child in its own process group so a Ctrl-C or terminal-close aimed
/// at cr's group does not reach it. Enough to outlive cr for Tier 0.
#[cfg(unix)]
fn detach(command: &mut std::process::Command) {
    use std::os::unix::process::CommandExt;
    command.process_group(0);
}

#[cfg(not(unix))]
fn detach(_command: &mut std::process::Command) {}

fn write_state(dir: &Path, record: &JobRecord) -> anyhow::Result<()> {
    let mut f = fs::File::create(dir.join("state.json"))?;
    f.write_all(serde_json::to_string_pretty(record)?.as_bytes())?;
    Ok(())
}

/// Is a pid still alive? `kill(pid, 0)` on unix; assume alive elsewhere.
#[cfg(unix)]
fn pid_alive(pid: u32) -> bool {
    // SAFETY: `kill` with signal 0 performs only an existence/permission check.
    unsafe { libc::kill(pid as libc::pid_t, 0) == 0 }
}

#[cfg(not(unix))]
fn pid_alive(_pid: u32) -> bool {
    true
}

/// Read a job's journal lines as parsed JSON values (best-effort; malformed or
/// partial trailing lines are skipped).
fn journal_events(id: &str) -> anyhow::Result<Vec<Value>> {
    let path = jobs_dir()?.join(id).join("journal.jsonl");
    let Ok(file) = fs::File::open(&path) else {
        return Ok(Vec::new());
    };
    let mut out = Vec::new();
    for line in std::io::BufReader::new(file).lines().map_while(Result::ok) {
        if line.trim().is_empty() {
            continue;
        }
        if let Ok(v) = serde_json::from_str::<Value>(&line) {
            out.push(v);
        }
    }
    Ok(out)
}

/// The final `result` event, if the job has finished.
fn result_event(events: &[Value]) -> Option<&Value> {
    events
        .iter()
        .rev()
        .find(|e| e.get("type").and_then(Value::as_str) == Some("result"))
}

/// The session id claude assigned, from the first init event.
pub fn session_id(id: &str) -> Option<String> {
    let events = journal_events(id).ok()?;
    events.iter().find_map(|e| {
        if e.get("type").and_then(Value::as_str) == Some("system") {
            e.get("session_id")
                .and_then(Value::as_str)
                .map(str::to_string)
        } else {
            None
        }
    })
}

pub fn status(record: &JobRecord) -> Status {
    let events = journal_events(&record.id).unwrap_or_default();
    if let Some(r) = result_event(&events) {
        let is_error = r.get("is_error").and_then(Value::as_bool).unwrap_or(false)
            || r.get("subtype").and_then(Value::as_str) == Some("error");
        return Status::Done { is_error };
    }
    if pid_alive(record.pid) {
        Status::Running
    } else {
        Status::Stopped
    }
}

/// All cr jobs, newest first.
pub fn list() -> anyhow::Result<Vec<JobRecord>> {
    let dir = jobs_dir()?;
    let mut jobs = Vec::new();
    let Ok(entries) = fs::read_dir(&dir) else {
        return Ok(jobs);
    };
    for entry in entries.flatten() {
        let state = entry.path().join("state.json");
        if let Ok(text) = fs::read_to_string(&state)
            && let Ok(rec) = serde_json::from_str::<JobRecord>(&text)
        {
            jobs.push(rec);
        }
    }
    jobs.sort_by_key(|j| std::cmp::Reverse(j.created_secs));
    Ok(jobs)
}

/// Resolve a selector to a job: exact id, unique id-prefix, or session name
/// (newest match wins).
pub fn resolve(selector: &str) -> anyhow::Result<JobRecord> {
    let jobs = list()?;
    if let Some(j) = jobs.iter().find(|j| j.id == selector) {
        return Ok(j.clone());
    }
    if let Some(j) = jobs
        .iter()
        .find(|j| j.session_name.as_deref() == Some(selector))
    {
        return Ok(j.clone());
    }
    let matches: Vec<&JobRecord> = jobs.iter().filter(|j| j.id.starts_with(selector)).collect();
    match matches.as_slice() {
        [one] => Ok((*one).clone()),
        [] => anyhow::bail!("no job matching {selector:?} (try `cr jobs`)"),
        _ => anyhow::bail!("{selector:?} is ambiguous ({} jobs match)", matches.len()),
    }
}

/// Render cr's jobs as rows (no header, no empty message; the caller frames it).
pub fn render_list(jobs: &[JobRecord]) {
    for j in jobs {
        let st = status(j);
        let name = j.session_name.as_deref().unwrap_or("-");
        println!(
            "{:<10} {:<8} {:<10} {}  {}",
            j.id,
            st.label(),
            name,
            Color::DarkGray.paint(ago(j.created_secs)),
            first_line(&j.prompt),
        );
    }
}

/// Also list Claude Code's own background jobs (read-only, via the library's
/// `jobs` introspection). Best-effort: returns the count printed, or 0 if the
/// daemon store is absent or unreadable. Marked `(claude)` to distinguish them.
pub fn render_daemon() -> usize {
    let Ok(root) = claude_wrapper::jobs::JobsRoot::home() else {
        return 0;
    };
    let Ok(list) = root.list() else {
        return 0;
    };
    for s in &list {
        let intent = s.intent.as_deref().or(s.name.as_deref()).unwrap_or("");
        println!(
            "{:<10} {:<8} {:<10} {}  {}",
            s.short_id,
            daemon_state_label(&s.state),
            "(claude)",
            Color::DarkGray.paint(s.created_at.clone().unwrap_or_default()),
            first_line(intent),
        );
    }
    list.len()
}

fn daemon_state_label(state: &str) -> nu_ansi_term::AnsiString<'static> {
    match state {
        "running" => Color::Yellow.paint(state.to_string()),
        "done" | "completed" => Color::Green.paint(state.to_string()),
        _ => Color::Red.paint(state.to_string()),
    }
}

/// Render one job: its header, a readable pass over the journal, and the final
/// status. With `follow`, keep tailing until a result appears. With `json`,
/// stream the raw journal lines instead.
pub fn render_job(record: &JobRecord, follow: bool, json: bool) -> anyhow::Result<()> {
    let st = status(record);
    eprintln!(
        "{} {}  [{}]  pid {}  {}",
        Color::Cyan.bold().paint(&record.id),
        st.label(),
        record.model.as_deref().unwrap_or("default"),
        record.pid,
        Color::DarkGray.paint(ago(record.created_secs)),
    );
    eprintln!("{} {}", Color::DarkGray.paint("prompt:"), record.prompt);
    if let Some(sid) = session_id(&record.id) {
        eprintln!(
            "{} {}  ({})",
            Color::DarkGray.paint("session:"),
            sid,
            Color::DarkGray.paint(format!("resume: cr repl --resume {sid}")),
        );
    }

    if json {
        return render_json(record, follow);
    }

    let mut seen = 0usize;
    loop {
        let events = journal_events(&record.id)?;
        for ev in events.iter().skip(seen) {
            if let Some(line) = render_event(ev) {
                println!("{line}");
            }
        }
        seen = events.len();
        if !follow || result_event(&events).is_some() || !pid_alive(record.pid) {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(700));
    }
    Ok(())
}

fn render_json(record: &JobRecord, follow: bool) -> anyhow::Result<()> {
    let mut out = std::io::stdout();
    let mut seen = 0usize;
    loop {
        let events = journal_events(&record.id)?;
        for ev in events.iter().skip(seen) {
            let _ = writeln!(out, "{ev}");
        }
        seen = events.len();
        if !follow || result_event(&events).is_some() || !pid_alive(record.pid) {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(700));
    }
    Ok(())
}

/// Turn one stream-json event into a readable line, or `None` to skip it.
fn render_event(ev: &Value) -> Option<String> {
    match ev.get("type").and_then(Value::as_str)? {
        "assistant" => {
            let content = ev.get("message")?.get("content")?.as_array()?;
            let mut parts = Vec::new();
            for block in content {
                match block.get("type").and_then(Value::as_str) {
                    Some("text") => {
                        if let Some(t) = block.get("text").and_then(Value::as_str) {
                            let t = t.trim();
                            if !t.is_empty() {
                                parts.push(t.to_string());
                            }
                        }
                    }
                    Some("tool_use") => {
                        let name = block.get("name").and_then(Value::as_str).unwrap_or("tool");
                        parts.push(
                            Color::Blue
                                .paint(format!("[tool] {name}{}", tool_hint(block)))
                                .to_string(),
                        );
                    }
                    _ => {}
                }
            }
            (!parts.is_empty()).then(|| parts.join("\n"))
        }
        "result" => {
            let cost = ev.get("total_cost_usd").and_then(Value::as_f64);
            let dur = ev.get("duration_ms").and_then(Value::as_u64);
            let mut tail = String::new();
            if let Some(c) = cost {
                tail.push_str(&format!(" ${c:.4}"));
            }
            if let Some(d) = dur {
                tail.push_str(&format!(" {:.1}s", d as f64 / 1000.0));
            }
            Some(Color::Green.paint(format!("[done]{tail}")).to_string())
        }
        _ => None,
    }
}

/// A short hint of a tool call's target (a path or command), if obvious.
fn tool_hint(block: &Value) -> String {
    let input = block.get("input");
    let hint = input
        .and_then(|i| i.get("file_path").or_else(|| i.get("path")))
        .or_else(|| input.and_then(|i| i.get("command")))
        .or_else(|| input.and_then(|i| i.get("pattern")))
        .and_then(Value::as_str);
    match hint {
        Some(h) => format!(" {}", first_line(h)),
        None => String::new(),
    }
}

fn first_line(s: &str) -> String {
    let line = s.lines().next().unwrap_or("").trim();
    if line.chars().count() > 72 {
        let truncated: String = line.chars().take(71).collect();
        format!("{truncated}…")
    } else {
        line.to_string()
    }
}

fn ago(created_secs: u64) -> String {
    let secs = now_secs().saturating_sub(created_secs);
    if secs < 60 {
        format!("{secs}s ago")
    } else if secs < 3600 {
        format!("{}m ago", secs / 60)
    } else if secs < 86400 {
        format!("{}h ago", secs / 3600)
    } else {
        format!("{}d ago", secs / 86400)
    }
}
