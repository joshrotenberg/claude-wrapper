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
use claude_wrapper::duplex::{DuplexOptions, DuplexSession};
use nu_ansi_term::Color;
use reedline::{DefaultPrompt, DefaultPromptSegment, FileBackedHistory, Reedline, Signal};

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
            cost: if reset { 0.0 } else { cost },
            turns: if reset { 0 } else { turns },
            history: if reset { Vec::new() } else { history },
        })
    }
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
    let mut settings = Settings::default();
    if let Some(d) = &user.defaults {
        settings = settings.overlay(d);
    }
    if let Some(d) = &project.defaults {
        settings = settings.overlay(d);
    }
    if let Some(name) = &active {
        match crate::lookup_profile(project, user, name) {
            Some(p) => settings = settings.overlay(p),
            None => anyhow::bail!("unknown profile: {name}"),
        }
    }
    settings = settings.overlay(&crate::env_settings());
    // Repl flag overrides (only the seed knobs are exposed as flags here).
    if args.model.is_some() {
        settings.model = args.model.clone();
    }
    if args.effort.is_some() {
        settings.effort = args.effort.clone();
    }
    // A saved prompt template is meaningless for an interactive session.
    settings.prompt = None;

    let mut builder = Claude::builder();
    if let Some(cwd) = &args.cwd {
        builder = builder.working_dir(cwd);
    }
    let claude = builder.build()?;

    let session = Session::spawn(&claude, "main".to_string(), settings).await?;

    let input = if std::io::stdin().is_terminal() {
        Input::Editor(Box::new(build_editor()))
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

    async fn loop_(&mut self) -> anyhow::Result<std::process::ExitCode> {
        loop {
            let prompt = self.prompt();
            let Some(line) = self.next_line(&prompt) else {
                break;
            };
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            if let Some(rest) = line.strip_prefix('/') {
                match self.command(rest).await {
                    Ok(true) => break,
                    Ok(false) => {}
                    Err(e) => eprintln!("{}: {e}", Color::Red.paint("cr")),
                }
                continue;
            }
            if let Err(e) = self.turn(line.to_string()).await {
                eprintln!("{}: {e}", Color::Red.paint("cr"));
            }
        }
        // Close every session cleanly on the way out.
        for s in self.sessions.drain(..) {
            let _ = s.inner.close().await;
        }
        Ok(std::process::ExitCode::SUCCESS)
    }

    /// Run one prompt turn against the current session, cancellable with Ctrl-C.
    async fn turn(&mut self, prompt: String) -> anyhow::Result<()> {
        let idx = self.current;
        let echo = prompt.clone();
        let result = {
            let session = &self.sessions[idx].inner;
            tokio::select! {
                res = session.send(prompt) => Some(res?),
                _ = tokio::signal::ctrl_c() => {
                    eprintln!("\n{}", Color::DarkGray.paint("(interrupting...)"));
                    let _ = session.interrupt().await;
                    None
                }
            }
        };
        let Some(turn) = result else {
            return Ok(());
        };

        let answer = turn.result_text().unwrap_or("").to_string();
        println!("{answer}");
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
                // Rebuild the options from the current settings; this is the
                // spawn command for a fresh child (a live respawn also adds
                // --resume <id>).
                let opts = duplex_options(&self.cur().settings)?;
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
                let profile = crate::lookup_profile(&self.project, &self.user, arg)
                    .ok_or_else(|| anyhow::anyhow!("unknown profile: {arg}"))?
                    .clone();
                let mut s = Settings::default();
                if let Some(d) = &self.user.defaults {
                    s = s.overlay(d);
                }
                if let Some(d) = &self.project.defaults {
                    s = s.overlay(d);
                }
                s = s.overlay(&profile);
                s.prompt = None;
                self.reconfigure(s, false).await?;
            }
            "editor" => {
                let prompt = crate::compose_in_editor()?;
                self.turn(prompt).await?;
            }
            other => anyhow::bail!("unknown command: /{other} (try /help)"),
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
}

fn build_editor() -> Reedline {
    let editor = Reedline::create();
    if let Some(home) = std::env::var_os("HOME") {
        let path = PathBuf::from(home).join(".cr_history");
        if let Ok(history) = FileBackedHistory::with_file(2000, path) {
            return editor.with_history(Box::new(history));
        }
    }
    editor
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
  /sessions             list open sessions
  /explain              print the `claude` command this session was spawned with

Anything not starting with / is sent as a prompt. Ctrl-C cancels a running
turn; Ctrl-D exits."
    );
}
