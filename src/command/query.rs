//! The `claude -p` query builder.
//!
//! [`QueryCommand`] is the crate's workhorse: a builder for oneshot
//! print-mode queries covering the full `claude -p` flag surface, with
//! typed output ([`execute`](QueryCommand::execute) /
//! [`execute_json`](QueryCommand::execute_json)) and, under the `sync`
//! feature, blocking peers. Spawn-time flags shared with
//! [`DuplexOptions`](crate::duplex::DuplexOptions) live in a common
//! internal `SharedSpawnArgs`, so the two builders cannot drift.

use crate::Claude;
use crate::command::ClaudeCommand;
use crate::command::spawn_args::SharedSpawnArgs;
use crate::error::Result;
use crate::exec::{self, CommandOutput};
use crate::tool_pattern::ToolPattern;
use crate::types::{Effort, InputFormat, OutputFormat, PermissionMode};

/// Builder for `claude -p <prompt>` (oneshot print-mode queries).
///
/// This is the primary command for programmatic use. It runs a single
/// prompt through Claude and returns the result.
///
/// # Example
///
/// ```no_run
/// use claude_wrapper::{Claude, ClaudeCommand, QueryCommand, OutputFormat};
///
/// # async fn example() -> claude_wrapper::Result<()> {
/// let claude = Claude::builder().build()?;
///
/// let output = QueryCommand::new("explain this error: file not found")
///     .model("sonnet")
///     .output_format(OutputFormat::Json)
///     .max_turns(1)
///     .execute(&claude)
///     .await?;
/// # Ok(())
/// # }
/// ```
#[derive(Debug, Clone)]
pub struct QueryCommand {
    prompt: String,
    // Spawn-time knobs shared with DuplexOptions; the flag emission
    // lives on SharedSpawnArgs so the two builders cannot drift.
    shared: SharedSpawnArgs,
    output_format: Option<OutputFormat>,
    tools: Vec<String>,
    file: Vec<String>,
    include_partial_messages: bool,
    input_format: Option<InputFormat>,
    settings: Option<String>,
    fork_session: bool,
    retry_policy: Option<crate::retry::RetryPolicy>,
    brief: bool,
    debug_filter: Option<String>,
    debug_file: Option<String>,
    betas: Option<String>,
    plugin_dirs: Vec<String>,
    plugin_urls: Vec<String>,
    tmux: bool,
    bare: bool,
    safe_mode: bool,
    disable_slash_commands: bool,
    include_hook_events: bool,
    exclude_dynamic_system_prompt_sections: bool,
    name: Option<String>,
    from_pr: Option<String>,
    prompt_via_stdin: bool,
    verbose: bool,
    prompt_suggestions: bool,
    replay_user_messages: bool,
}

impl QueryCommand {
    /// Create a new query command with the given prompt.
    #[must_use]
    pub fn new(prompt: impl Into<String>) -> Self {
        Self {
            prompt: prompt.into(),
            shared: SharedSpawnArgs::default(),
            output_format: None,
            tools: Vec::new(),
            file: Vec::new(),
            include_partial_messages: false,
            input_format: None,
            settings: None,
            fork_session: false,
            retry_policy: None,
            brief: false,
            debug_filter: None,
            debug_file: None,
            betas: None,
            plugin_dirs: Vec::new(),
            plugin_urls: Vec::new(),
            tmux: false,
            bare: false,
            safe_mode: false,
            disable_slash_commands: false,
            include_hook_events: false,
            exclude_dynamic_system_prompt_sections: false,
            name: None,
            from_pr: None,
            prompt_via_stdin: false,
            verbose: false,
            prompt_suggestions: false,
            replay_user_messages: false,
        }
    }

    /// Set the model to use (e.g. "sonnet", "opus", or a full model ID).
    #[must_use]
    pub fn model(mut self, model: impl Into<String>) -> Self {
        self.shared.model = Some(model.into());
        self
    }

    /// Set a custom system prompt (replaces the default).
    #[must_use]
    pub fn system_prompt(mut self, prompt: impl Into<String>) -> Self {
        self.shared.system_prompt = Some(prompt.into());
        self
    }

    /// Append to the default system prompt.
    #[must_use]
    pub fn append_system_prompt(mut self, prompt: impl Into<String>) -> Self {
        self.shared.append_system_prompt = Some(prompt.into());
        self
    }

    /// Set the output format.
    #[must_use]
    pub fn output_format(mut self, format: OutputFormat) -> Self {
        self.output_format = Some(format);
        self
    }

    /// Set the maximum budget in USD.
    #[must_use]
    pub fn max_budget_usd(mut self, budget: f64) -> Self {
        self.shared.max_budget_usd = Some(budget);
        self
    }

    /// Set the permission mode.
    #[must_use]
    pub fn permission_mode(mut self, mode: PermissionMode) -> Self {
        self.shared.permission_mode = Some(mode);
        self
    }

    /// Add allowed tool patterns.
    ///
    /// Accepts anything convertible into [`ToolPattern`], including
    /// bare strings (e.g. `"Bash"`, `"Bash(git log:*)"`,
    /// `"mcp__my-server__*"`) and values produced by
    /// [`ToolPattern`]'s constructors.
    ///
    /// ```
    /// use claude_wrapper::{QueryCommand, ToolPattern};
    ///
    /// let cmd = QueryCommand::new("hi")
    ///     .allowed_tools(["Bash", "Read"]) // raw strings still work
    ///     .allowed_tool(ToolPattern::tool_with_args("Bash", "git log:*"))
    ///     .allowed_tool(ToolPattern::all("Write"));
    /// ```
    #[must_use]
    pub fn allowed_tools<I, T>(mut self, tools: I) -> Self
    where
        I: IntoIterator<Item = T>,
        T: Into<ToolPattern>,
    {
        self.shared
            .allowed_tools
            .extend(tools.into_iter().map(Into::into));
        self
    }

    /// Add a single allowed tool pattern.
    #[must_use]
    pub fn allowed_tool(mut self, tool: impl Into<ToolPattern>) -> Self {
        self.shared.allowed_tools.push(tool.into());
        self
    }

    /// Add disallowed tool patterns.
    #[must_use]
    pub fn disallowed_tools<I, T>(mut self, tools: I) -> Self
    where
        I: IntoIterator<Item = T>,
        T: Into<ToolPattern>,
    {
        self.shared
            .disallowed_tools
            .extend(tools.into_iter().map(Into::into));
        self
    }

    /// Add a single disallowed tool pattern.
    #[must_use]
    pub fn disallowed_tool(mut self, tool: impl Into<ToolPattern>) -> Self {
        self.shared.disallowed_tools.push(tool.into());
        self
    }

    /// Add an MCP config file path.
    #[must_use]
    pub fn mcp_config(mut self, path: impl Into<String>) -> Self {
        self.shared.mcp_config.push(path.into());
        self
    }

    /// Add an additional directory for tool access.
    #[must_use]
    pub fn add_dir(mut self, dir: impl Into<String>) -> Self {
        self.shared.add_dir.push(dir.into());
        self
    }

    /// Set the effort level.
    #[must_use]
    pub fn effort(mut self, effort: Effort) -> Self {
        self.shared.effort = Some(effort);
        self
    }

    /// Set the maximum number of turns.
    #[must_use]
    pub fn max_turns(mut self, turns: u32) -> Self {
        self.shared.max_turns = Some(turns);
        self
    }

    /// Set a JSON schema for structured output validation.
    #[must_use]
    pub fn json_schema(mut self, schema: impl Into<String>) -> Self {
        self.shared.json_schema = Some(schema.into());
        self
    }

    /// Continue the most recent conversation.
    #[must_use]
    pub fn continue_session(mut self) -> Self {
        self.shared.continue_session = true;
        self
    }

    /// Resume a specific session by ID.
    #[must_use]
    pub fn resume(mut self, session_id: impl Into<String>) -> Self {
        self.shared.resume = Some(session_id.into());
        self
    }

    /// Use a specific session ID.
    #[must_use]
    pub fn session_id(mut self, id: impl Into<String>) -> Self {
        self.shared.session_id = Some(id.into());
        self
    }

    /// Clear every session-related flag and set `--resume` to the given id.
    ///
    /// Used by `Session::execute` to override whatever session flags the
    /// caller may have set on their command (including a stale `--resume`,
    /// `--continue`, `--session-id`, or `--fork-session`). Keeping the
    /// override logic in one place prevents conflicting flags from reaching
    /// the CLI.
    #[cfg(all(feature = "json", feature = "async"))]
    pub(crate) fn replace_session(mut self, id: impl Into<String>) -> Self {
        self.shared.continue_session = false;
        self.shared.resume = Some(id.into());
        self.shared.session_id = None;
        self.fork_session = false;
        self
    }

    /// Set a fallback model for when the primary model is overloaded.
    #[must_use]
    pub fn fallback_model(mut self, model: impl Into<String>) -> Self {
        self.shared.fallback_model = Some(model.into());
        self
    }

    /// Disable session persistence (sessions won't be saved to disk).
    #[must_use]
    pub fn no_session_persistence(mut self) -> Self {
        self.shared.no_session_persistence = true;
        self
    }

    /// Bypass all permission checks. Only use in sandboxed environments.
    #[must_use]
    pub fn dangerously_skip_permissions(mut self) -> Self {
        self.shared.dangerously_skip_permissions = true;
        self
    }

    /// Pin the session to a named subagent (`--agent <name>`).
    ///
    /// `name` is resolved by the CLI in this order: inline
    /// definitions from [`Self::agents_json`], then user-level
    /// `~/.claude/agents/<name>.md` files, then project-level dirs
    /// loaded by the active `--setting-sources`.
    ///
    /// **Caveat**: as of Claude Code 2.1.143, the CLI silently
    /// ignores an unknown `name` and falls back to the default
    /// behavior -- no warning, no error. Callers that want a hard
    /// "agent must exist" semantics should validate the name out of
    /// band (e.g. via [`crate::artifacts::AgentsRoot::get`]) before
    /// passing it here.
    #[must_use]
    pub fn agent(mut self, agent: impl Into<String>) -> Self {
        self.shared.agent = Some(agent.into());
        self
    }

    /// Inline subagent definitions for this session
    /// (`--agents <json>`).
    ///
    /// `json` is a JSON object keyed by agent name, with each value
    /// carrying at least `description` and `prompt`. Inline
    /// definitions take precedence over on-disk
    /// `~/.claude/agents/*.md` of the same name. Pass [`Self::agent`]
    /// to select which one to use as the session's persona.
    ///
    /// Example: `{"reviewer": {"description": "Reviews code",
    /// "prompt": "You are a code reviewer"}}`.
    #[must_use]
    pub fn agents_json(mut self, json: impl Into<String>) -> Self {
        self.shared.agents_json = Some(json.into());
        self
    }

    /// Set the list of available built-in tools.
    ///
    /// Use `""` to disable all tools, `"default"` for all tools, or
    /// specific tool names like `["Bash", "Edit", "Read"]`.
    /// This is different from `allowed_tools` which controls MCP tool permissions.
    #[must_use]
    pub fn tools(mut self, tools: impl IntoIterator<Item = impl Into<String>>) -> Self {
        self.tools.extend(tools.into_iter().map(Into::into));
        self
    }

    /// Add a file resource to download at startup.
    ///
    /// Format: `file_id:relative_path` (e.g. `file_abc:doc.txt`).
    #[must_use]
    pub fn file(mut self, spec: impl Into<String>) -> Self {
        self.file.push(spec.into());
        self
    }

    /// Include partial message chunks as they arrive.
    ///
    /// Only works with `--output-format stream-json`.
    #[must_use]
    pub fn include_partial_messages(mut self) -> Self {
        self.include_partial_messages = true;
        self
    }

    /// Set the input format.
    #[must_use]
    pub fn input_format(mut self, format: InputFormat) -> Self {
        self.input_format = Some(format);
        self
    }

    /// Only use MCP servers from `--mcp-config`, ignoring all other MCP configurations.
    #[must_use]
    pub fn strict_mcp_config(mut self) -> Self {
        self.shared.strict_mcp_config = true;
        self
    }

    /// Path to a settings JSON file or a JSON string.
    #[must_use]
    pub fn settings(mut self, settings: impl Into<String>) -> Self {
        self.settings = Some(settings.into());
        self
    }

    /// When resuming, create a new session ID instead of reusing the original.
    #[must_use]
    pub fn fork_session(mut self) -> Self {
        self.fork_session = true;
        self
    }

    /// Create a new git worktree for this session, providing an isolated working directory.
    #[must_use]
    pub fn worktree(mut self) -> Self {
        self.shared.worktree = true;
        self
    }

    /// Create a new git worktree with an explicit name, providing an
    /// isolated working directory.
    ///
    /// Equivalent to [`Self::worktree`] but emits `--worktree NAME`,
    /// pinning the worktree's directory/branch name rather than
    /// letting the CLI auto-generate one.
    ///
    /// # Example
    ///
    /// ```no_run
    /// use claude_wrapper::{Claude, ClaudeCommand, QueryCommand};
    ///
    /// # async fn example() -> claude_wrapper::Result<()> {
    /// let claude = Claude::builder().build()?;
    ///
    /// let output = QueryCommand::new("refactor the parser")
    ///     .worktree_named("parser-refactor")
    ///     .execute(&claude)
    ///     .await?;
    /// # Ok(())
    /// # }
    /// ```
    #[must_use]
    pub fn worktree_named(mut self, name: impl Into<String>) -> Self {
        self.shared.worktree = true;
        self.shared.worktree_name = Some(name.into());
        self
    }

    /// Enable brief mode, which activates the SendUserMessage tool for agent-to-user communication.
    #[must_use]
    pub fn brief(mut self) -> Self {
        self.brief = true;
        self
    }

    /// Enable debug logging with an optional filter (e.g., "api,hooks").
    #[must_use]
    pub fn debug_filter(mut self, filter: impl Into<String>) -> Self {
        self.debug_filter = Some(filter.into());
        self
    }

    /// Write debug logs to the specified file path.
    #[must_use]
    pub fn debug_file(mut self, path: impl Into<String>) -> Self {
        self.debug_file = Some(path.into());
        self
    }

    /// Beta feature headers for API key authentication.
    #[must_use]
    pub fn betas(mut self, betas: impl Into<String>) -> Self {
        self.betas = Some(betas.into());
        self
    }

    /// Load plugins from the specified directory for this session.
    #[must_use]
    pub fn plugin_dir(mut self, dir: impl Into<String>) -> Self {
        self.plugin_dirs.push(dir.into());
        self
    }

    /// Fetch a plugin `.zip` from a URL for this session only
    /// (`--plugin-url`). Repeatable; the URL-based counterpart to
    /// [`Self::plugin_dir`].
    #[must_use]
    pub fn plugin_url(mut self, url: impl Into<String>) -> Self {
        self.plugin_urls.push(url.into());
        self
    }

    /// Comma-separated list of setting sources to load (e.g., "user,project,local").
    #[must_use]
    pub fn setting_sources(mut self, sources: impl Into<String>) -> Self {
        self.shared.setting_sources = Some(sources.into());
        self
    }

    /// Create a tmux session for the worktree.
    #[must_use]
    pub fn tmux(mut self) -> Self {
        self.tmux = true;
        self
    }

    /// Run in minimal mode (`--bare`).
    ///
    /// Skips hooks, LSP, plugin sync, attribution, auto-memory,
    /// background prefetches, keychain reads, and CLAUDE.md
    /// auto-discovery. Sets `CLAUDE_CODE_SIMPLE=1` inside the child.
    /// Anthropic auth is restricted to `ANTHROPIC_API_KEY` or
    /// `apiKeyHelper` via `--settings`; OAuth and keychain are never
    /// read. Third-party providers (Bedrock/Vertex/Foundry) use their
    /// own credentials as normal.
    ///
    /// Intended for headless/CI use where you want deterministic
    /// context: provide everything explicitly via `--system-prompt`,
    /// `--append-system-prompt`, `--add-dir`, `--mcp-config`,
    /// `--settings`, `--agents`, and `--plugin-dir`. Skills still
    /// resolve via explicit `/skill-name` references.
    #[must_use]
    pub fn bare(mut self) -> Self {
        self.bare = true;
        self
    }

    /// Disable all slash-command skills (`--disable-slash-commands`).
    #[must_use]
    pub fn disable_slash_commands(mut self) -> Self {
        self.disable_slash_commands = true;
        self
    }

    /// Start with all customizations disabled (`--safe-mode`).
    ///
    /// Disables CLAUDE.md, skills, plugins, hooks, MCP servers, custom
    /// commands and agents, output styles, and other customizations for
    /// troubleshooting a broken configuration. Admin-managed (policy)
    /// settings still apply. Auth, model selection, built-in tools, and
    /// permissions work normally. Sets `CLAUDE_CODE_SAFE_MODE=1` inside
    /// the child.
    #[must_use]
    pub fn safe_mode(mut self) -> Self {
        self.safe_mode = true;
        self
    }

    /// Include every hook lifecycle event in the stream-json output
    /// (`--include-hook-events`). Only meaningful with
    /// `OutputFormat::StreamJson`.
    #[must_use]
    pub fn include_hook_events(mut self) -> Self {
        self.include_hook_events = true;
        self
    }

    /// Move per-machine sections (cwd, env info, memory paths, git
    /// status) out of the system prompt and into the first user
    /// message (`--exclude-dynamic-system-prompt-sections`). Improves
    /// cross-user prompt-cache reuse. Only applies with the default
    /// system prompt; ignored with `--system-prompt`.
    #[must_use]
    pub fn exclude_dynamic_system_prompt_sections(mut self) -> Self {
        self.exclude_dynamic_system_prompt_sections = true;
        self
    }

    /// Set a display name for this session (`--name`). Shown in the
    /// prompt box, `/resume` picker, and terminal title.
    #[must_use]
    pub fn name(mut self, name: impl Into<String>) -> Self {
        self.name = Some(name.into());
        self
    }

    /// Resume a session linked to a PR by number or URL
    /// (`--from-pr <value>`).
    ///
    /// This wrapper only supports the valued form; the CLI's
    /// no-value mode opens an interactive picker and would hang a
    /// headless caller.
    #[must_use]
    pub fn from_pr(mut self, pr: impl Into<String>) -> Self {
        self.from_pr = Some(pr.into());
        self
    }

    /// Enable verbose logging (`--verbose`), overriding the CLI's
    /// configured verbose-mode setting.
    ///
    /// Note: [`OutputFormat::StreamJson`] already forces `--verbose`
    /// on (the CLI requires it alongside `--print`), so this builder
    /// only changes behavior for the text and JSON output formats.
    /// Either path emits the flag at most once.
    #[must_use]
    pub fn verbose(mut self, value: bool) -> Self {
        self.verbose = value;
        self
    }

    /// Emit a predicted next-user-prompt after each turn
    /// (`--prompt-suggestions`).
    ///
    /// In print/SDK mode the CLI emits a `prompt_suggestion` message
    /// alongside the normal result. Off by default.
    #[must_use]
    pub fn prompt_suggestions(mut self, value: bool) -> Self {
        self.prompt_suggestions = value;
        self
    }

    /// Re-emit user messages from stdin back on stdout
    /// (`--replay-user-messages`).
    ///
    /// Only meaningful for bidirectional stream-json flows: the CLI
    /// requires both [`InputFormat::StreamJson`] and
    /// [`OutputFormat::StreamJson`] for this to take effect. Off by
    /// default.
    #[must_use]
    pub fn replay_user_messages(mut self, value: bool) -> Self {
        self.replay_user_messages = value;
        self
    }

    /// Set a per-command retry policy, overriding the client default.
    ///
    /// # Example
    ///
    /// ```no_run
    /// use claude_wrapper::{Claude, ClaudeCommand, QueryCommand, RetryPolicy};
    /// use std::time::Duration;
    ///
    /// # async fn example() -> claude_wrapper::Result<()> {
    /// let claude = Claude::builder().build()?;
    ///
    /// let output = QueryCommand::new("explain quicksort")
    ///     .retry(RetryPolicy::new()
    ///         .max_attempts(5)
    ///         .initial_backoff(Duration::from_secs(2))
    ///         .exponential()
    ///         .retry_on_timeout(true))
    ///     .execute(&claude)
    ///     .await?;
    /// # Ok(())
    /// # }
    /// ```
    #[must_use]
    pub fn retry(mut self, policy: crate::retry::RetryPolicy) -> Self {
        self.retry_policy = Some(policy);
        self
    }

    /// Return the full command as a string that could be run in a shell.
    ///
    /// Constructs a command string using the binary path from the Claude instance
    /// and the arguments from this query. Arguments containing spaces or special
    /// shell characters are shell-quoted to be safe for shell execution.
    ///
    /// # Example
    ///
    /// ```no_run
    /// use claude_wrapper::{Claude, QueryCommand};
    ///
    /// # async fn example() -> claude_wrapper::Result<()> {
    /// let claude = Claude::builder().build()?;
    ///
    /// let cmd = QueryCommand::new("explain quicksort")
    ///     .model("sonnet");
    ///
    /// let command_str = cmd.to_command_string(&claude);
    /// println!("Would run: {}", command_str);
    /// # Ok(())
    /// # }
    /// ```
    pub fn to_command_string(&self, claude: &Claude) -> String {
        let args = self.build_args();
        let quoted_args = args.iter().map(|arg| shell_quote(arg)).collect::<Vec<_>>();
        format!("{} {}", claude.binary().display(), quoted_args.join(" "))
    }

    /// Execute the query and parse the JSON result.
    ///
    /// This is a convenience method that sets `OutputFormat::Json` and
    /// deserializes the response into a [`QueryResult`](crate::types::QueryResult).
    #[cfg(all(feature = "json", feature = "async"))]
    pub async fn execute_json(&self, claude: &Claude) -> Result<crate::types::QueryResult> {
        let args = self.build_args_with_forced_json();

        let output = if self.prompt_via_stdin {
            // Retry is skipped for stdin mode: the stdin pipe is consumed
            // after the first attempt and cannot be rewound.
            exec::run_claude_with_stdin_prompt(claude, args, self.prompt.clone()).await?
        } else {
            exec::run_claude_with_retry(claude, args, self.retry_policy.as_ref()).await?
        };

        serde_json::from_str(&output.stdout).map_err(|e| crate::error::Error::Json {
            message: format!("failed to parse query result: {e}"),
            source: e,
        })
    }

    /// Blocking analog of [`QueryCommand::execute`] that honours the
    /// configured [`RetryPolicy`](crate::retry::RetryPolicy).
    ///
    /// Overrides the blanket
    /// [`ClaudeCommandSyncExt::execute_sync`](crate::ClaudeCommandSyncExt)
    /// impl so retries still fire on the sync path.
    #[cfg(feature = "sync")]
    pub fn execute_sync(&self, claude: &Claude) -> Result<CommandOutput> {
        if self.prompt_via_stdin {
            // Retry is skipped for stdin mode: the stdin pipe is consumed
            // after the first attempt and cannot be rewound.
            exec::run_claude_with_stdin_prompt_sync(claude, self.build_args(), self.prompt.clone())
        } else {
            exec::run_claude_with_retry_sync(claude, self.args(), self.retry_policy.as_ref())
        }
    }

    /// Blocking mirror of [`QueryCommand::execute_json`].
    #[cfg(all(feature = "sync", feature = "json"))]
    pub fn execute_json_sync(&self, claude: &Claude) -> Result<crate::types::QueryResult> {
        let args = self.build_args_with_forced_json();

        let output = if self.prompt_via_stdin {
            // Retry is skipped for stdin mode: the stdin pipe is consumed
            // after the first attempt and cannot be rewound.
            exec::run_claude_with_stdin_prompt_sync(claude, args, self.prompt.clone())?
        } else {
            exec::run_claude_with_retry_sync(claude, args, self.retry_policy.as_ref())?
        };

        serde_json::from_str(&output.stdout).map_err(|e| crate::error::Error::Json {
            message: format!("failed to parse query result: {e}"),
            source: e,
        })
    }

    /// Route the prompt through stdin rather than argv.
    ///
    /// When set, the prompt body does not appear in the spawned
    /// process's argument list (`ps`, `/proc/PID/cmdline`, APM
    /// agents). Use this for any prompt that contains sensitive
    /// content: private code, internal design notes, orchestrator
    /// dispatch specs.
    ///
    /// Requires that `claude --print` read from stdin when no
    /// positional prompt is supplied (verified as of claude 2.1.x).
    ///
    /// Note: retry is skipped when stdin mode is active -- the stdin
    /// pipe is consumed after the first attempt and cannot be rewound.
    ///
    /// # Example
    /// ```no_run
    /// use claude_wrapper::{Claude, ClaudeCommand, QueryCommand};
    /// # async fn example() -> claude_wrapper::Result<()> {
    /// let claude = Claude::builder().build()?;
    /// let out = QueryCommand::new("my secret prompt")
    ///     .prompt_via_stdin(true)
    ///     .execute(&claude)
    ///     .await?;
    /// # Ok(()) }
    /// ```
    #[must_use]
    pub fn prompt_via_stdin(mut self, value: bool) -> Self {
        self.prompt_via_stdin = value;
        self
    }

    /// Like [`Self::build_args`], but if `output_format` is unset on
    /// this command, force it to `json`. The naive approach -- call
    /// `build_args` then `args.push("--output-format")` -- breaks
    /// because `build_args` already appended `--` and the prompt at
    /// the end, so the late flag becomes positional and is eaten as
    /// part of the prompt. We clone-and-set instead so the flag
    /// lands in its proper slot before `--`.
    fn build_args_with_forced_json(&self) -> Vec<String> {
        if self.output_format.is_some() {
            return self.build_args();
        }
        let mut effective = self.clone();
        effective.output_format = Some(OutputFormat::Json);
        effective.build_args()
    }

    fn build_args(&self) -> Vec<String> {
        let mut args = vec!["--print".to_string()];

        if let Some(ref format) = self.output_format {
            args.push("--output-format".to_string());
            args.push(format.as_arg().to_string());
        }

        // --verbose: explicit opt-in via `.verbose(true)`, or forced
        // for stream-json (CLI v2.1.72+ requires it with --print).
        // Emitted once so the two paths can't double up the flag.
        if self.verbose || matches!(self.output_format, Some(OutputFormat::StreamJson)) {
            args.push("--verbose".to_string());
        }

        self.shared.append_to(&mut args);

        if !self.tools.is_empty() {
            args.push("--tools".to_string());
            args.push(self.tools.join(","));
        }

        for spec in &self.file {
            args.push("--file".to_string());
            args.push(spec.clone());
        }

        if self.include_partial_messages {
            args.push("--include-partial-messages".to_string());
        }

        if let Some(ref format) = self.input_format {
            args.push("--input-format".to_string());
            args.push(format.as_arg().to_string());
        }

        if let Some(ref settings) = self.settings {
            args.push("--settings".to_string());
            args.push(settings.clone());
        }

        if self.fork_session {
            args.push("--fork-session".to_string());
        }

        if self.brief {
            args.push("--brief".to_string());
        }

        if let Some(ref filter) = self.debug_filter {
            args.push("--debug".to_string());
            args.push(filter.clone());
        }

        if let Some(ref path) = self.debug_file {
            args.push("--debug-file".to_string());
            args.push(path.clone());
        }

        if let Some(ref betas) = self.betas {
            args.push("--betas".to_string());
            args.push(betas.clone());
        }

        for dir in &self.plugin_dirs {
            args.push("--plugin-dir".to_string());
            args.push(dir.clone());
        }

        for url in &self.plugin_urls {
            args.push("--plugin-url".to_string());
            args.push(url.clone());
        }

        if self.tmux {
            args.push("--tmux".to_string());
        }

        if self.bare {
            args.push("--bare".to_string());
        }

        if self.safe_mode {
            args.push("--safe-mode".to_string());
        }

        if self.disable_slash_commands {
            args.push("--disable-slash-commands".to_string());
        }

        if self.include_hook_events {
            args.push("--include-hook-events".to_string());
        }

        if self.exclude_dynamic_system_prompt_sections {
            args.push("--exclude-dynamic-system-prompt-sections".to_string());
        }

        if self.prompt_suggestions {
            args.push("--prompt-suggestions".to_string());
        }

        if self.replay_user_messages {
            args.push("--replay-user-messages".to_string());
        }

        if let Some(ref name) = self.name {
            args.push("--name".to_string());
            args.push(name.clone());
        }

        if let Some(ref pr) = self.from_pr {
            args.push("--from-pr".to_string());
            args.push(pr.clone());
        }

        // Separator to prevent flags like --allowed-tools from consuming the prompt.
        // When prompt_via_stdin is set, the prompt is sent via stdin after spawn
        // rather than appearing in argv (avoids ps/APM/crash-dump leakage).
        if !self.prompt_via_stdin {
            args.push("--".to_string());
            args.push(self.prompt.clone());
        }

        args
    }
}

impl ClaudeCommand for QueryCommand {
    type Output = CommandOutput;

    fn args(&self) -> Vec<String> {
        self.build_args()
    }

    #[cfg(feature = "async")]
    async fn execute(&self, claude: &Claude) -> Result<CommandOutput> {
        if self.prompt_via_stdin {
            // Retry is skipped for stdin mode: the stdin pipe is consumed
            // after the first attempt and cannot be rewound.
            let args = self.build_args(); // prompt not in args
            exec::run_claude_with_stdin_prompt(claude, args, self.prompt.clone()).await
        } else {
            exec::run_claude_with_retry(claude, self.args(), self.retry_policy.as_ref()).await
        }
    }
}

/// Shell-quote an argument if it contains spaces or special characters.
fn shell_quote(arg: &str) -> String {
    // Check if the argument needs quoting (contains whitespace or shell metacharacters)
    if arg.contains(|c: char| c.is_whitespace() || "\"'$\\`|;<>&()[]{}".contains(c)) {
        // Use single quotes and escape any existing single quotes
        format!("'{}'", arg.replace("'", "'\\''"))
    } else {
        arg.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_basic_query_args() {
        let cmd = QueryCommand::new("hello world");
        let args = cmd.args();
        assert_eq!(args, vec!["--print", "--", "hello world"]);
    }

    #[test]
    fn prompt_via_stdin_omits_prompt_from_args() {
        let cmd = QueryCommand::new("secret payload").prompt_via_stdin(true);
        let args = cmd.args();
        assert!(
            !args.contains(&"secret payload".to_string()),
            "prompt must not appear in args when prompt_via_stdin is set"
        );
        assert!(
            !args.contains(&"--".to_string()),
            "-- separator must be absent when prompt_via_stdin is set"
        );
    }

    #[test]
    fn prompt_via_stdin_false_keeps_prompt_in_args() {
        let cmd = QueryCommand::new("visible prompt").prompt_via_stdin(false);
        let args = cmd.args();
        assert!(
            args.contains(&"visible prompt".to_string()),
            "prompt must still appear in args when prompt_via_stdin is false"
        );
        assert!(
            args.contains(&"--".to_string()),
            "-- separator must be present when prompt_via_stdin is false"
        );
    }

    #[test]
    #[cfg(feature = "async")] // uses tokio + the async-only execute()
    #[ignore = "requires a real claude binary"]
    fn prompt_via_stdin_integration() {
        // Verify round-trip: prompt sent via stdin produces a valid response.
        // Run with: cargo test --lib -p claude-wrapper -- --ignored prompt_via_stdin_integration
        use crate::{Claude, ClaudeCommand};
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            let claude = Claude::builder().build().unwrap();
            let out = QueryCommand::new("reply with: STDIN_OK")
                .prompt_via_stdin(true)
                .execute(&claude)
                .await
                .unwrap();
            assert!(
                !out.stdout.is_empty(),
                "expected non-empty output from stdin-mode query"
            );
        });
    }

    #[test]
    fn build_args_with_forced_json_inserts_flag_before_separator() {
        // Regression: prior to this fix, execute_json appended
        // --output-format json AFTER build_args's `-- prompt` tail,
        // so the flag was treated as positional and eaten as part
        // of the prompt. With the fix the flag must land BEFORE the
        // `--` separator.
        let cmd = QueryCommand::new("hello");
        let args = cmd.build_args_with_forced_json();

        // The trailing pair must still be the separator + prompt.
        assert_eq!(
            &args[args.len() - 2..],
            &["--".to_string(), "hello".to_string()],
        );

        // --output-format json must appear BEFORE `--`.
        let sep = args.iter().position(|a| a == "--").expect("`--` present");
        let fmt = args
            .iter()
            .position(|a| a == "--output-format")
            .expect("--output-format present");
        assert!(
            fmt < sep,
            "--output-format must come before `--` separator; got {args:?}"
        );
        assert_eq!(args[fmt + 1], "json");
    }

    #[test]
    fn build_args_with_forced_json_respects_explicit_format() {
        // If the caller already set output_format on the builder,
        // the helper must NOT override it.
        let cmd = QueryCommand::new("hello").output_format(OutputFormat::Text);
        let args = cmd.build_args_with_forced_json();
        let fmt = args
            .iter()
            .position(|a| a == "--output-format")
            .expect("--output-format present");
        assert_eq!(args[fmt + 1], "text");
        // Just one occurrence -- not double-pushed.
        assert_eq!(args.iter().filter(|a| *a == "--output-format").count(), 1);
    }

    #[test]
    #[allow(deprecated)] // exercises PermissionMode::BypassPermissions directly; prefer dangerous::DangerousClient in new code
    fn test_full_query_args() {
        let cmd = QueryCommand::new("explain this")
            .model("sonnet")
            .system_prompt("be concise")
            .output_format(OutputFormat::Json)
            .max_budget_usd(0.50)
            .permission_mode(PermissionMode::BypassPermissions)
            .allowed_tools(["Bash", "Read"])
            .mcp_config("/tmp/mcp.json")
            .effort(Effort::High)
            .max_turns(3)
            .no_session_persistence();

        let args = cmd.args();
        assert!(args.contains(&"--print".to_string()));
        assert!(args.contains(&"--model".to_string()));
        assert!(args.contains(&"sonnet".to_string()));
        assert!(args.contains(&"--system-prompt".to_string()));
        assert!(args.contains(&"--output-format".to_string()));
        assert!(args.contains(&"json".to_string()));
        // json format should NOT include --verbose (only stream-json needs it)
        assert!(!args.contains(&"--verbose".to_string()));
        assert!(args.contains(&"--max-budget-usd".to_string()));
        assert!(args.contains(&"--permission-mode".to_string()));
        assert!(args.contains(&"bypassPermissions".to_string()));
        assert!(args.contains(&"--allowed-tools".to_string()));
        assert!(args.contains(&"Bash,Read".to_string()));
        assert!(args.contains(&"--effort".to_string()));
        assert!(args.contains(&"high".to_string()));
        assert!(args.contains(&"--max-turns".to_string()));
        assert!(args.contains(&"--no-session-persistence".to_string()));
        // Prompt is last, preceded by -- separator
        assert_eq!(args.last().unwrap(), "explain this");
        assert_eq!(args[args.len() - 2], "--");
    }

    #[test]
    fn typed_patterns_render_in_allowed_tools() {
        use crate::ToolPattern;

        let cmd = QueryCommand::new("hi")
            .allowed_tool(ToolPattern::tool("Read"))
            .allowed_tool(ToolPattern::tool_with_args("Bash", "git log:*"))
            .allowed_tool(ToolPattern::all("Write"))
            .allowed_tool(ToolPattern::mcp("srv", "*"));

        let args = cmd.args();
        let joined = args
            .iter()
            .position(|a| a == "--allowed-tools")
            .map(|i| &args[i + 1])
            .unwrap();
        assert_eq!(joined, "Read,Bash(git log:*),Write(*),mcp__srv__*");
    }

    #[test]
    fn disallowed_tool_singular_appends() {
        use crate::ToolPattern;

        let cmd = QueryCommand::new("hi")
            .disallowed_tool("Write")
            .disallowed_tool(ToolPattern::tool_with_args("Bash", "rm*"));

        let args = cmd.args();
        let joined = args
            .iter()
            .position(|a| a == "--disallowed-tools")
            .map(|i| &args[i + 1])
            .unwrap();
        assert_eq!(joined, "Write,Bash(rm*)");
    }

    #[test]
    fn mixed_string_and_typed_patterns_both_accepted() {
        use crate::ToolPattern;

        // Smoke test for API ergonomics: one plural call with mixed
        // inputs should compile even though the builder is generic
        // over T: Into<ToolPattern>.
        let strs: Vec<ToolPattern> = vec!["Bash".into(), ToolPattern::all("Read")];
        let cmd = QueryCommand::new("hi").allowed_tools(strs);
        assert!(cmd.args().contains(&"--allowed-tools".to_string()));
    }

    #[test]
    fn new_bool_flags_emit_correct_cli_args() {
        let args = QueryCommand::new("hi")
            .bare()
            .disable_slash_commands()
            .include_hook_events()
            .exclude_dynamic_system_prompt_sections()
            .args();
        assert!(args.contains(&"--bare".to_string()));
        assert!(args.contains(&"--disable-slash-commands".to_string()));
        assert!(args.contains(&"--include-hook-events".to_string()));
        assert!(args.contains(&"--exclude-dynamic-system-prompt-sections".to_string()));
    }

    #[test]
    fn name_flag_renders_with_value() {
        let args = QueryCommand::new("hi").name("my session").args();
        let pos = args.iter().position(|a| a == "--name").unwrap();
        assert_eq!(args[pos + 1], "my session");
    }

    #[test]
    fn from_pr_flag_renders_with_value() {
        let args = QueryCommand::new("hi").from_pr("42").args();
        let pos = args.iter().position(|a| a == "--from-pr").unwrap();
        assert_eq!(args[pos + 1], "42");
    }

    #[test]
    fn new_bool_flags_default_to_off() {
        let args = QueryCommand::new("hi").args();
        assert!(!args.contains(&"--bare".to_string()));
        assert!(!args.contains(&"--disable-slash-commands".to_string()));
        assert!(!args.contains(&"--include-hook-events".to_string()));
        assert!(!args.contains(&"--exclude-dynamic-system-prompt-sections".to_string()));
        assert!(!args.contains(&"--name".to_string()));
    }

    #[test]
    fn test_separator_before_prompt_prevents_greedy_flag_parsing() {
        // Regression: --allowed-tools was consuming the prompt as a tool name
        // when the prompt appeared after it without a -- separator.
        let cmd = QueryCommand::new("fix the bug")
            .allowed_tools(["Read", "Edit", "Bash(cargo *)"])
            .output_format(OutputFormat::StreamJson);
        let args = cmd.args();
        // -- separator must appear before the prompt
        let sep_pos = args.iter().position(|a| a == "--").unwrap();
        let prompt_pos = args.iter().position(|a| a == "fix the bug").unwrap();
        assert_eq!(prompt_pos, sep_pos + 1, "prompt must follow -- separator");
        // --allowed-tools value must appear before the separator
        let tools_pos = args
            .iter()
            .position(|a| a.contains("Bash(cargo *)"))
            .unwrap();
        assert!(
            tools_pos < sep_pos,
            "allowed-tools must come before -- separator"
        );
    }

    #[test]
    fn test_stream_json_includes_verbose() {
        let cmd = QueryCommand::new("test").output_format(OutputFormat::StreamJson);
        let args = cmd.args();
        assert!(args.contains(&"--output-format".to_string()));
        assert!(args.contains(&"stream-json".to_string()));
        assert!(args.contains(&"--verbose".to_string()));
    }

    #[test]
    fn verbose_flag_emitted_when_set() {
        let args = QueryCommand::new("test").verbose(true).args();
        assert!(args.contains(&"--verbose".to_string()));
    }

    #[test]
    fn verbose_absent_by_default_and_when_false() {
        assert!(
            !QueryCommand::new("test")
                .args()
                .contains(&"--verbose".to_string())
        );
        assert!(
            !QueryCommand::new("test")
                .verbose(false)
                .args()
                .contains(&"--verbose".to_string())
        );
    }

    #[test]
    fn verbose_not_duplicated_with_stream_json() {
        // stream-json forces --verbose; an explicit .verbose(true) must
        // not push a second copy.
        let cmd = QueryCommand::new("test")
            .verbose(true)
            .output_format(OutputFormat::StreamJson);
        let count = cmd.args().iter().filter(|a| *a == "--verbose").count();
        assert_eq!(count, 1, "--verbose must appear exactly once");
    }

    #[test]
    fn prompt_suggestions_flag_emitted_when_set() {
        let args = QueryCommand::new("test").prompt_suggestions(true).args();
        assert!(args.contains(&"--prompt-suggestions".to_string()));
        // Must land before the `--` separator, not be eaten as part of
        // the prompt.
        let sep = args.iter().position(|a| a == "--").unwrap();
        let flag = args
            .iter()
            .position(|a| a == "--prompt-suggestions")
            .unwrap();
        assert!(flag < sep, "--prompt-suggestions must precede `--`");
    }

    #[test]
    fn prompt_suggestions_absent_by_default_and_when_false() {
        assert!(
            !QueryCommand::new("test")
                .args()
                .contains(&"--prompt-suggestions".to_string())
        );
        assert!(
            !QueryCommand::new("test")
                .prompt_suggestions(false)
                .args()
                .contains(&"--prompt-suggestions".to_string())
        );
    }

    #[test]
    fn replay_user_messages_flag_emitted_when_set() {
        let args = QueryCommand::new("test").replay_user_messages(true).args();
        assert!(args.contains(&"--replay-user-messages".to_string()));
    }

    #[test]
    fn replay_user_messages_absent_by_default_and_when_false() {
        assert!(
            !QueryCommand::new("test")
                .args()
                .contains(&"--replay-user-messages".to_string())
        );
        assert!(
            !QueryCommand::new("test")
                .replay_user_messages(false)
                .args()
                .contains(&"--replay-user-messages".to_string())
        );
    }

    #[test]
    fn test_to_command_string_simple() {
        let claude = Claude::builder()
            .binary("/usr/local/bin/claude")
            .build()
            .unwrap();

        let cmd = QueryCommand::new("hello");
        let command_str = cmd.to_command_string(&claude);

        assert!(command_str.starts_with("/usr/local/bin/claude"));
        assert!(command_str.contains("--print"));
        assert!(command_str.contains("hello"));
    }

    #[test]
    fn test_to_command_string_with_spaces() {
        let claude = Claude::builder()
            .binary("/usr/local/bin/claude")
            .build()
            .unwrap();

        let cmd = QueryCommand::new("hello world").model("sonnet");
        let command_str = cmd.to_command_string(&claude);

        assert!(command_str.starts_with("/usr/local/bin/claude"));
        assert!(command_str.contains("--print"));
        // Prompt with spaces should be quoted
        assert!(command_str.contains("'hello world'"));
        assert!(command_str.contains("--model"));
        assert!(command_str.contains("sonnet"));
    }

    #[test]
    fn test_to_command_string_with_special_chars() {
        let claude = Claude::builder()
            .binary("/usr/local/bin/claude")
            .build()
            .unwrap();

        let cmd = QueryCommand::new("test $VAR and `cmd`");
        let command_str = cmd.to_command_string(&claude);

        // Arguments with special shell characters should be quoted
        assert!(command_str.contains("'test $VAR and `cmd`'"));
    }

    #[test]
    fn test_to_command_string_with_single_quotes() {
        let claude = Claude::builder()
            .binary("/usr/local/bin/claude")
            .build()
            .unwrap();

        let cmd = QueryCommand::new("it's");
        let command_str = cmd.to_command_string(&claude);

        // Single quotes should be escaped in shell
        assert!(command_str.contains("'it'\\''s'"));
    }

    #[test]
    fn test_worktree_flag() {
        let cmd = QueryCommand::new("test").worktree();
        let args = cmd.args();
        assert!(args.contains(&"--worktree".to_string()));
    }

    #[test]
    fn test_worktree_named() {
        let cmd = QueryCommand::new("test").worktree_named("feature-x");
        let args = cmd.args();
        assert!(
            args.windows(2).any(|w| w == ["--worktree", "feature-x"]),
            "missing --worktree feature-x in {args:?}"
        );
    }

    #[test]
    fn test_brief_flag() {
        let cmd = QueryCommand::new("test").brief();
        let args = cmd.args();
        assert!(args.contains(&"--brief".to_string()));
    }

    #[test]
    fn test_debug_filter() {
        let cmd = QueryCommand::new("test").debug_filter("api,hooks");
        let args = cmd.args();
        assert!(args.contains(&"--debug".to_string()));
        assert!(args.contains(&"api,hooks".to_string()));
    }

    #[test]
    fn test_debug_file() {
        let cmd = QueryCommand::new("test").debug_file("/tmp/debug.log");
        let args = cmd.args();
        assert!(args.contains(&"--debug-file".to_string()));
        assert!(args.contains(&"/tmp/debug.log".to_string()));
    }

    #[test]
    fn test_betas() {
        let cmd = QueryCommand::new("test").betas("feature-x");
        let args = cmd.args();
        assert!(args.contains(&"--betas".to_string()));
        assert!(args.contains(&"feature-x".to_string()));
    }

    #[test]
    fn test_plugin_dir_single() {
        let cmd = QueryCommand::new("test").plugin_dir("/plugins/foo");
        let args = cmd.args();
        assert!(args.contains(&"--plugin-dir".to_string()));
        assert!(args.contains(&"/plugins/foo".to_string()));
    }

    #[test]
    fn test_plugin_dir_multiple() {
        let cmd = QueryCommand::new("test")
            .plugin_dir("/plugins/foo")
            .plugin_dir("/plugins/bar");
        let args = cmd.args();
        let plugin_dir_count = args.iter().filter(|a| *a == "--plugin-dir").count();
        assert_eq!(plugin_dir_count, 2);
        assert!(args.contains(&"/plugins/foo".to_string()));
        assert!(args.contains(&"/plugins/bar".to_string()));
    }

    #[test]
    fn test_plugin_url_single() {
        let cmd = QueryCommand::new("test").plugin_url("https://example.com/p.zip");
        let args = cmd.args();
        assert!(args.contains(&"--plugin-url".to_string()));
        assert!(args.contains(&"https://example.com/p.zip".to_string()));
    }

    #[test]
    fn test_plugin_url_multiple() {
        let cmd = QueryCommand::new("test")
            .plugin_url("https://example.com/a.zip")
            .plugin_url("https://example.com/b.zip");
        let args = cmd.args();
        let plugin_url_count = args.iter().filter(|a| *a == "--plugin-url").count();
        assert_eq!(plugin_url_count, 2);
        assert!(args.contains(&"https://example.com/a.zip".to_string()));
        assert!(args.contains(&"https://example.com/b.zip".to_string()));
    }

    #[test]
    fn test_safe_mode_flag() {
        let cmd = QueryCommand::new("test").safe_mode();
        let args = cmd.args();
        assert!(args.contains(&"--safe-mode".to_string()));
    }

    #[test]
    fn test_safe_mode_absent_by_default() {
        let cmd = QueryCommand::new("test");
        let args = cmd.args();
        assert!(!args.contains(&"--safe-mode".to_string()));
    }

    #[test]
    fn test_setting_sources() {
        let cmd = QueryCommand::new("test").setting_sources("user,project,local");
        let args = cmd.args();
        assert!(args.contains(&"--setting-sources".to_string()));
        assert!(args.contains(&"user,project,local".to_string()));
    }

    #[test]
    fn test_tmux_flag() {
        let cmd = QueryCommand::new("test").tmux();
        let args = cmd.args();
        assert!(args.contains(&"--tmux".to_string()));
    }

    // ─── shell_quote unit tests (#455) ───

    #[test]
    fn shell_quote_plain_word_is_unchanged() {
        assert_eq!(shell_quote("simple"), "simple");
        assert_eq!(shell_quote(""), "");
        assert_eq!(shell_quote("file.rs"), "file.rs");
    }

    #[test]
    fn shell_quote_whitespace_gets_single_quoted() {
        assert_eq!(shell_quote("hello world"), "'hello world'");
        assert_eq!(shell_quote("a\tb"), "'a\tb'");
    }

    #[test]
    fn shell_quote_metacharacters_get_quoted() {
        assert_eq!(shell_quote("a|b"), "'a|b'");
        assert_eq!(shell_quote("$VAR"), "'$VAR'");
        assert_eq!(shell_quote("a;b"), "'a;b'");
        assert_eq!(shell_quote("(x)"), "'(x)'");
    }

    #[test]
    fn shell_quote_embedded_single_quote_is_escaped() {
        assert_eq!(shell_quote("it's"), "'it'\\''s'");
    }

    #[test]
    fn shell_quote_double_quote_gets_single_quoted() {
        assert_eq!(shell_quote(r#"say "hi""#), r#"'say "hi"'"#);
    }
}
