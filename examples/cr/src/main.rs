//! `cr` -- a config-driven CLI over `claude-wrapper`.
//!
//! A saved `claude -p` you can re-run: name a bundle of flags (and optionally a
//! prompt) as a profile, then repeat it with a word. One concept -- the profile
//! -- carries the whole surface.
//!
//! - per-option layering `defaults < profile < CR_<KEY> env < CLI flag`; an
//!   explicit flag always wins
//! - alias profiles: a `[profiles.NAME]` that carries a `prompt` template is
//!   invocable positionally (`cr review foo.rs`), with `{{args}}`/`{{1}}`/
//!   `{{stdin}}` substitution
//! - `-e` editor compose ($VISUAL/$EDITOR on a `.md` scratch file)
//! - `--explain` (dry-run: print the exact `claude` command), no spawn
//! - `--save NAME` (creation-by-use: capture resolved flags, and a supplied
//!   prompt, into a profile)
//! - the cost/turns footer, from the parsed `QueryResult`
//!
//! Stays passthrough-thin: no host-side prompt assembly beyond template
//! substitution (no file/git composition).
//!
//! Install:  `cargo install claude-cr`  (installs the `cr` binary)
//! Run:      `cr --help`  /  `cr --profile cheap --explain "summarize this"`

use std::collections::BTreeMap;
use std::io::IsTerminal;
use std::path::{Path, PathBuf};

use clap::{Parser, Subcommand};
use claude_wrapper::streaming::{BlockDelta, PartialMessageEvent, stream_query};
use claude_wrapper::{Claude, ClaudeCommand, Effort, OutputFormat, PermissionMode, QueryCommand};
use serde::{Deserialize, Serialize};

mod jobs;
mod repl;

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
    /// List profiles, or show what NAME resolves to (defaults + profile).
    Profiles {
        /// Print the resolved settings for one profile, as TOML.
        name: Option<String>,
    },
    /// Show the config file paths, or open the project one in $EDITOR.
    Config {
        /// Open the project cr.toml in $EDITOR (creating it if absent).
        #[arg(long)]
        edit: bool,
    },
    /// Open an interactive multi-turn session (one `claude` child held open).
    Repl(ReplArgs),
    /// List background jobs (cr's detached jobs and Claude Code's own).
    Jobs,
    /// Show one background job: its progress, cost, and resume command.
    Job {
        /// Job id (or a unique prefix), or a session name.
        selector: String,
        /// Tail the journal until the job finishes.
        #[arg(short, long)]
        follow: bool,
        /// Print the raw stream-json journal instead of a rendered view.
        #[arg(long)]
        json: bool,
    },
}

/// Options that seed the initial REPL session. Everything else (tools,
/// permissions, budget, ...) comes from the resolved profile/config/env, and
/// can be retuned live with `/model`, `/effort`, `/profile`.
#[derive(clap::Args, Debug, Default)]
struct ReplArgs {
    /// Apply a named profile to the opening session.
    #[arg(long, value_name = "NAME")]
    profile: Option<String>,

    /// Ignore the auto-applied default profile.
    #[arg(long)]
    no_profile: bool,

    /// sonnet | opus | haiku | full model id.
    #[arg(short = 'm', long, value_name = "MODEL")]
    model: Option<String>,

    /// low | medium | high | xhigh | max.
    #[arg(long, value_name = "LEVEL")]
    effort: Option<String>,

    /// Run as if from PATH (git -C style).
    #[arg(short = 'C', long, value_name = "PATH")]
    cwd: Option<PathBuf>,

    /// Reconnect to an existing session id (e.g. a finished job's session).
    #[arg(long, value_name = "ID")]
    resume: Option<String>,

    /// Run a command (a prompt or a /command) then exit; repeatable, in order.
    /// Suppresses the banner. Exit status is non-zero if any command errored.
    #[arg(short = 'e', long = "exec", value_name = "CMD")]
    exec: Vec<String>,
}

#[derive(clap::Args, Debug, Default)]
struct RunArgs {
    /// Prompt text, or an alias-profile name (see `cr profiles`). Omit to read
    /// stdin; or use -f / -e.
    #[arg(value_name = "PROMPT_OR_ALIAS")]
    prompt: Option<String>,

    /// Template arguments for an alias profile (`{{args}}`, `{{1}}`, ...).
    #[arg(value_name = "ARG", help_heading = "Prompt")]
    extra: Vec<String>,

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

    /// Fall back to this model if the primary is overloaded.
    #[arg(long, value_name = "MODEL", help_heading = "Model")]
    fallback_model: Option<String>,

    /// Pin a subagent / persona by name.
    #[arg(long, value_name = "NAME", help_heading = "Model")]
    agent: Option<String>,

    /// Append text to the system prompt.
    #[arg(long, value_name = "TEXT", help_heading = "Model")]
    append_system_prompt: Option<String>,

    /// Full structured result (JSON) instead of prose.
    #[arg(long, help_heading = "Output")]
    json: bool,

    /// Constrain the answer to a JSON Schema file (implies --json).
    #[arg(long, value_name = "FILE", help_heading = "Output")]
    schema: Option<PathBuf>,

    /// Answer only: no footer.
    #[arg(short = 'q', long, help_heading = "Output")]
    quiet: bool,

    /// Force live streaming (default: auto -- on a TTY, off on a pipe).
    #[arg(long, help_heading = "Output", conflicts_with = "no_stream")]
    stream: bool,

    /// Force buffered output.
    #[arg(long, help_heading = "Output")]
    no_stream: bool,

    /// Allow a tool pattern (repeatable). Replaces any from config/env.
    #[arg(
        long = "allow-tool",
        value_name = "PATTERN",
        help_heading = "Tools & permissions"
    )]
    allow_tool: Vec<String>,

    /// Deny a tool pattern (repeatable). Replaces any from config/env.
    #[arg(
        long = "disallow-tool",
        value_name = "PATTERN",
        help_heading = "Tools & permissions"
    )]
    disallow_tool: Vec<String>,

    /// Permission mode: default | acceptEdits | plan | auto | dontAsk.
    #[arg(long, value_name = "MODE", help_heading = "Tools & permissions")]
    permission_mode: Option<String>,

    /// Shortcut for --permission-mode acceptEdits.
    #[arg(
        long,
        help_heading = "Tools & permissions",
        conflicts_with = "permission_mode"
    )]
    accept_edits: bool,

    /// Shortcut for --permission-mode plan (read-only).
    #[arg(
        long,
        help_heading = "Tools & permissions",
        conflicts_with_all = ["permission_mode", "accept_edits"]
    )]
    plan: bool,

    /// Extra directory the agent may access (repeatable).
    #[arg(
        long = "add-dir",
        value_name = "PATH",
        help_heading = "Tools & permissions"
    )]
    add_dir: Vec<String>,

    /// Load MCP servers from a config file.
    #[arg(long, value_name = "PATH", help_heading = "Tools & permissions")]
    mcp_config: Option<PathBuf>,

    /// Cap the number of agentic turns.
    #[arg(long, value_name = "N", help_heading = "Limits")]
    max_turns: Option<u32>,

    /// Per-run budget ceiling in USD.
    #[arg(long, value_name = "USD", help_heading = "Limits")]
    max_budget_usd: Option<f64>,

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

    /// Launch this run as a detached background job, then exit (see `cr jobs`).
    #[arg(short = 'd', long, help_heading = "Background")]
    detach: bool,

    /// Name/label a detached job, or address one for --check.
    #[arg(long, value_name = "NAME", help_heading = "Background")]
    session: Option<String>,

    /// Allow a detached job with no budget or turn cap (unattended, risky).
    #[arg(long, help_heading = "Background")]
    uncapped: bool,

    /// Show a background job instead of running: by --session NAME or a
    /// positional id, or list all if neither is given.
    #[arg(long, help_heading = "Background")]
    check: bool,

    /// With --check or a detached run, tail the job until it finishes.
    #[arg(long, help_heading = "Background")]
    follow: bool,

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
    /// A saved prompt template. A profile with a `prompt` is an *alias*:
    /// invocable positionally (`cr NAME [args]`) with `{{args}}`/`{{N}}`/
    /// `{{stdin}}` substitution.
    #[serde(skip_serializing_if = "Option::is_none")]
    prompt: Option<String>,
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
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    disallowed_tools: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    max_turns: Option<u32>,
    /// Permission mode string (`default`/`acceptEdits`/`plan`/`auto`/`dontAsk`);
    /// validated at command-build time, like `effort`.
    #[serde(skip_serializing_if = "Option::is_none")]
    permission_mode: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    fallback_model: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    mcp_config: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    add_dir: Vec<String>,
}

impl Settings {
    /// Overlay `over` onto `self`: any field set in `over` wins. This is the
    /// whole layering mechanism -- `defaults.overlay(profile).overlay(cli)`.
    fn overlay(mut self, over: &Settings) -> Settings {
        if over.prompt.is_some() {
            self.prompt = over.prompt.clone();
        }
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
        if !over.disallowed_tools.is_empty() {
            self.disallowed_tools = over.disallowed_tools.clone();
        }
        if over.max_turns.is_some() {
            self.max_turns = over.max_turns;
        }
        if over.permission_mode.is_some() {
            self.permission_mode = over.permission_mode.clone();
        }
        if over.fallback_model.is_some() {
            self.fallback_model = over.fallback_model.clone();
        }
        if over.mcp_config.is_some() {
            self.mcp_config = over.mcp_config.clone();
        }
        if !over.add_dir.is_empty() {
            self.add_dir = over.add_dir.clone();
        }
        self
    }
}

#[derive(Deserialize, Debug, Default, Clone)]
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
/// layers on top. Two files, no walk-up.
fn config_paths() -> (Option<PathBuf>, PathBuf) {
    let user = std::env::var_os("HOME")
        .map(|h| PathBuf::from(h).join(".config/cr/config.toml"))
        .filter(|p| p.exists());
    (user, PathBuf::from("cr.toml"))
}

/// The `CR_<KEY>` env layer: a partial `Settings` read from the environment,
/// overlaid between the config file and the CLI flags (so `file < env < flag`).
/// Every profile-able option has a mirror; `CR_PROFILE` is separate (it selects
/// a profile, it is not an option value).
fn env_settings() -> Settings {
    let max_budget_usd = match env_str("CR_MAX_BUDGET_USD") {
        Some(v) => match v.parse() {
            Ok(n) => Some(n),
            Err(_) => {
                eprintln!("cr: ignoring non-numeric CR_MAX_BUDGET_USD={v:?}");
                None
            }
        },
        None => None,
    };
    let max_turns = match env_str("CR_MAX_TURNS") {
        Some(v) => match v.parse() {
            Ok(n) => Some(n),
            Err(_) => {
                eprintln!("cr: ignoring non-numeric CR_MAX_TURNS={v:?}");
                None
            }
        },
        None => None,
    };
    Settings {
        prompt: None,
        model: env_str("CR_MODEL"),
        effort: env_str("CR_EFFORT"),
        hermetic: env_bool("CR_HERMETIC"),
        worktree: env_bool("CR_WORKTREE"),
        agent: env_str("CR_AGENT"),
        append_system_prompt: env_str("CR_APPEND_SYSTEM_PROMPT"),
        max_budget_usd,
        allowed_tools: env_list("CR_ALLOWED_TOOLS"),
        disallowed_tools: env_list("CR_DISALLOWED_TOOLS"),
        max_turns,
        permission_mode: env_str("CR_PERMISSION_MODE"),
        fallback_model: env_str("CR_FALLBACK_MODEL"),
        mcp_config: env_str("CR_MCP_CONFIG"),
        add_dir: env_list("CR_ADD_DIR"),
    }
}

fn env_str(key: &str) -> Option<String> {
    std::env::var(key).ok().filter(|v| !v.is_empty())
}

/// A comma/space-separated list env var (`CR_ALLOWED_TOOLS`, `CR_ADD_DIR`, ...).
fn env_list(key: &str) -> Vec<String> {
    env_str(key)
        .map(|v| {
            v.split([',', ' '])
                .filter(|t| !t.is_empty())
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

/// Parse a boolean env var. `1|true|yes|on` -> true, `0|false|no|off` -> false;
/// anything else (or unset) is treated as unset, with a warning for garbage.
fn env_bool(key: &str) -> Option<bool> {
    let v = env_str(key)?;
    match v.to_ascii_lowercase().as_str() {
        "1" | "true" | "yes" | "on" => Some(true),
        "0" | "false" | "no" | "off" => Some(false),
        _ => {
            eprintln!("cr: ignoring non-boolean {key}={v:?}");
            None
        }
    }
}

/// The CLI-flag layer as a partial `Settings` (the top of `file < env < flag`).
/// Every profile-able option has a flag here, so an explicit flag always wins.
/// `hermetic` and `worktree` are one-way (a flag can turn them on, env/file can
/// set either). `--accept-edits`/`--plan` are shortcuts for `--permission-mode`.
fn cli_settings(args: &RunArgs) -> Settings {
    let permission_mode = if args.plan {
        Some("plan".to_string())
    } else if args.accept_edits {
        Some("acceptEdits".to_string())
    } else {
        args.permission_mode.clone()
    };
    Settings {
        prompt: None,
        model: args.model.clone(),
        effort: args.effort.clone(),
        hermetic: if args.hermetic { Some(true) } else { None },
        worktree: if args.worktree || args.worktree_name.is_some() {
            Some(true)
        } else {
            None
        },
        agent: args.agent.clone(),
        append_system_prompt: args.append_system_prompt.clone(),
        max_budget_usd: args.max_budget_usd,
        allowed_tools: args.allow_tool.clone(),
        disallowed_tools: args.disallow_tool.clone(),
        max_turns: args.max_turns,
        permission_mode,
        fallback_model: args.fallback_model.clone(),
        mcp_config: args.mcp_config.as_ref().map(|p| p.display().to_string()),
        add_dir: args.add_dir.clone(),
    }
}

/// Look up a profile by name: project profiles shadow user profiles.
fn lookup_profile<'a>(
    project: &'a ConfigFile,
    user: &'a ConfigFile,
    name: &str,
) -> Option<&'a Settings> {
    project
        .profiles
        .get(name)
        .or_else(|| user.profiles.get(name))
}

/// Substitute a profile's prompt template. `{{args}}` -> all args joined,
/// `{{1}}`..`{{9}}` -> the Nth arg, `{{stdin}}` -> piped stdin (read only when
/// referenced). A template with no `{{...}}` placeholder appends the args.
fn render_template(template: &str, args: &[String]) -> anyhow::Result<String> {
    if !template.contains("{{") {
        return Ok(if args.is_empty() {
            template.to_string()
        } else {
            format!("{template}\n\n{}", args.join(" "))
        });
    }
    let mut out = template.replace("{{args}}", &args.join(" "));
    for (i, a) in args.iter().enumerate() {
        out = out.replace(&format!("{{{{{}}}}}", i + 1), a);
    }
    if out.contains("{{stdin}}") {
        use std::io::Read;
        let mut buf = String::new();
        std::io::stdin().read_to_string(&mut buf)?;
        out = out.replace("{{stdin}}", buf.trim_end_matches('\n'));
    }
    Ok(out)
}

/// Compose a prompt in `$VISUAL`/`$EDITOR` (fallback `vi`) on a `.md` scratch
/// file. Errors on a non-zero editor exit or an empty buffer.
fn compose_in_editor() -> anyhow::Result<String> {
    let tmp = tempfile::Builder::new()
        .prefix("cr-prompt-")
        .suffix(".md")
        .tempfile()?;
    let editor = std::env::var("VISUAL")
        .or_else(|_| std::env::var("EDITOR"))
        .unwrap_or_else(|_| "vi".to_string());
    let mut parts = editor.split_whitespace();
    let program = parts
        .next()
        .ok_or_else(|| anyhow::anyhow!("empty $EDITOR"))?;
    let status = std::process::Command::new(program)
        .args(parts)
        .arg(tmp.path())
        .status()?;
    if !status.success() {
        anyhow::bail!("editor exited with {status}");
    }
    let body = std::fs::read_to_string(tmp.path())?;
    let trimmed = body.trim().to_string();
    if trimmed.is_empty() {
        anyhow::bail!("editor returned an empty prompt");
    }
    Ok(trimmed)
}

fn main() -> std::process::ExitCode {
    let cli = Cli::parse();
    let (user_path, project_path) = config_paths();
    let user = user_path.as_deref().map(load_config).unwrap_or_default();
    let project = load_config(&project_path);

    // Profiles/config are synchronous; the run and repl paths need the async
    // runtime (the library's execute/stream/duplex API is tokio-backed).
    match cli.command {
        Some(Command::Profiles { name }) => {
            return cmd_profiles(&user, &project, name.as_deref());
        }
        Some(Command::Config { edit }) => {
            return cmd_config(user_path.as_deref(), &project_path, edit);
        }
        // Job inspection reads the on-disk store; no async runtime needed.
        Some(Command::Jobs) => return cmd_jobs(),
        Some(Command::Job {
            ref selector,
            follow,
            json,
        }) => return cmd_job(selector, follow, json),
        _ => {}
    }

    let rt = match tokio::runtime::Runtime::new() {
        Ok(rt) => rt,
        Err(e) => {
            eprintln!("cr: failed to start async runtime: {e}");
            return std::process::ExitCode::from(1);
        }
    };

    let result = rt.block_on(async {
        match cli.command {
            Some(Command::Repl(args)) => repl::run(args, &user, &project, &project_path).await,
            _ => run(cli.run, &user, &project, &project_path).await,
        }
    });

    match result {
        Ok(code) => code,
        Err(e) => {
            eprintln!("cr: {e}");
            std::process::ExitCode::from(1)
        }
    }
}

fn cmd_profiles(
    user: &ConfigFile,
    project: &ConfigFile,
    name: Option<&str>,
) -> std::process::ExitCode {
    // `cr profiles NAME`: show what that profile resolves to. Env and per-run
    // flags are deliberately excluded; this is the profile in isolation, over
    // the config defaults.
    if let Some(name) = name {
        let Some(profile) = lookup_profile(project, user, name) else {
            eprintln!("unknown profile: {name}");
            return std::process::ExitCode::from(1);
        };
        let mut resolved = Settings::default();
        if let Some(d) = &user.defaults {
            resolved = resolved.overlay(d);
        }
        if let Some(d) = &project.defaults {
            resolved = resolved.overlay(d);
        }
        resolved = resolved.overlay(profile);
        match toml::to_string_pretty(&resolved) {
            Ok(t) if t.trim().is_empty() => println!("{name}: (no settings)"),
            Ok(t) => print!("{t}"),
            Err(e) => {
                eprintln!("cr: {e}");
                return std::process::ExitCode::from(1);
            }
        }
        return std::process::ExitCode::SUCCESS;
    }

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
        let alias = lookup_profile(project, user, name)
            .filter(|p| p.prompt.is_some())
            .map(|_| " (alias)")
            .unwrap_or("");
        println!("{name}  [{source}]{star}{alias}");
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

fn cmd_jobs() -> std::process::ExitCode {
    match jobs::list() {
        Ok(list) => {
            jobs::render_list(&list);
            std::process::ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("cr: {e}");
            std::process::ExitCode::from(1)
        }
    }
}

fn cmd_job(selector: &str, follow: bool, json: bool) -> std::process::ExitCode {
    match jobs::resolve(selector).and_then(|rec| jobs::render_job(&rec, follow, json)) {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("cr: {e}");
            std::process::ExitCode::from(1)
        }
    }
}

/// Build the profile-able subset of a `QueryCommand` for a detached job:
/// stream-json output (the journal), plus the resolved settings.
fn build_detach_query(prompt: &str, s: &Settings) -> anyhow::Result<QueryCommand> {
    let mut cmd = QueryCommand::new(prompt.to_string())
        .output_format(OutputFormat::StreamJson)
        .verbose(true);
    if let Some(m) = &s.model {
        cmd = cmd.model(m);
    }
    if let Some(e) = &s.effort {
        cmd = cmd.effort(parse_effort(e)?);
    }
    if s.hermetic == Some(true) {
        cmd = cmd.hermetic();
    }
    if s.worktree == Some(true) {
        cmd = cmd.worktree();
    }
    if let Some(a) = &s.agent {
        cmd = cmd.agent(a);
    }
    if let Some(sp) = &s.append_system_prompt {
        cmd = cmd.append_system_prompt(sp);
    }
    if let Some(b) = s.max_budget_usd {
        cmd = cmd.max_budget_usd(b);
    }
    if !s.allowed_tools.is_empty() {
        cmd = cmd.allowed_tools(s.allowed_tools.iter().map(String::as_str));
    }
    if !s.disallowed_tools.is_empty() {
        cmd = cmd.disallowed_tools(s.disallowed_tools.iter().map(String::as_str));
    }
    if let Some(n) = s.max_turns {
        cmd = cmd.max_turns(n);
    }
    if let Some(pm) = &s.permission_mode {
        cmd = cmd.permission_mode(parse_permission_mode(pm)?);
    }
    if let Some(fm) = &s.fallback_model {
        cmd = cmd.fallback_model(fm);
    }
    if let Some(mc) = &s.mcp_config {
        cmd = cmd.mcp_config(mc);
    }
    for d in &s.add_dir {
        cmd = cmd.add_dir(d);
    }
    Ok(cmd)
}

/// A human note of the cap in force, for the job record and the launch line.
fn cap_note(s: &Settings) -> Option<String> {
    let mut parts = Vec::new();
    if let Some(b) = s.max_budget_usd {
        parts.push(format!("${b:.2}"));
    }
    if let Some(n) = s.max_turns {
        parts.push(format!("{n} turns"));
    }
    (!parts.is_empty()).then(|| parts.join(", "))
}

async fn run(
    args: RunArgs,
    user: &ConfigFile,
    project: &ConfigFile,
    project_path: &Path,
) -> anyhow::Result<std::process::ExitCode> {
    // 0. --check: show a background job (by --session/positional) instead of
    //    running, or list all if no selector was given.
    if args.check {
        let selector = args.session.clone().or_else(|| args.prompt.clone());
        return match selector {
            Some(sel) => {
                let rec = jobs::resolve(&sel)?;
                jobs::render_job(&rec, args.follow, args.json)?;
                Ok(std::process::ExitCode::SUCCESS)
            }
            None => {
                jobs::render_list(&jobs::list()?);
                Ok(std::process::ExitCode::SUCCESS)
            }
        };
    }

    // 1. Alias dispatch: a bare first positional that names a profile carrying
    //    a `prompt` template runs that profile, with the rest as template args.
    //    Only when nothing else already fixes the prompt or the profile.
    let alias = if args.no_profile || args.profile.is_some() || args.file.is_some() || args.editor {
        None
    } else if let Some(word) = &args.prompt {
        lookup_profile(project, user, word)
            .filter(|p| p.prompt.is_some())
            .map(|_| word.clone())
    } else {
        None
    };

    // Extra positionals are template args; they only make sense in alias mode.
    if alias.is_none() && !args.extra.is_empty() {
        let joined = args.extra.join(" ");
        match &args.prompt {
            Some(w) => anyhow::bail!(
                "unexpected arguments after {w:?}: {joined} ({w:?} is not an alias profile)"
            ),
            None => anyhow::bail!("unexpected arguments: {joined}"),
        }
    }

    // 2. Resolve settings, low to high: defaults < profile < CR_<KEY> env < flag.
    let mut settings = Settings::default();
    if let Some(d) = &user.defaults {
        settings = settings.overlay(d);
    }
    if let Some(d) = &project.defaults {
        settings = settings.overlay(d);
    }

    // Profile selection: alias name > --profile > CR_PROFILE > project default
    // > user default. (An alias invocation is an explicit selection.)
    let active = if let Some(name) = &alias {
        Some(name.clone())
    } else if args.no_profile {
        None
    } else {
        args.profile
            .clone()
            .or_else(|| std::env::var("CR_PROFILE").ok().filter(|s| !s.is_empty()))
            .or_else(|| project.default_profile.clone())
            .or_else(|| user.default_profile.clone())
    };
    if let Some(name) = &active {
        match lookup_profile(project, user, name) {
            Some(p) => settings = settings.overlay(p),
            None => anyhow::bail!("unknown profile: {name}"),
        }
    }

    // Env then flags: each wins over the layer below it, per option.
    settings = settings.overlay(&env_settings());
    settings = settings.overlay(&cli_settings(&args));

    // 3. --save: capture and exit before doing any work. A supplied prompt (a
    //    positional string or -f file, but never stdin) is saved as the alias
    //    `prompt`, so `cr "Review {{args}}" --save review` mints an alias.
    if let Some(name) = &args.save {
        let mut to_save = settings.clone();
        if let Some(p) = &args.prompt {
            to_save.prompt = Some(p.clone());
        } else if let Some(f) = &args.file {
            to_save.prompt = Some(std::fs::read_to_string(f)?);
        }
        save_profile(project_path, name, &to_save)?;
        println!("saved [profiles.{name}] to {}", project_path.display());
        return Ok(std::process::ExitCode::SUCCESS);
    }

    // 4. Resolve the prompt. Explicit sources (editor, -f, positional) win;
    //    otherwise an active profile's `prompt` template is the default;
    //    otherwise stdin. In alias mode the template is rendered with the args.
    let prompt = resolve_prompt(&args, &settings, alias.is_some())?;

    // 4. Build the Claude client (cwd) and the query.
    let mut builder = Claude::builder();
    if let Some(cwd) = &args.cwd {
        builder = builder.working_dir(cwd);
    }
    let claude = builder.build()?;

    // 4b. --detach: launch a background job and exit. Unattended agent work
    //     with tool access, so require a cap unless --uncapped.
    if args.detach {
        let cap = cap_note(&settings);
        if cap.is_none() && !args.uncapped {
            anyhow::bail!(
                "detached jobs run unattended: set a cap (--max-budget-usd or --max-turns, \
                 or a profile/config default), or pass --uncapped to override"
            );
        }
        let cmd = build_detach_query(&prompt, &settings)?;
        let record = jobs::launch(
            &claude,
            cmd.args(),
            args.cwd.as_deref(),
            &prompt,
            args.session.clone(),
            settings.model.clone(),
            cap.clone(),
        )?;
        let label = record
            .session_name
            .as_deref()
            .map(|n| format!(" [{n}]"))
            .unwrap_or_default();
        let cap_str = match &cap {
            Some(c) => format!(", cap {c}"),
            None => " (uncapped)".to_string(),
        };
        println!("job {}{label} launched{cap_str}", record.id);
        eprintln!(
            "  cr job {}   (add --follow to tail; cr jobs to list)",
            record.id
        );
        return Ok(std::process::ExitCode::SUCCESS);
    }

    // Streaming vs buffered. Structured output can't token-stream, and
    // --no-stream forces buffered; otherwise --stream forces it on, else auto
    // by whether stdout is a TTY.
    let streaming = if args.json || args.schema.is_some() || args.no_stream {
        false
    } else {
        args.stream || std::io::stdout().is_terminal()
    };

    // The sync streamer reads nothing from stdin (it nulls it), so a streaming
    // run must carry the prompt in argv; a buffered run keeps prompt_via_stdin
    // so the prompt never lands in ps/argv.
    let mut cmd = QueryCommand::new(prompt);
    if streaming {
        cmd = cmd
            .output_format(OutputFormat::StreamJson)
            .include_partial_messages();
    } else {
        cmd = cmd.prompt_via_stdin(true);
    }
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
    if !settings.disallowed_tools.is_empty() {
        cmd = cmd.disallowed_tools(settings.disallowed_tools.iter().map(String::as_str));
    }
    if let Some(n) = settings.max_turns {
        cmd = cmd.max_turns(n);
    }
    if let Some(pm) = &settings.permission_mode {
        cmd = cmd.permission_mode(parse_permission_mode(pm)?);
    }
    if let Some(fm) = &settings.fallback_model {
        cmd = cmd.fallback_model(fm);
    }
    if let Some(mc) = &settings.mcp_config {
        cmd = cmd.mcp_config(mc);
    }
    for dir in &settings.add_dir {
        cmd = cmd.add_dir(dir);
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

    // 6. Run and render.
    let result = if streaming {
        stream_run(&claude, &cmd).await?
    } else {
        cmd.execute_json(&claude).await?
    };

    // Streaming already wrote the answer live; buffered/JSON prints it now.
    if args.json || args.schema.is_some() {
        println!("{}", serde_json::to_string_pretty(&result)?);
    } else if !streaming {
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

/// Stream a run, writing assistant text deltas to stdout as they arrive, and
/// return the final `result` event decoded as a `QueryResult` (for the footer).
async fn stream_run(
    claude: &Claude,
    cmd: &QueryCommand,
) -> anyhow::Result<claude_wrapper::QueryResult> {
    use std::io::Write;

    let mut out = std::io::stdout();
    let mut final_result: Option<claude_wrapper::QueryResult> = None;
    let mut wrote_any = false;

    stream_query(claude, cmd, |ev| {
        if let Some(PartialMessageEvent::BlockDelta {
            delta: BlockDelta::Text(t),
            ..
        }) = ev.partial_message()
        {
            let _ = write!(out, "{t}");
            let _ = out.flush();
            wrote_any = true;
        }
        if ev.is_result() {
            final_result = serde_json::from_value(ev.data.clone()).ok();
        }
    })
    .await?;

    if wrote_any {
        let _ = writeln!(out);
    }
    // No partial-message deltas (older CLI, or a result-only run): fall back to
    // the result text so the answer still appears.
    match final_result {
        Some(r) => {
            if !wrote_any && !r.result.is_empty() {
                println!("{}", r.result);
            }
            Ok(r)
        }
        None => anyhow::bail!("streaming run ended without a result event"),
    }
}

fn resolve_prompt(args: &RunArgs, settings: &Settings, alias: bool) -> anyhow::Result<String> {
    if args.editor {
        return compose_in_editor();
    }
    if let Some(f) = &args.file {
        return Ok(std::fs::read_to_string(f)?);
    }
    if alias {
        // The positional was the alias name; render its template with the args.
        let template = settings.prompt.as_deref().unwrap_or_default();
        return render_template(template, &args.extra);
    }
    if let Some(p) = &args.prompt {
        return Ok(p.clone());
    }
    // No explicit prompt: an active profile's template is the default.
    if let Some(template) = &settings.prompt {
        return render_template(template, &args.extra);
    }
    use std::io::Read;
    let mut buf = String::new();
    std::io::stdin().read_to_string(&mut buf)?;
    if buf.trim().is_empty() {
        anyhow::bail!("no prompt: pass a positional prompt, -f FILE, -e, or pipe stdin");
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

/// Parse a permission-mode string. Accepts the CLI's camelCase spellings and
/// kebab-case aliases. `bypassPermissions` is intentionally omitted: it is the
/// library's gated dangerous path, not a routine flag.
fn parse_permission_mode(s: &str) -> anyhow::Result<PermissionMode> {
    Ok(match s {
        "default" => PermissionMode::Default,
        "acceptEdits" | "accept-edits" => PermissionMode::AcceptEdits,
        "plan" => PermissionMode::Plan,
        "auto" => PermissionMode::Auto,
        "dontAsk" | "dont-ask" => PermissionMode::DontAsk,
        other => anyhow::bail!(
            "unknown permission mode '{other}' (default|acceptEdits|plan|auto|dontAsk)"
        ),
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
