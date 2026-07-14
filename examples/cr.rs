//! `cr` -- a spike of a minimal, config-driven CLI over `claude-wrapper`.
//!
//! This is an EVALUATION PROTOTYPE, not a shipped binary. It exists to answer
//! one question: is a curated "profiles + a handful of flags" front door over
//! the wrapper genuinely simpler than the full roba surface, or does it just
//! move the complexity around?
//!
//! What it demonstrates:
//! - the mocked `cr` flag surface, rendered by clap (`cr --help`)
//! - TOML profiles with `defaults < profile < CLI` layering
//! - `--explain` (dry-run: print the exact `claude` command), no spawn
//! - `--save NAME` (creation-by-use: capture resolved flags into a profile)
//! - the cost/turns footer, from the parsed `QueryResult`
//!
//! Deliberately NOT in the spike: streaming, `-e` editor compose, worktree
//! lifecycle, the composition family (`--attach`/`--git-*`). Wiring those is a
//! one-liner each against the wrapper; they're omitted to keep the surface
//! honest.
//!
//! Run:
//!   cargo run --example cr --no-default-features \
//!     --features sync,json,tempfile -- --help
//!   cargo run --example cr ... -- --profile cheap --explain "summarize this"

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use clap::{Parser, Subcommand};
use claude_wrapper::{Claude, Effort, QueryCommand};
use serde::{Deserialize, Serialize};

/// A saved `claude -p` you can re-run: isolated, typed, repeatable.
#[derive(Parser, Debug)]
#[command(name = "cr", version)]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,

    #[command(flatten)]
    run: RunArgs,
}

#[derive(Subcommand, Debug)]
enum Command {
    /// List profiles and show what one resolves to.
    Profiles,
    /// Show the config file paths, or open the project one in $EDITOR.
    Config {
        /// Open the project cr.toml in $EDITOR (creating it if absent).
        #[arg(long)]
        edit: bool,
    },
}

#[derive(clap::Args, Debug, Default)]
struct RunArgs {
    /// Prompt text. Omit to read stdin; or use -f / -e.
    prompt: Option<String>,

    /// Read the prompt from a file.
    #[arg(short = 'f', long, value_name = "PATH", help_heading = "Prompt")]
    file: Option<PathBuf>,

    /// Compose the prompt in $EDITOR.
    #[arg(short = 'e', long, help_heading = "Prompt")]
    editor: bool,

    /// sonnet | opus | haiku | full model id.
    #[arg(short = 'm', long, value_name = "MODEL", help_heading = "Model")]
    model: Option<String>,

    /// low | medium | high | xhigh | max.
    #[arg(long, value_name = "LEVEL", help_heading = "Model")]
    effort: Option<String>,

    /// Full structured result (JSON) instead of prose.
    #[arg(long, help_heading = "Output")]
    json: bool,

    /// Constrain the answer to a JSON Schema file (implies --json).
    #[arg(long, value_name = "FILE", help_heading = "Output")]
    schema: Option<PathBuf>,

    /// Answer only: no footer.
    #[arg(short = 'q', long, help_heading = "Output")]
    quiet: bool,

    /// Continue the most recent session in this dir.
    #[arg(long, help_heading = "Session")]
    r#continue: bool,

    /// Resume a specific session id.
    #[arg(long, value_name = "ID", help_heading = "Session")]
    resume: Option<String>,

    /// Mint a new session with an id you choose (for scripted multi-turn).
    #[arg(
        long,
        value_name = "UUID",
        help_heading = "Session",
        conflicts_with_all = ["resume", "continue"]
    )]
    session_id: Option<String>,

    /// Run as if from PATH (git -C style); resolved first.
    #[arg(
        short = 'C',
        long,
        value_name = "PATH",
        help_heading = "Location & isolation"
    )]
    cwd: Option<PathBuf>,

    /// Run in a fresh isolated git worktree (anonymous).
    #[arg(long, help_heading = "Location & isolation")]
    worktree: bool,

    /// ...with an explicit worktree/branch name.
    #[arg(long, value_name = "NAME", help_heading = "Location & isolation")]
    worktree_name: Option<String>,

    /// Seal ambient ~/.claude config (reproducible promptspace).
    #[arg(long, help_heading = "Location & isolation")]
    hermetic: bool,

    /// Apply a named profile (project or user config).
    #[arg(long, value_name = "NAME", help_heading = "Profile")]
    profile: Option<String>,

    /// Ignore the auto-applied default profile.
    #[arg(long, help_heading = "Profile")]
    no_profile: bool,

    /// Print the exact `claude` command it would run, then exit.
    #[arg(long, help_heading = "Meta")]
    explain: bool,

    /// Capture the resolved flags into a project profile, then exit.
    #[arg(long, value_name = "NAME", help_heading = "Meta")]
    save: Option<String>,
}

/// The profile-able subset of settings: the keys a `[profile.NAME]` (or
/// `[defaults]`) table may set, and what `--save` writes back. Per-invocation
/// flags (prompt, session, cwd, output mode) are not profile state.
#[derive(Deserialize, Serialize, Debug, Default, Clone)]
struct Settings {
    #[serde(skip_serializing_if = "Option::is_none")]
    model: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    effort: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    hermetic: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    worktree: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    agent: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    append_system_prompt: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    max_budget_usd: Option<f64>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    allowed_tools: Vec<String>,
}

impl Settings {
    /// Overlay `over` onto `self`: any field set in `over` wins. This is the
    /// whole layering mechanism -- `defaults.overlay(profile).overlay(cli)`.
    fn overlay(mut self, over: &Settings) -> Settings {
        if over.model.is_some() {
            self.model = over.model.clone();
        }
        if over.effort.is_some() {
            self.effort = over.effort.clone();
        }
        if over.hermetic.is_some() {
            self.hermetic = over.hermetic;
        }
        if over.worktree.is_some() {
            self.worktree = over.worktree;
        }
        if over.agent.is_some() {
            self.agent = over.agent.clone();
        }
        if over.append_system_prompt.is_some() {
            self.append_system_prompt = over.append_system_prompt.clone();
        }
        if over.max_budget_usd.is_some() {
            self.max_budget_usd = over.max_budget_usd;
        }
        if !over.allowed_tools.is_empty() {
            self.allowed_tools = over.allowed_tools.clone();
        }
        self
    }
}

#[derive(Deserialize, Debug, Default)]
struct ConfigFile {
    default_profile: Option<String>,
    defaults: Option<Settings>,
    #[serde(default)]
    profiles: BTreeMap<String, Settings>,
}

fn load_config(path: &Path) -> ConfigFile {
    match std::fs::read_to_string(path) {
        Ok(text) => toml::from_str(&text).unwrap_or_else(|e| {
            eprintln!("cr: ignoring malformed {}: {e}", path.display());
            ConfigFile::default()
        }),
        Err(_) => ConfigFile::default(),
    }
}

/// User config (`~/.config/cr/config.toml`) is the base; project (`./cr.toml`)
/// layers on top. Two files, no walk-up in the spike.
fn config_paths() -> (Option<PathBuf>, PathBuf) {
    let user = std::env::var_os("HOME")
        .map(|h| PathBuf::from(h).join(".config/cr/config.toml"))
        .filter(|p| p.exists());
    (user, PathBuf::from("cr.toml"))
}

fn main() -> std::process::ExitCode {
    let cli = Cli::parse();
    let (user_path, project_path) = config_paths();
    let user = user_path.as_deref().map(load_config).unwrap_or_default();
    let project = load_config(&project_path);

    match cli.command {
        Some(Command::Profiles) => return cmd_profiles(&user, &project),
        Some(Command::Config { edit }) => {
            return cmd_config(user_path.as_deref(), &project_path, edit);
        }
        None => {}
    }

    match run(cli.run, &user, &project, &project_path) {
        Ok(code) => code,
        Err(e) => {
            eprintln!("cr: {e}");
            std::process::ExitCode::from(1)
        }
    }
}

fn cmd_profiles(user: &ConfigFile, project: &ConfigFile) -> std::process::ExitCode {
    let mut names: BTreeMap<&str, &str> = BTreeMap::new();
    for n in user.profiles.keys() {
        names.insert(n, "user");
    }
    for n in project.profiles.keys() {
        names.insert(n, "project");
    }
    if names.is_empty() {
        eprintln!("no profiles defined (add [profiles.NAME] to cr.toml)");
        return std::process::ExitCode::SUCCESS;
    }
    let default = project
        .default_profile
        .as_deref()
        .or(user.default_profile.as_deref());
    for (name, source) in names {
        let star = if Some(name) == default {
            " (default)"
        } else {
            ""
        };
        println!("{name}  [{source}]{star}");
    }
    std::process::ExitCode::SUCCESS
}

fn cmd_config(user_path: Option<&Path>, project_path: &Path, edit: bool) -> std::process::ExitCode {
    if edit {
        let editor = std::env::var("VISUAL")
            .or_else(|_| std::env::var("EDITOR"))
            .unwrap_or_else(|_| "vi".to_string());
        let status = std::process::Command::new(editor)
            .arg(project_path)
            .status();
        return match status {
            Ok(s) if s.success() => std::process::ExitCode::SUCCESS,
            _ => std::process::ExitCode::from(1),
        };
    }
    // Base first, project second: the order they layer in.
    match user_path {
        Some(p) => println!("user     {}", p.display()),
        None => println!("user     (none; ~/.config/cr/config.toml)"),
    }
    let exists = if project_path.exists() {
        ""
    } else {
        "  (absent)"
    };
    println!("project  {}{exists}", project_path.display());
    std::process::ExitCode::SUCCESS
}

fn run(
    args: RunArgs,
    user: &ConfigFile,
    project: &ConfigFile,
    project_path: &Path,
) -> anyhow::Result<std::process::ExitCode> {
    // 1. Resolve settings: defaults < profile < CLI.
    let mut settings = Settings::default();
    if let Some(d) = &user.defaults {
        settings = settings.overlay(d);
    }
    if let Some(d) = &project.defaults {
        settings = settings.overlay(d);
    }

    // Profile selection: --profile > CR_PROFILE > project default > user default.
    let active = if args.no_profile {
        None
    } else {
        args.profile
            .clone()
            .or_else(|| std::env::var("CR_PROFILE").ok().filter(|s| !s.is_empty()))
            .or_else(|| project.default_profile.clone())
            .or_else(|| user.default_profile.clone())
    };
    if let Some(name) = &active {
        let p = project
            .profiles
            .get(name)
            .or_else(|| user.profiles.get(name));
        match p {
            Some(p) => settings = settings.overlay(p),
            None => anyhow::bail!("unknown profile: {name}"),
        }
    }

    // CLI flags override the profile.
    if args.model.is_some() {
        settings.model = args.model.clone();
    }
    if args.effort.is_some() {
        settings.effort = args.effort.clone();
    }
    if args.hermetic {
        settings.hermetic = Some(true);
    }
    if args.worktree || args.worktree_name.is_some() {
        settings.worktree = Some(true);
    }

    // 2. --save: capture and exit before doing any work.
    if let Some(name) = &args.save {
        save_profile(project_path, name, &settings)?;
        println!("saved [profiles.{name}] to {}", project_path.display());
        return Ok(std::process::ExitCode::SUCCESS);
    }

    // 3. Resolve the prompt (file > positional > stdin).
    let prompt = resolve_prompt(&args)?;

    // 4. Build the Claude client (cwd) and the query.
    let mut builder = Claude::builder();
    if let Some(cwd) = &args.cwd {
        builder = builder.working_dir(cwd);
    }
    let claude = builder.build()?;

    let mut cmd = QueryCommand::new(prompt).prompt_via_stdin(true);
    if let Some(m) = &settings.model {
        cmd = cmd.model(m);
    }
    if let Some(e) = &settings.effort {
        cmd = cmd.effort(parse_effort(e)?);
    }
    if settings.hermetic == Some(true) {
        cmd = cmd.hermetic();
    }
    if let Some(name) = &args.worktree_name {
        cmd = cmd.worktree_named(name);
    } else if settings.worktree == Some(true) {
        cmd = cmd.worktree();
    }
    if let Some(a) = &settings.agent {
        cmd = cmd.agent(a);
    }
    if let Some(sp) = &settings.append_system_prompt {
        cmd = cmd.append_system_prompt(sp);
    }
    if let Some(b) = settings.max_budget_usd {
        cmd = cmd.max_budget_usd(b);
    }
    if !settings.allowed_tools.is_empty() {
        cmd = cmd.allowed_tools(settings.allowed_tools.iter().map(String::as_str));
    }
    if args.r#continue {
        cmd = cmd.continue_session();
    }
    if let Some(id) = &args.resume {
        cmd = cmd.resume(id);
    }
    if let Some(id) = &args.session_id {
        cmd = cmd.session_id(id);
    }
    if let Some(schema_path) = &args.schema {
        let schema = std::fs::read_to_string(schema_path)?;
        cmd = cmd.json_schema(schema);
    }

    // 5. --explain: print the command it would run, no spawn.
    if args.explain {
        println!("{}", cmd.to_command_string(&claude));
        return Ok(std::process::ExitCode::SUCCESS);
    }

    // 6. Run (buffered; streaming is a deliberate spike gap) and render.
    let result = cmd.execute_json_sync(&claude)?;

    if args.json || args.schema.is_some() {
        println!("{}", serde_json::to_string_pretty(&result)?);
    } else {
        print!("{}", result.result);
        if !result.result.ends_with('\n') {
            println!();
        }
    }

    if !args.quiet && !args.json {
        eprintln!("{}", footer(&settings, &result));
    }

    Ok(if result.is_error {
        std::process::ExitCode::from(1)
    } else {
        std::process::ExitCode::SUCCESS
    })
}

fn resolve_prompt(args: &RunArgs) -> anyhow::Result<String> {
    if args.editor {
        anyhow::bail!("-e/--editor is not wired in this spike; pass a prompt or pipe stdin");
    }
    if let Some(f) = &args.file {
        return Ok(std::fs::read_to_string(f)?);
    }
    if let Some(p) = &args.prompt {
        return Ok(p.clone());
    }
    use std::io::Read;
    let mut buf = String::new();
    std::io::stdin().read_to_string(&mut buf)?;
    if buf.trim().is_empty() {
        anyhow::bail!("no prompt: pass a positional prompt, -f FILE, or pipe stdin");
    }
    Ok(buf)
}

fn parse_effort(s: &str) -> anyhow::Result<Effort> {
    Ok(match s {
        "low" => Effort::Low,
        "medium" => Effort::Medium,
        "high" => Effort::High,
        "xhigh" => Effort::Xhigh,
        "max" => Effort::Max,
        other => anyhow::bail!("unknown effort '{other}' (low|medium|high|xhigh|max)"),
    })
}

fn footer(settings: &Settings, r: &claude_wrapper::QueryResult) -> String {
    let model = actual_model(settings, r);
    let turns = r
        .num_turns
        .map(|n| format!("{n} turns"))
        .unwrap_or_default();
    let cost = r.cost_usd.map(|c| format!("${c:.4}")).unwrap_or_default();
    let dur = r
        .duration_ms
        .map(|d| format!("{:.1}s", d as f64 / 1000.0))
        .unwrap_or_default();
    [model, turns, cost, dur]
        .into_iter()
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join(" · ")
}

/// The model(s) the CLI actually billed, from the result's `modelUsage` map
/// (keyed by model id). Falls back to the configured model, then "default" --
/// so even a bare run reports what ran, not what you happened to set.
fn actual_model(settings: &Settings, r: &claude_wrapper::QueryResult) -> String {
    if let Some(serde_json::Value::Object(usage)) = r.extra.get("modelUsage")
        && !usage.is_empty()
    {
        return usage.keys().cloned().collect::<Vec<_>>().join("+");
    }
    settings
        .model
        .clone()
        .unwrap_or_else(|| "default".to_string())
}

/// Write `settings` as `[profiles.NAME]` into the project cr.toml, preserving
/// whatever else is already there.
fn save_profile(path: &Path, name: &str, settings: &Settings) -> anyhow::Result<()> {
    let mut doc: toml::Table = std::fs::read_to_string(path)
        .ok()
        .and_then(|t| t.parse().ok())
        .unwrap_or_default();
    let profiles = doc
        .entry("profiles".to_string())
        .or_insert_with(|| toml::Value::Table(toml::Table::new()));
    if let toml::Value::Table(t) = profiles {
        t.insert(name.to_string(), toml::Value::try_from(settings)?);
    }
    std::fs::write(path, toml::to_string_pretty(&doc)?)?;
    Ok(())
}
