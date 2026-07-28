//! `cr repl` -- an interactive multi-turn session.
//!
//! Holds one `claude` child open across turns via `DuplexSession`, seeded from
//! the same resolved `Settings` (defaults < profile < env < flags) the one-shot
//! path uses. Plain lines are prompts; `/`-prefixed lines are meta-commands.
//! Retuning a knob (`/model`, `/effort`, `/profile`) respawns the child with
//! `--resume` so the conversation history carries over.

use std::io::IsTerminal;
use std::path::{Path, PathBuf};

use claude_wrapper::Claude;
use claude_wrapper::duplex::{DuplexOptions, DuplexSession, InboundEvent};
use nu_ansi_term::Color;
use reedline::{
    ColumnarMenu, Completer, DefaultPrompt, DefaultPromptSegment, Emacs, FileBackedHistory,
    KeyCode, KeyModifiers, MenuBuilder, Reedline, ReedlineEvent, ReedlineMenu, Signal, Span,
    Suggestion, default_emacs_keybindings,
};

use crate::{ConfigFile, ReplArgs, Settings};

/// Where prompt lines come from. On a TTY it is the reedline editor (history,
/// hints, line editing); on a pipe it is plain stdin, so a scripted
/// `printf '...\n/exit\n' | cr repl` works with no editor.
enum Input {
    Editor(Box<Reedline>),
    Plain,
}

/// One open conversation: a live child plus the host-side bookkeeping the
/// duplex layer does not accumulate (cost, turn count, a prompt/answer log).
struct Session {
    name: String,
    settings: Settings,
    inner: DuplexSession,
    /// The CLI-assigned session id from the last turn, for `--resume` on a
    /// respawn. `None` until the first turn completes.
    session_id: Option<String>,
    /// The `--resume <id>` the live child was actually spawned with, if any, so
    /// `/explain` reflects the running command rather than a fresh spawn.
    resume_id: Option<String>,
    cost: f64,
    turns: u32,
    history: Vec<(String, String)>,
}

impl Session {
    async fn spawn(claude: &Claude, name: String, settings: Settings) -> anyhow::Result<Self> {
        let inner = DuplexSession::spawn(claude, duplex_options(&settings)?).await?;
        Ok(Session {
            name,
            settings,
            inner,
            session_id: None,
            resume_id: None,
            cost: 0.0,
            turns: 0,
            history: Vec::new(),
        })
    }

    /// Close the child and open a fresh one with `new_settings`, resuming the
    /// same session id so history carries over. Cost/turns/log are preserved.
    /// `reset` drops the resume (a `/new`: same settings, empty context).
    async fn respawn(
        self,
        claude: &Claude,
        new_settings: Settings,
        reset: bool,
    ) -> anyhow::Result<Self> {
        let Session {
            name,
            session_id,
            cost,
            turns,
            history,
            inner,
            ..
        } = self;
        let resume = if reset { None } else { session_id.clone() };
        let _ = inner.close().await;
        let mut opts = duplex_options(&new_settings)?;
        if let Some(id) = &resume {
            opts = opts.resume(id);
        }
        let inner = DuplexSession::spawn(claude, opts).await?;
        Ok(Session {
            name,
            settings: new_settings,
            inner,
            session_id: if reset { None } else { session_id },
            resume_id: resume,
            cost: if reset { 0.0 } else { cost },
            turns: if reset { 0 } else { turns },
            history: if reset { Vec::new() } else { history },
        })
    }
}

/// Resolve `defaults < profile < CR_<KEY> env` into a `Settings` for a session.
/// (CLI-flag overrides are layered on by the caller.) A saved prompt template
/// is dropped, since an interactive session has no template to render.
fn resolve_settings(
    user: &ConfigFile,
    project: &ConfigFile,
    active: Option<&str>,
) -> anyhow::Result<Settings> {
    let mut s = Settings::default();
    if let Some(d) = &user.defaults {
        s = s.overlay(d);
    }
    if let Some(d) = &project.defaults {
        s = s.overlay(d);
    }
    if let Some(name) = active {
        match crate::lookup_profile(project, user, name) {
            Some(p) => s = s.overlay(p),
            None => anyhow::bail!("unknown profile: {name}"),
        }
    }
    s = s.overlay(&crate::env_settings());
    s.prompt = None;
    Ok(s)
}

/// Map a resolved `Settings` onto a `DuplexOptions`. Mirrors the one-shot
/// command build in `main::run`, minus the per-run/output knobs.
fn duplex_options(settings: &Settings) -> anyhow::Result<DuplexOptions> {
    let mut opts = DuplexOptions::default();
    if let Some(m) = &settings.model {
        opts = opts.model(m);
    }
    if let Some(e) = &settings.effort {
        opts = opts.effort(crate::parse_effort(e)?);
    }
    if let Some(a) = &settings.agent {
        opts = opts.agent(a);
    }
    if let Some(sp) = &settings.append_system_prompt {
        opts = opts.append_system_prompt(sp);
    }
    if let Some(pm) = &settings.permission_mode {
        opts = opts.permission_mode(crate::parse_permission_mode(pm)?);
    }
    if !settings.allowed_tools.is_empty() {
        opts = opts.allowed_tools(settings.allowed_tools.iter().map(String::as_str));
    }
    if !settings.disallowed_tools.is_empty() {
        opts = opts.disallowed_tools(settings.disallowed_tools.iter().map(String::as_str));
    }
    if let Some(n) = settings.max_turns {
        opts = opts.max_turns(n);
    }
    if let Some(b) = settings.max_budget_usd {
        opts = opts.max_budget_usd(b);
    }
    if let Some(fm) = &settings.fallback_model {
        opts = opts.fallback_model(fm);
    }
    if let Some(mc) = &settings.mcp_config {
        opts = opts.mcp_config(mc);
    }
    for d in &settings.add_dir {
        opts = opts.add_dir(d);
    }
    if settings.hermetic == Some(true) {
        opts = opts.hermetic();
    }
    if settings.worktree == Some(true) {
        opts = opts.worktree(None::<String>);
    }
    Ok(opts)
}

pub async fn run(
    args: ReplArgs,
    user: &ConfigFile,
    project: &ConfigFile,
    _project_path: &Path,
) -> anyhow::Result<std::process::ExitCode> {
    // Resolve the opening session's settings: defaults < profile < env < flags.
    let active = if args.no_profile {
        None
    } else {
        args.profile
            .clone()
            .or_else(|| std::env::var("CR_PROFILE").ok().filter(|s| !s.is_empty()))
            .or_else(|| project.default_profile.clone())
            .or_else(|| user.default_profile.clone())
    };
    let mut settings = resolve_settings(user, project, active.as_deref())?;
    // Repl flag overrides (only the seed knobs are exposed as flags here).
    if args.model.is_some() {
        settings.model = args.model.clone();
    }
    if args.effort.is_some() {
        settings.effort = args.effort.clone();
    }

    let mut builder = Claude::builder();
    if let Some(cwd) = &args.cwd {
        builder = builder.working_dir(cwd);
    }
    let claude = builder.build()?;

    let session = Session::spawn(&claude, "main".to_string(), settings).await?;

    // -e/--exec: run the given commands in order, then exit. No editor, no
    // banner, so output stays scriptable.
    if !args.exec.is_empty() {
        let mut state = Repl {
            claude,
            user: user.clone(),
            project: project.clone(),
            sessions: vec![session],
            current: 0,
            input: Input::Plain,
        };
        let mut had_err = false;
        for cmd in &args.exec {
            match state.handle_line(cmd).await {
                Ok(true) => break,
                Ok(false) => {}
                Err(e) => {
                    eprintln!("{}: {e}", Color::Red.paint("cr"));
                    had_err = true;
                }
            }
        }
        for s in state.sessions.drain(..) {
            let _ = s.inner.close().await;
        }
        return Ok(if had_err {
            std::process::ExitCode::from(1)
        } else {
            std::process::ExitCode::SUCCESS
        });
    }

    let input = if std::io::stdin().is_terminal() {
        Input::Editor(Box::new(build_editor(profile_names(user, project))))
    } else {
        Input::Plain
    };

    let mut state = Repl {
        claude,
        user: user.clone(),
        project: project.clone(),
        sessions: vec![session],
        current: 0,
        input,
    };

    state.banner();
    state.loop_().await
}

/// Every profile name known to either config, for command completion.
fn profile_names(user: &ConfigFile, project: &ConfigFile) -> Vec<String> {
    let mut names: Vec<String> = user
        .profiles
        .keys()
        .chain(project.profiles.keys())
        .cloned()
        .collect();
    names.sort();
    names.dedup();
    names
}

/// The interactive state: the client, a snapshot of config for `/profile`
/// lookups, and the session list with a current pointer.
struct Repl {
    claude: Claude,
    user: ConfigFile,
    project: ConfigFile,
    sessions: Vec<Session>,
    current: usize,
    input: Input,
}

impl Repl {
    fn cur(&self) -> &Session {
        &self.sessions[self.current]
    }

    fn banner(&self) {
        let s = self.cur();
        let model = s.settings.model.as_deref().unwrap_or("default");
        eprintln!(
            "{} interactive session ({}), model {}. /help for commands, /exit to quit.",
            Color::Green.bold().paint("cr"),
            s.name,
            Color::Cyan.paint(model),
        );
    }

    fn prompt(&self) -> DefaultPrompt {
        let s = self.cur();
        let model = s.settings.model.as_deref().unwrap_or("default");
        DefaultPrompt::new(
            DefaultPromptSegment::Basic(format!("{}:{model}", s.name)),
            DefaultPromptSegment::Empty,
        )
    }

    /// Read the next input line. `None` ends the REPL (Ctrl-D or EOF); an empty
    /// string means "skip" (Ctrl-C on an editor line).
    fn next_line(&mut self, prompt: &DefaultPrompt) -> Option<String> {
        match &mut self.input {
            Input::Editor(editor) => match editor.read_line(prompt) {
                Ok(Signal::Success(line)) => Some(line),
                Ok(Signal::CtrlC) => Some(String::new()),
                Ok(Signal::CtrlD) => None,
                Ok(_) => Some(String::new()),
                Err(e) => {
                    eprintln!("cr: input error: {e}");
                    None
                }
            },
            Input::Plain => {
                let mut line = String::new();
                match std::io::stdin().read_line(&mut line) {
                    Ok(0) | Err(_) => None,
                    Ok(_) => Some(line),
                }
            }
        }
    }

    /// Handle one input line: a `/command` (returns `Ok(true)` to exit) or a
    /// prompt turn. Shared by the interactive loop and `-e/--exec`.
    async fn handle_line(&mut self, line: &str) -> anyhow::Result<bool> {
        let line = line.trim();
        if line.is_empty() {
            return Ok(false);
        }
        if let Some(rest) = line.strip_prefix('/') {
            return self.command(rest).await;
        }
        self.turn(line.to_string()).await?;
        Ok(false)
    }

    async fn loop_(&mut self) -> anyhow::Result<std::process::ExitCode> {
        loop {
            let prompt = self.prompt();
            let Some(line) = self.next_line(&prompt) else {
                break;
            };
            match self.handle_line(&line).await {
                Ok(true) => break,
                Ok(false) => {}
                Err(e) => eprintln!("{}: {e}", Color::Red.paint("cr")),
            }
        }
        // Close every session cleanly on the way out.
        for s in self.sessions.drain(..) {
            let _ = s.inner.close().await;
        }
        Ok(std::process::ExitCode::SUCCESS)
    }

    /// Run one prompt turn against the current session. Assistant text streams
    /// to stdout as it arrives; Ctrl-C cancels the turn via `interrupt()`.
    async fn turn(&mut self, prompt: String) -> anyhow::Result<()> {
        use std::io::Write;
        let idx = self.current;
        let echo = prompt.clone();
        let mut printed_any = false;
        let result = {
            let session = &self.sessions[idx].inner;
            // Subscribe before the send is polled so no early delta is missed.
            let mut rx = session.subscribe();
            let send_fut = session.send(prompt);
            tokio::pin!(send_fut);
            let mut out = std::io::stdout();
            let mut turn = None;
            loop {
                tokio::select! {
                    // Prefer draining text deltas over taking the result, so the
                    // streamed output is complete before the turn is finalized.
                    biased;
                    ev = rx.recv() => {
                        if let Ok(ev) = ev
                            && let Some(t) = stream_text_delta(&ev)
                        {
                            let _ = write!(out, "{t}");
                            let _ = out.flush();
                            printed_any = true;
                        }
                    }
                    res = &mut send_fut => {
                        turn = Some(res?);
                        while let Ok(ev) = rx.try_recv() {
                            if let Some(t) = stream_text_delta(&ev) {
                                let _ = write!(out, "{t}");
                                let _ = out.flush();
                                printed_any = true;
                            }
                        }
                        break;
                    }
                    _ = tokio::signal::ctrl_c() => {
                        eprintln!("\n{}", Color::DarkGray.paint("(interrupting...)"));
                        let _ = session.interrupt().await;
                        break;
                    }
                }
            }
            turn
        };
        let Some(turn) = result else {
            if printed_any {
                println!();
            }
            return Ok(());
        };

        let answer = turn.result_text().unwrap_or("").to_string();
        if printed_any {
            // Terminate the streamed line.
            println!();
        } else {
            // No partial deltas were emitted; print the buffered answer.
            println!("{answer}");
        }
        let s = &mut self.sessions[idx];
        s.turns += 1;
        if let Some(c) = turn.total_cost_usd() {
            s.cost += c;
        }
        if let Some(id) = turn.session_id() {
            s.session_id = Some(id.to_string());
        }
        s.history.push((echo, answer));
        eprintln!("{}", Color::DarkGray.paint(turn_footer(&turn)));
        Ok(())
    }

    /// Dispatch a `/command`. Returns `Ok(true)` to exit the REPL.
    async fn command(&mut self, line: &str) -> anyhow::Result<bool> {
        let mut parts = line.split_whitespace();
        let cmd = parts.next().unwrap_or("");
        let arg = line[cmd.len()..].trim();
        match cmd {
            "help" | "?" => print_help(),
            "exit" | "quit" | "q" => return Ok(true),
            "cost" => self.cmd_cost(),
            "history" => self.cmd_history(),
            "sessions" => self.cmd_sessions(),
            "explain" => {
                // Rebuild the options from the current settings and reflect the
                // live child's --resume, so this matches what is actually running.
                let s = self.cur();
                let mut opts = duplex_options(&s.settings)?;
                if let Some(id) = &s.resume_id {
                    opts = opts.resume(id);
                }
                println!("{}", opts.to_command_string(&self.claude));
            }
            "new" => {
                let s = self.cur().settings.clone();
                self.reconfigure(s, true).await?;
            }
            "model" => {
                if arg.is_empty() {
                    anyhow::bail!("usage: /model <name>");
                }
                let mut s = self.cur().settings.clone();
                s.model = Some(arg.to_string());
                self.reconfigure(s, false).await?;
            }
            "effort" => {
                if arg.is_empty() {
                    anyhow::bail!("usage: /effort <low|medium|high|xhigh|max>");
                }
                crate::parse_effort(arg)?; // validate before respawn
                let mut s = self.cur().settings.clone();
                s.effort = Some(arg.to_string());
                self.reconfigure(s, false).await?;
            }
            "profile" => {
                if arg.is_empty() {
                    anyhow::bail!("usage: /profile <name>");
                }
                let s = resolve_settings(&self.user, &self.project, Some(arg))?;
                self.reconfigure(s, false).await?;
            }
            "session" => self.cmd_session_new(arg).await?,
            "use" => self.cmd_use(arg)?,
            "close" => self.cmd_close(arg).await?,
            "all" => self.cmd_all(arg).await?,
            "editor" => {
                let prompt = crate::compose_in_editor()?;
                self.turn(prompt).await?;
            }
            other => match nearest_command(other) {
                Some(sug) => anyhow::bail!("unknown command: /{other} (did you mean /{sug}?)"),
                None => anyhow::bail!("unknown command: /{other} (try /help)"),
            },
        }
        Ok(false)
    }

    /// Respawn the current session with new settings (see `Session::respawn`).
    async fn reconfigure(&mut self, settings: Settings, reset: bool) -> anyhow::Result<()> {
        let idx = self.current;
        let old = self.sessions.remove(idx);
        let label = settings.model.clone().unwrap_or_else(|| "default".into());
        match old.respawn(&self.claude, settings, reset).await {
            Ok(new) => {
                self.sessions.insert(idx, new);
                let note = if reset {
                    "reset context"
                } else {
                    "reconfigured"
                };
                eprintln!(
                    "{}",
                    Color::DarkGray.paint(format!("({note}, model {label})"))
                );
                Ok(())
            }
            Err(e) => {
                // The old session is already closed; drop the slot rather than
                // leave a dangling index. A fresh /profile or /model can reopen.
                anyhow::bail!("respawn failed, session closed: {e}");
            }
        }
    }

    fn cmd_cost(&self) {
        let s = self.cur();
        println!("{}: {} turns, ${:.4}", s.name, s.turns, s.cost);
        if self.sessions.len() > 1 {
            let total: f64 = self.sessions.iter().map(|s| s.cost).sum();
            println!("total across {} sessions: ${total:.4}", self.sessions.len());
        }
    }

    fn cmd_history(&self) {
        let s = self.cur();
        if s.history.is_empty() {
            println!("(no turns yet)");
            return;
        }
        for (i, (prompt, answer)) in s.history.iter().enumerate() {
            println!(
                "{} {}",
                Color::Cyan.paint(format!("{}.", i + 1)),
                first_line(prompt)
            );
            println!("   {}", first_line(answer));
        }
    }

    fn cmd_sessions(&self) {
        for (i, s) in self.sessions.iter().enumerate() {
            let star = if i == self.current { "*" } else { " " };
            let model = s.settings.model.as_deref().unwrap_or("default");
            println!(
                "{star} {}  [{}]  {} turns  ${:.4}",
                s.name, model, s.turns, s.cost
            );
        }
    }

    /// `/session new <name> [profile]`: open a second (or Nth) conversation and
    /// select it. With a profile, seed from it; otherwise clone the current
    /// session's settings.
    async fn cmd_session_new(&mut self, arg: &str) -> anyhow::Result<()> {
        let mut it = arg.split_whitespace();
        match it.next() {
            Some("new") => {}
            Some(other) => anyhow::bail!("unknown: /session {other} (only `/session new`)"),
            None => anyhow::bail!("usage: /session new <name> [profile]"),
        }
        let name = it
            .next()
            .ok_or_else(|| anyhow::anyhow!("usage: /session new <name> [profile]"))?
            .to_string();
        if self.sessions.iter().any(|s| s.name == name) {
            anyhow::bail!("session {name:?} already exists");
        }
        let settings = match it.next() {
            Some(profile) => resolve_settings(&self.user, &self.project, Some(profile))?,
            None => self.cur().settings.clone(),
        };
        let session = Session::spawn(&self.claude, name.clone(), settings).await?;
        self.sessions.push(session);
        self.current = self.sessions.len() - 1;
        eprintln!(
            "{}",
            Color::DarkGray.paint(format!("(opened {name} and selected it)"))
        );
        Ok(())
    }

    /// `/use <name>`: make an existing session current.
    fn cmd_use(&mut self, arg: &str) -> anyhow::Result<()> {
        if arg.is_empty() {
            anyhow::bail!("usage: /use <name>");
        }
        let idx = self
            .sessions
            .iter()
            .position(|s| s.name == arg)
            .ok_or_else(|| anyhow::anyhow!("no session named {arg:?} (see /sessions)"))?;
        self.current = idx;
        Ok(())
    }

    /// `/close [name]`: close a session (the current one by default). The last
    /// open session cannot be closed; use `/exit` to leave.
    async fn cmd_close(&mut self, arg: &str) -> anyhow::Result<()> {
        if self.sessions.len() == 1 {
            anyhow::bail!("can't close the last session; use /exit to leave");
        }
        let idx = if arg.is_empty() {
            self.current
        } else {
            self.sessions
                .iter()
                .position(|s| s.name == arg)
                .ok_or_else(|| anyhow::anyhow!("no session named {arg:?}"))?
        };
        let s = self.sessions.remove(idx);
        let name = s.name.clone();
        let _ = s.inner.close().await;
        if idx < self.current {
            self.current -= 1;
        }
        if self.current >= self.sessions.len() {
            self.current = self.sessions.len() - 1;
        }
        eprintln!("{}", Color::DarkGray.paint(format!("(closed {name})")));
        Ok(())
    }

    /// `/all <prompt>`: send the same prompt to every session in turn.
    async fn cmd_all(&mut self, arg: &str) -> anyhow::Result<()> {
        if arg.is_empty() {
            anyhow::bail!("usage: /all <prompt>");
        }
        let saved = self.current;
        for i in 0..self.sessions.len() {
            self.current = i;
            let name = self.sessions[i].name.clone();
            eprintln!("{}", Color::Cyan.bold().paint(format!("── {name} ──")));
            if let Err(e) = self.turn(arg.to_string()).await {
                eprintln!("{}: {e}", Color::Red.paint("cr"));
            }
        }
        self.current = saved.min(self.sessions.len().saturating_sub(1));
        Ok(())
    }
}

/// The meta-command words, for did-you-mean and completion.
const COMMANDS: &[&str] = &[
    "help", "exit", "quit", "cost", "history", "sessions", "explain", "new", "model", "effort",
    "profile", "session", "use", "close", "all", "editor",
];

/// Tab-completion: command words after `/`, and profile names after `/profile`.
struct CrCompleter {
    profiles: Vec<String>,
}

impl Completer for CrCompleter {
    fn complete(&mut self, line: &str, pos: usize) -> Vec<Suggestion> {
        let end = pos.min(line.len());
        let prefix = &line[..end];
        let Some(rest) = prefix.strip_prefix('/') else {
            return Vec::new();
        };
        match rest.split_once(char::is_whitespace) {
            // No space yet: complete the command word (span starts after '/').
            None => COMMANDS
                .iter()
                .filter(|c| c.starts_with(rest))
                .map(|c| suggestion(c, 1, end))
                .collect(),
            // `/profile <partial>`: complete profile names on the last word.
            Some(("profile", _)) => {
                let start = prefix.rfind(char::is_whitespace).map_or(0, |i| i + 1);
                let word = &prefix[start..];
                self.profiles
                    .iter()
                    .filter(|p| p.starts_with(word))
                    .map(|p| suggestion(p, start, end))
                    .collect()
            }
            _ => Vec::new(),
        }
    }
}

fn suggestion(value: &str, start: usize, end: usize) -> Suggestion {
    Suggestion {
        value: value.to_string(),
        span: Span { start, end },
        append_whitespace: true,
        ..Suggestion::default()
    }
}

fn build_editor(profiles: Vec<String>) -> Reedline {
    let completer = Box::new(CrCompleter { profiles });
    let menu = Box::new(ColumnarMenu::default().with_name("completion_menu"));
    let mut keybindings = default_emacs_keybindings();
    keybindings.add_binding(
        KeyModifiers::NONE,
        KeyCode::Tab,
        ReedlineEvent::UntilFound(vec![
            ReedlineEvent::Menu("completion_menu".to_string()),
            ReedlineEvent::MenuNext,
        ]),
    );
    let mut editor = Reedline::create()
        .with_completer(completer)
        .with_menu(ReedlineMenu::EngineCompleter(menu))
        .with_edit_mode(Box::new(Emacs::new(keybindings)));
    if let Some(home) = std::env::var_os("HOME") {
        let path = PathBuf::from(home).join(".cr_history");
        if let Ok(history) = FileBackedHistory::with_file(2000, path) {
            editor = editor.with_history(Box::new(history));
        }
    }
    editor
}

/// The nearest command word to `input` by edit distance, if close enough to be
/// a plausible typo (scaled to the input length).
fn nearest_command(input: &str) -> Option<&'static str> {
    let mut best: Option<(&'static str, usize)> = None;
    for &c in COMMANDS {
        let d = levenshtein(input, c);
        if best.is_none_or(|(_, bd)| d < bd) {
            best = Some((c, d));
        }
    }
    best.filter(|(_, d)| *d <= 2.max(input.len() / 3))
        .map(|(c, _)| c)
}

fn levenshtein(a: &str, b: &str) -> usize {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    let mut curr = vec![0usize; b.len() + 1];
    for (i, &ca) in a.iter().enumerate() {
        curr[0] = i + 1;
        for (j, &cb) in b.iter().enumerate() {
            let cost = usize::from(ca != cb);
            curr[j + 1] = (prev[j + 1] + 1).min(curr[j] + 1).min(prev[j] + cost);
        }
        std::mem::swap(&mut prev, &mut curr);
    }
    prev[b.len()]
}

/// Extract an assistant text delta from a streaming event, if it is one.
/// The duplex spawn runs with partial messages on, so a turn's assistant text
/// arrives as a run of `content_block_delta`/`text_delta` stream events.
fn stream_text_delta(ev: &InboundEvent) -> Option<String> {
    let InboundEvent::StreamEvent(v) = ev else {
        return None;
    };
    let event = v.get("event")?;
    if event.get("type")?.as_str()? != "content_block_delta" {
        return None;
    }
    let delta = event.get("delta")?;
    if delta.get("type")?.as_str()? != "text_delta" {
        return None;
    }
    Some(delta.get("text")?.as_str()?.to_string())
}

fn turn_footer(turn: &claude_wrapper::duplex::TurnResult) -> String {
    let mut parts = Vec::new();
    if let Some(c) = turn.total_cost_usd() {
        parts.push(format!("${c:.4}"));
    }
    if let Some(d) = turn.duration_ms() {
        parts.push(format!("{:.1}s", d as f64 / 1000.0));
    }
    parts.join(" · ")
}

fn first_line(s: &str) -> String {
    let line = s.lines().next().unwrap_or("");
    if line.chars().count() > 80 {
        let truncated: String = line.chars().take(79).collect();
        format!("{truncated}…")
    } else {
        line.to_string()
    }
}

fn print_help() {
    println!(
        "\
commands:
  /help                 this list
  /exit, /quit          close the session and leave
  /model <name>         switch model (respawns, keeps history)
  /effort <level>       switch effort (respawns, keeps history)
  /profile <name>       apply a profile's settings (respawns, keeps history)
  /new                  reset the conversation (same settings, empty context)
  /editor               compose a prompt in $EDITOR
  /cost                 turns and cost for this session
  /history              prompts and answers so far
  /explain              print the `claude` command this session was spawned with

sessions (run several conversations at once):
  /session new <name> [profile]   open another session and select it
  /use <name>                     switch to a session
  /sessions                       list open sessions
  /close [name]                   close a session (current by default)
  /all <prompt>                   send one prompt to every session

Anything not starting with / is sent as a prompt. Ctrl-C cancels a running
turn; Ctrl-D exits."
    );
}
