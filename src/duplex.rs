//! Long-lived duplex stream-json sessions.
//!
//! [`DuplexSession`] holds a `claude` subprocess open in
//! `--input-format stream-json --output-format stream-json` mode for
//! the duration of a conversation. A single child is held open across
//! many turns; user messages are written to its stdin, NDJSON events
//! are read from its stdout and dispatched back to `send()` callers.
//!
//! # When to use
//!
//! [`DuplexSession`] is the recommended primitive for long-running
//! hosts that drive multi-turn conversations: agent servers, IDE
//! backends, daemons, chat UIs. Holding the child open across turns
//! amortizes init cost and unlocks capabilities that are awkward or
//! impossible from a transient subprocess: mid-turn permission
//! decisions ([`PermissionHandler`]), clean
//! [interrupts](DuplexSession::interrupt), and a typed
//! [event subscriber stream](DuplexSession::subscribe) that fans out
//! events to multiple consumers.
//!
//! For short-lived processes (CLIs, build scripts, batch jobs,
//! lambdas) where each turn can stand on its own, prefer
//! [`QueryCommand`] for one-off calls or [`Session`] for transient
//! multi-turn with cumulative cost / history tracking.
//!
//! # Cost and budget bookkeeping
//!
//! [`DuplexSession`] itself keeps no cross-turn accounting: each
//! [`TurnResult`] carries that turn's cost and nothing accumulates.
//! For cumulative cost, turn history, and a
//! [`BudgetTracker`]-enforced spend ceiling (send fails fast with
//! [`Error::BudgetExceeded`] once the ceiling is hit), wrap the
//! session in a [`Conversation`] --
//! it is a thin bookkeeping layer over this module, not a different
//! transport.
//!
//! [`QueryCommand`]: crate::QueryCommand
//! [`Session`]: crate::session::Session
//! [`Conversation`]: crate::conversation::Conversation
//! [`BudgetTracker`]: crate::budget::BudgetTracker
//!
//! # Example
//!
//! ```no_run
//! use claude_wrapper::Claude;
//! use claude_wrapper::duplex::{DuplexOptions, DuplexSession};
//!
//! # async fn example() -> claude_wrapper::Result<()> {
//! let claude = Claude::builder().build()?;
//! let session = DuplexSession::spawn(
//!     &claude,
//!     DuplexOptions::default().model("haiku"),
//! ).await?;
//!
//! let turn = session.send("hello").await?;
//! if let Some(text) = turn.result_text() {
//!     println!("{text}");
//! }
//!
//! session.close().await?;
//! # Ok(())
//! # }
//! ```
//!
//! # Subscribers
//!
//! For event-driven UIs that want to react to assistant tokens,
//! tool-use blocks, or system events as they arrive, call
//! [`DuplexSession::subscribe`] before issuing a [`DuplexSession::send`].
//! Each receiver gets its own buffered view of the event stream;
//! slow consumers see [`tokio::sync::broadcast::error::RecvError::Lagged`]
//! rather than blocking the session task.
//!
//! ```no_run
//! use claude_wrapper::Claude;
//! use claude_wrapper::duplex::{DuplexOptions, DuplexSession, InboundEvent};
//!
//! # async fn example() -> claude_wrapper::Result<()> {
//! let claude = Claude::builder().build()?;
//! let session = DuplexSession::spawn(&claude, DuplexOptions::default()).await?;
//!
//! let mut rx = session.subscribe();
//! let _turn = session.send("hello").await?;
//!
//! while let Ok(event) = rx.try_recv() {
//!     match event {
//!         InboundEvent::SystemInit { session_id } => {
//!             println!("session id: {session_id}");
//!         }
//!         InboundEvent::Assistant(_) => {
//!             // partial or complete assistant message
//!         }
//!         _ => {}
//!     }
//! }
//!
//! session.close().await?;
//! # Ok(())
//! # }
//! ```
//!
//! For interleaved (concurrent) event handling while a turn is in
//! flight, drive `rx.recv()` and the `send()` future together via
//! `tokio::select!`. Pin the send future and use a block scope so
//! its borrow of the session ends before [`DuplexSession::close`].
//!
//! # Mid-turn permission decisions
//!
//! Configure a [`PermissionHandler`] at spawn time to answer the
//! CLI's permission prompts in-flight. The session writes
//! `--permission-prompt-tool stdio` automatically when a handler is
//! set, so the CLI emits `control_request` messages for tool use
//! over the duplex channel rather than blocking on a TUI prompt.
//!
//! ```no_run
//! use claude_wrapper::Claude;
//! use claude_wrapper::duplex::{
//!     DuplexOptions, DuplexSession, PermissionDecision, PermissionHandler,
//! };
//!
//! # async fn example() -> claude_wrapper::Result<()> {
//! let handler = PermissionHandler::new(|req| async move {
//!     if req.tool_name == "Bash" {
//!         PermissionDecision::Deny { message: "bash is denied".into() }
//!     } else {
//!         PermissionDecision::Allow { updated_input: None }
//!     }
//! });
//!
//! let claude = Claude::builder().build()?;
//! let session = DuplexSession::spawn(
//!     &claude,
//!     DuplexOptions::default().on_permission(handler),
//! ).await?;
//! # Ok(())
//! # }
//! ```
//!
//! For human-in-the-loop UIs, return [`PermissionDecision::Defer`]
//! from the handler, capture the [`PermissionRequest::request_id`],
//! and answer later via [`DuplexSession::respond_to_permission`].
//!
//! **Known limitation:** as of claude CLI 2.1.x,
//! `--permission-prompt-tool stdio` does not cause the CLI to emit
//! `control_request {subtype: "can_use_tool"}` in
//! `--print --output-format stream-json` mode. The permission handler
//! registered here is wire-correct and unit-tested, but will not be
//! invoked end-to-end until the upstream CLI bug is resolved. Tracked
//! upstream at
//! <https://github.com/anthropics/claude-agent-sdk-python/issues/469>.
//!
//! # Mid-turn interrupt
//!
//! [`DuplexSession::interrupt`] sends a clean
//! `control_request {subtype: "interrupt"}` to the CLI. The CLI
//! stops generating, closes the in-flight turn (`send().await`
//! resolves with the truncated [`TurnResult`]), and answers our
//! interrupt with a `control_response`. Use this instead of dropping
//! the session or killing the child when you want to cancel one
//! turn but keep the conversation going.
//!
//! ```no_run
//! use std::time::Duration;
//! use claude_wrapper::Claude;
//! use claude_wrapper::duplex::{DuplexOptions, DuplexSession};
//!
//! # async fn example() -> claude_wrapper::Result<()> {
//! let claude = Claude::builder().build()?;
//! let session = DuplexSession::spawn(&claude, DuplexOptions::default()).await?;
//!
//! let send_fut = session.send("write a long essay about rust");
//! let interrupt_fut = async {
//!     tokio::time::sleep(Duration::from_millis(500)).await;
//!     session.interrupt().await
//! };
//!
//! let (turn, interrupt_result) = tokio::join!(send_fut, interrupt_fut);
//! let _truncated = turn?;
//! interrupt_result?;
//! # Ok(())
//! # }
//! ```
//!
//! # Phased rollout
//!
//! This module rolled out in four PRs tracked in
//! <https://github.com/joshrotenberg/claude-wrapper/issues/561>:
//! `spawn`/`send`/`close` (PR 1), `subscribe` (PR 2), mid-turn
//! permission handling (PR 3), and `interrupt` (PR 4, this one).

use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;

use serde_json::Value;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, ChildStdout, Command};
use tokio::sync::{broadcast, mpsc, oneshot, watch};
use tokio::task::JoinHandle;
use tracing::{Instrument, debug, warn};

use crate::Claude;
use crate::command::spawn_args::{SharedSpawnArgs, shell_quote};
use crate::error::{Error, Result};
use crate::tool_pattern::ToolPattern;
use crate::types::{Effort, HermeticScope, PermissionMode};

/// Default capacity of the per-session [`broadcast::Sender`] backing
/// [`DuplexSession::subscribe`].
///
/// Override per-session via [`DuplexOptions::subscriber_capacity`].
pub const DEFAULT_SUBSCRIBER_CAPACITY: usize = 256;

/// A mid-turn permission prompt from the CLI for a single tool
/// invocation.
///
/// Forwarded to the [`PermissionHandler`] registered via
/// [`DuplexOptions::on_permission`]. Capture
/// [`Self::request_id`] inside your handler if you intend to return
/// [`PermissionDecision::Defer`] and answer later via
/// [`DuplexSession::respond_to_permission`].
#[derive(Debug, Clone)]
pub struct PermissionRequest {
    /// CLI-assigned correlation id. Pass this to
    /// [`DuplexSession::respond_to_permission`] when deferring.
    pub request_id: String,
    /// The tool the model wants to use (e.g. `"Bash"`, `"Edit"`).
    pub tool_name: String,
    /// The tool's `input` payload as the model produced it.
    pub input: Value,
    /// The full `request` object as sent by the CLI, for fields not
    /// promoted to typed accessors.
    pub raw: Value,
}

/// The decision returned from a [`PermissionHandler`] (or passed to
/// [`DuplexSession::respond_to_permission`] for deferred decisions).
///
/// `Allow` and `Deny` both write a control response to the CLI
/// immediately. `Defer` causes the run loop to skip writing a
/// response; the caller is then expected to invoke
/// [`DuplexSession::respond_to_permission`] later. Passing `Defer`
/// to `respond_to_permission` is a no-op.
#[derive(Debug, Clone)]
pub enum PermissionDecision {
    /// Allow the tool to run, optionally with rewritten input.
    Allow {
        /// Replace the model's input with this object before running
        /// the tool. `None` keeps the original input.
        updated_input: Option<Value>,
    },
    /// Deny the tool. The `message` is surfaced to the model.
    Deny {
        /// Human-readable explanation given back to the model.
        message: String,
    },
    /// Decision pending; the caller will supply it later via
    /// [`DuplexSession::respond_to_permission`].
    Defer,
}

type PermissionFuture = Pin<Box<dyn Future<Output = PermissionDecision> + Send + 'static>>;
type PermissionFn = dyn Fn(PermissionRequest) -> PermissionFuture + Send + Sync + 'static;

/// A user-supplied async callback invoked when the CLI requests
/// permission to use a tool.
///
/// Construct with [`Self::new`], passing an `async fn` or
/// async-block closure. Cheap to clone (`Arc` under the hood).
///
/// The handler runs inline on the duplex session's task. The CLI is
/// blocked on the response while the handler runs, so awaiting an
/// async policy check (DB lookup, remote call) is fine. If the
/// decision needs human input on a different timescale, return
/// [`PermissionDecision::Defer`] and answer via
/// [`DuplexSession::respond_to_permission`] when ready.
#[derive(Clone)]
pub struct PermissionHandler {
    inner: Arc<PermissionFn>,
}

impl PermissionHandler {
    /// Wrap an async closure as a permission handler.
    ///
    /// # Example
    ///
    /// ```
    /// use claude_wrapper::duplex::{PermissionDecision, PermissionHandler};
    ///
    /// let _handler = PermissionHandler::new(|req| async move {
    ///     if req.tool_name == "Bash" {
    ///         PermissionDecision::Deny { message: "no bash".into() }
    ///     } else {
    ///         PermissionDecision::Allow { updated_input: None }
    ///     }
    /// });
    /// ```
    pub fn new<F, Fut>(f: F) -> Self
    where
        F: Fn(PermissionRequest) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = PermissionDecision> + Send + 'static,
    {
        Self {
            inner: Arc::new(move |req| Box::pin(f(req))),
        }
    }

    fn invoke(&self, req: PermissionRequest) -> PermissionFuture {
        (self.inner)(req)
    }
}

impl std::fmt::Debug for PermissionHandler {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PermissionHandler").finish_non_exhaustive()
    }
}

/// Configuration for [`DuplexSession::spawn`].
///
/// Builder methods cover the spawn-time options shared with
/// [`QueryCommand`](crate::QueryCommand); the flag emission lives on a
/// common internal `SharedSpawnArgs`, so the oneshot and duplex paths
/// cannot drift on how a knob is rendered. The spawn call always
/// includes
/// `--print --verbose --input-format stream-json --output-format stream-json`
/// regardless of these options.
///
/// A few `QueryCommand` knobs are intentionally not surfaced here
/// because they only make sense for a oneshot run or are owned by the
/// duplex transport itself:
///
/// - Transport is fixed: `output_format`, `input_format`,
///   `include_partial_messages`, `verbose`, and `prompt_via_stdin` are
///   pinned by the duplex spawn and not configurable.
/// - `retry_policy` reruns a whole oneshot invocation; a duplex session
///   holds one child open across turns, so there is nothing to retry at
///   this layer.
/// - `brief` and `from_pr` shape a single oneshot run (SendUserMessage
///   for one-turn agent-to-user replies, resume-from-PR startup); a
///   duplex host drives turns and session selection itself.
/// - `prompt_suggestions` and `replay_user_messages` shape stdin/stream
///   echoing; the duplex layer owns its own stream plumbing.
///
/// Use [`Self::arg`] if you need one of these on a duplex spawn anyway.
#[derive(Debug, Default, Clone)]
pub struct DuplexOptions {
    // Spawn-time knobs shared with QueryCommand; the flag emission
    // lives on SharedSpawnArgs so the two builders cannot drift.
    shared: SharedSpawnArgs,
    additional_args: Vec<String>,
    subscriber_capacity: Option<usize>,
    on_permission: Option<PermissionHandler>,
}

impl DuplexOptions {
    /// Set the model for this session (`--model`).
    #[must_use]
    pub fn model(mut self, model: impl Into<String>) -> Self {
        self.shared.model = Some(model.into());
        self
    }

    /// Set the system prompt for this session (`--system-prompt`).
    #[must_use]
    pub fn system_prompt(mut self, prompt: impl Into<String>) -> Self {
        self.shared.system_prompt = Some(prompt.into());
        self
    }

    /// Append to the default system prompt (`--append-system-prompt`).
    #[must_use]
    pub fn append_system_prompt(mut self, prompt: impl Into<String>) -> Self {
        self.shared.append_system_prompt = Some(prompt.into());
        self
    }

    /// Resume a prior session by id (`--resume <session_id>`).
    ///
    /// Mirrors [`QueryCommand::resume`](crate::QueryCommand::resume)
    /// for the duplex path. The spawned `claude` process picks up the
    /// conversation that produced `session_id` and continues it; turns
    /// sent through [`DuplexSession::send`] append to the existing
    /// history rather than starting fresh.
    ///
    /// Use case: a host (IDE, MCP server, agent backend) wants to
    /// upgrade a passive on-disk session to a live duplex one --
    /// pulls the `session_id` out of the existing JSONL log, opens a
    /// duplex session here, and the next turn extends the same
    /// conversation.
    ///
    /// `resume` and [`Self::continue_session`] are mutually exclusive
    /// at the CLI; passing both lets the CLI decide (it errors today).
    #[must_use]
    pub fn resume(mut self, session_id: impl Into<String>) -> Self {
        self.shared.resume = Some(session_id.into());
        self
    }

    /// Continue the most recent session in the current working
    /// directory (`--continue`).
    ///
    /// Mirrors [`QueryCommand::continue_session`](crate::QueryCommand::continue_session)
    /// for the duplex path. Use [`Self::resume`] to pick a specific
    /// session id; use this when "the last one" is what you want.
    #[must_use]
    pub fn continue_session(mut self) -> Self {
        self.shared.continue_session = true;
        self
    }

    /// Run this session in a fresh git worktree (`--worktree [name]`).
    ///
    /// `name` is the optional worktree name (the CLI auto-generates
    /// one if omitted). Calling this method always enables the
    /// worktree flag, with or without a name.
    ///
    /// Use case: an agent host wants the chat's writes isolated from
    /// the current working tree -- the chat opens with a fresh
    /// worktree, mutations land there, and the host can inspect or
    /// merge later.
    #[must_use]
    pub fn worktree(mut self, name: Option<impl Into<String>>) -> Self {
        self.shared.worktree = true;
        if let Some(n) = name {
            self.shared.worktree_name = Some(n.into());
        }
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
    pub fn agent(mut self, name: impl Into<String>) -> Self {
        self.shared.agent = Some(name.into());
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

    /// Set the permission mode for this session
    /// (`--permission-mode <mode>`).
    ///
    /// Mirrors [`QueryCommand::permission_mode`](crate::QueryCommand::permission_mode)
    /// for the duplex path. The default mode (when this method isn't
    /// called) drops to the CLI's interactive prompt for every
    /// tool-use approval, which is broken for non-interactive duplex
    /// sessions -- nothing answers the prompts and the session stalls
    /// or fails. Call this with [`PermissionMode::AcceptEdits`] for
    /// the "edit files autonomously" pattern, [`PermissionMode::Plan`]
    /// for read-only planning, etc.
    ///
    /// Bypass mode is a footgun; reach for [`Self::dangerously_skip_permissions`]
    /// (or, for stricter discipline, [`crate::dangerous::DangerousClient`])
    /// when you really need it.
    #[must_use]
    pub fn permission_mode(mut self, mode: PermissionMode) -> Self {
        self.shared.permission_mode = Some(mode);
        self
    }

    /// Pass `--dangerously-skip-permissions` to the spawned session.
    ///
    /// Bypasses ALL permission checks -- file edits, bash, network,
    /// the lot. Use only when you know the session runs in a trusted
    /// sandbox (a fresh worktree, a container, etc.). For most "run
    /// autonomously" cases you want [`Self::permission_mode`] with
    /// [`PermissionMode::AcceptEdits`] instead.
    #[must_use]
    pub fn dangerously_skip_permissions(mut self) -> Self {
        self.shared.dangerously_skip_permissions = true;
        self
    }

    /// Start a new session under a caller-chosen id
    /// (`--session-id <uuid>`).
    ///
    /// Mirrors [`QueryCommand::session_id`](crate::QueryCommand::session_id)
    /// for the duplex path. Unlike [`Self::resume`] (pick up an
    /// existing session) or [`Self::continue_session`] (pick up the
    /// most recent one), this mints a fresh session whose id the host
    /// knows up front -- useful when the host indexes sessions
    /// externally before the first turn completes.
    #[must_use]
    pub fn session_id(mut self, id: impl Into<String>) -> Self {
        self.shared.session_id = Some(id.into());
        self
    }

    /// Set a JSON schema for structured output validation
    /// (`--json-schema <schema>`).
    ///
    /// Mirrors [`QueryCommand::json_schema`](crate::QueryCommand::json_schema)
    /// for the duplex path. `schema` is the inline JSON of the schema;
    /// the turn's closing result message carries the validated
    /// `structured_output`.
    #[must_use]
    pub fn json_schema(mut self, schema: impl Into<String>) -> Self {
        self.shared.json_schema = Some(schema.into());
        self
    }

    /// Add allowed tool patterns (`--allowed-tools`).
    ///
    /// Mirrors [`QueryCommand::allowed_tools`](crate::QueryCommand::allowed_tools)
    /// for the duplex path: accepts anything convertible into
    /// [`ToolPattern`], including bare strings (e.g. `"Bash"`,
    /// `"Bash(git log:*)"`, `"mcp__my-server__*"`), and joins them
    /// into the comma-separated form the CLI expects.
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

    /// Add disallowed tool patterns (`--disallowed-tools`).
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

    /// Cap the number of agentic turns (`--max-turns <n>`).
    ///
    /// The cap applies per turn sent through [`DuplexSession::send`];
    /// a turn that exhausts it closes with an `error_max_turns`
    /// result rather than an assistant reply.
    #[must_use]
    pub fn max_turns(mut self, turns: u32) -> Self {
        self.shared.max_turns = Some(turns);
        self
    }

    /// Cap claude's own spend for the session
    /// (`--max-budget-usd <usd>`).
    ///
    /// This is the CLI's cap, checked post-hoc after each API call,
    /// so a session can overspend before tripping. It is distinct
    /// from the wrapper's [`BudgetTracker`](crate::budget::BudgetTracker)
    /// ceiling, which gates dispatch host-side -- attach one via
    /// [`Conversation::with_budget`](crate::conversation::Conversation::with_budget)
    /// to stop a duplex conversation before the next turn is sent.
    #[must_use]
    pub fn max_budget_usd(mut self, budget: f64) -> Self {
        self.shared.max_budget_usd = Some(budget);
        self
    }

    /// Set a fallback model for when the primary is overloaded
    /// (`--fallback-model <model>`).
    #[must_use]
    pub fn fallback_model(mut self, model: impl Into<String>) -> Self {
        self.shared.fallback_model = Some(model.into());
        self
    }

    /// Set the reasoning effort level (`--effort <level>`).
    #[must_use]
    pub fn effort(mut self, effort: Effort) -> Self {
        self.shared.effort = Some(effort);
        self
    }

    /// Add an additional directory for tool access
    /// (`--add-dir <dir>`, repeatable).
    #[must_use]
    pub fn add_dir(mut self, dir: impl Into<String>) -> Self {
        self.shared.add_dir.push(dir.into());
        self
    }

    /// Add an MCP config file path (`--mcp-config <path>`,
    /// repeatable).
    ///
    /// Pair with [`crate::McpConfigBuilder`] to generate the file.
    #[must_use]
    pub fn mcp_config(mut self, path: impl Into<String>) -> Self {
        self.shared.mcp_config.push(path.into());
        self
    }

    /// Only use MCP servers from `--mcp-config` files, ignoring the
    /// user- and project-level MCP configuration
    /// (`--strict-mcp-config`).
    #[must_use]
    pub fn strict_mcp_config(mut self) -> Self {
        self.shared.strict_mcp_config = true;
        self
    }

    /// Comma-separated list of setting sources the CLI loads, for example
    /// `"user,project,local"` (`--setting-sources`). Pass an empty string to
    /// load none, sealing the session's promptspace against ambient project
    /// config (agents, skills, `CLAUDE.md`). Mirrors
    /// [`QueryCommand::setting_sources`](crate::QueryCommand::setting_sources).
    #[must_use]
    pub fn setting_sources(mut self, sources: impl Into<String>) -> Self {
        self.shared.setting_sources = Some(sources.into());
        self
    }

    /// Seal the ambient `~/.claude` config for a reproducible session
    /// ([`HermeticScope::Full`]).
    ///
    /// Sets `--setting-sources ""`, `--strict-mcp-config`, and
    /// `--exclude-dynamic-system-prompt-sections`. This gives a warm
    /// duplex session a clean seal without the [`Self::arg`] escape
    /// hatch. Mirrors
    /// [`QueryCommand::hermetic`](crate::QueryCommand::hermetic).
    ///
    /// This is not [`Self::bare`]: a hermetic seal leaves OAuth and
    /// keychain auth working, whereas `--bare` forces API-key billing.
    /// A later [`Self::setting_sources`] call overrides the seal scope.
    #[must_use]
    pub fn hermetic(mut self) -> Self {
        self.shared.apply_hermetic(HermeticScope::Full);
        self
    }

    /// Seal the ambient `~/.claude` config at an explicit
    /// [`HermeticScope`].
    ///
    /// See [`Self::hermetic`] for the flag set. Mirrors
    /// [`QueryCommand::hermetic_scoped`](crate::QueryCommand::hermetic_scoped).
    #[must_use]
    pub fn hermetic_scoped(mut self, scope: HermeticScope) -> Self {
        self.shared.apply_hermetic(scope);
        self
    }

    /// Do not persist the session to on-disk history
    /// (`--no-session-persistence`).
    #[must_use]
    pub fn no_session_persistence(mut self) -> Self {
        self.shared.no_session_persistence = true;
        self
    }

    /// Set the list of available built-in tools (`--tools`).
    ///
    /// Use `""` to disable all tools, `"default"` for all tools, or
    /// specific tool names like `["Bash", "Edit", "Read"]`. This is
    /// distinct from [`Self::allowed_tools`], which controls tool
    /// permissions rather than which built-ins load. Mirrors
    /// [`QueryCommand::tools`](crate::QueryCommand::tools).
    #[must_use]
    pub fn tools(mut self, tools: impl IntoIterator<Item = impl Into<String>>) -> Self {
        self.shared.tools.extend(tools.into_iter().map(Into::into));
        self
    }

    /// Add a file resource to download at startup (`--file`).
    ///
    /// Format: `file_id:relative_path` (e.g. `file_abc:doc.txt`).
    /// Repeatable. Mirrors [`QueryCommand::file`](crate::QueryCommand::file).
    #[must_use]
    pub fn file(mut self, spec: impl Into<String>) -> Self {
        self.shared.file.push(spec.into());
        self
    }

    /// Path to a settings JSON file or a JSON string (`--settings`).
    ///
    /// Mirrors [`QueryCommand::settings`](crate::QueryCommand::settings).
    #[must_use]
    pub fn settings(mut self, settings: impl Into<String>) -> Self {
        self.shared.settings = Some(settings.into());
        self
    }

    /// When resuming, create a new session id instead of reusing the
    /// original (`--fork-session`).
    ///
    /// Only meaningful alongside [`Self::resume`] or
    /// [`Self::continue_session`]. Mirrors
    /// [`QueryCommand::fork_session`](crate::QueryCommand::fork_session).
    #[must_use]
    pub fn fork_session(mut self) -> Self {
        self.shared.fork_session = true;
        self
    }

    /// Enable debug logging with an optional filter, e.g. `"api,hooks"`
    /// (`--debug`). Mirrors
    /// [`QueryCommand::debug_filter`](crate::QueryCommand::debug_filter).
    #[must_use]
    pub fn debug_filter(mut self, filter: impl Into<String>) -> Self {
        self.shared.debug_filter = Some(filter.into());
        self
    }

    /// Write debug logs to the given file path (`--debug-file`).
    ///
    /// Mirrors [`QueryCommand::debug_file`](crate::QueryCommand::debug_file).
    #[must_use]
    pub fn debug_file(mut self, path: impl Into<String>) -> Self {
        self.shared.debug_file = Some(path.into());
        self
    }

    /// Beta feature headers for API key authentication (`--betas`).
    ///
    /// Mirrors [`QueryCommand::betas`](crate::QueryCommand::betas).
    #[must_use]
    pub fn betas(mut self, betas: impl Into<String>) -> Self {
        self.shared.betas = Some(betas.into());
        self
    }

    /// Load plugins from the given directory for this session
    /// (`--plugin-dir`). Repeatable. Mirrors
    /// [`QueryCommand::plugin_dir`](crate::QueryCommand::plugin_dir).
    #[must_use]
    pub fn plugin_dir(mut self, dir: impl Into<String>) -> Self {
        self.shared.plugin_dirs.push(dir.into());
        self
    }

    /// Fetch a plugin `.zip` from a URL for this session only
    /// (`--plugin-url`). Repeatable. Mirrors
    /// [`QueryCommand::plugin_url`](crate::QueryCommand::plugin_url).
    #[must_use]
    pub fn plugin_url(mut self, url: impl Into<String>) -> Self {
        self.shared.plugin_urls.push(url.into());
        self
    }

    /// Create a tmux session for the worktree (`--tmux`).
    ///
    /// Mirrors [`QueryCommand::tmux`](crate::QueryCommand::tmux).
    #[must_use]
    pub fn tmux(mut self) -> Self {
        self.shared.tmux = true;
        self
    }

    /// Run in minimal mode (`--bare`).
    ///
    /// Skips hooks, LSP, plugin sync, attribution, auto-memory,
    /// background prefetches, keychain reads, and `CLAUDE.md`
    /// auto-discovery. Anthropic auth is restricted to
    /// `ANTHROPIC_API_KEY` or `apiKeyHelper`; OAuth and keychain are
    /// never read. Mirrors [`QueryCommand::bare`](crate::QueryCommand::bare).
    #[must_use]
    pub fn bare(mut self) -> Self {
        self.shared.bare = true;
        self
    }

    /// Start with all customizations disabled (`--safe-mode`).
    ///
    /// Disables `CLAUDE.md`, skills, plugins, hooks, MCP servers,
    /// custom commands and agents, and output styles for
    /// troubleshooting. Mirrors
    /// [`QueryCommand::safe_mode`](crate::QueryCommand::safe_mode).
    #[must_use]
    pub fn safe_mode(mut self) -> Self {
        self.shared.safe_mode = true;
        self
    }

    /// Disable all slash-command skills (`--disable-slash-commands`).
    ///
    /// Mirrors
    /// [`QueryCommand::disable_slash_commands`](crate::QueryCommand::disable_slash_commands).
    #[must_use]
    pub fn disable_slash_commands(mut self) -> Self {
        self.shared.disable_slash_commands = true;
        self
    }

    /// Include every hook lifecycle event in the stream-json output
    /// (`--include-hook-events`).
    ///
    /// Duplex sessions always run in stream-json, so this takes effect
    /// without extra configuration. Mirrors
    /// [`QueryCommand::include_hook_events`](crate::QueryCommand::include_hook_events).
    #[must_use]
    pub fn include_hook_events(mut self) -> Self {
        self.shared.include_hook_events = true;
        self
    }

    /// Move per-machine sections (cwd, env info, memory paths, git
    /// status) out of the system prompt and into the first user
    /// message (`--exclude-dynamic-system-prompt-sections`).
    ///
    /// Improves cross-user prompt-cache reuse. Only applies with the
    /// default system prompt; ignored with [`Self::system_prompt`].
    /// Mirrors
    /// [`QueryCommand::exclude_dynamic_system_prompt_sections`](crate::QueryCommand::exclude_dynamic_system_prompt_sections).
    #[must_use]
    pub fn exclude_dynamic_system_prompt_sections(mut self) -> Self {
        self.shared.exclude_dynamic_system_prompt_sections = true;
        self
    }

    /// Set a display name for this session (`--name`). Shown in the
    /// `/resume` picker and terminal title. Mirrors
    /// [`QueryCommand::name`](crate::QueryCommand::name).
    #[must_use]
    pub fn name(mut self, name: impl Into<String>) -> Self {
        self.shared.name = Some(name.into());
        self
    }

    /// Add a raw argument to the spawn command line.
    ///
    /// Escape hatch for flags not covered by the dedicated builder
    /// methods.
    #[must_use]
    pub fn arg(mut self, arg: impl Into<String>) -> Self {
        self.additional_args.push(arg.into());
        self
    }

    /// Set the per-session [`broadcast::Sender`] capacity backing
    /// [`DuplexSession::subscribe`].
    ///
    /// Defaults to [`DEFAULT_SUBSCRIBER_CAPACITY`] (256). Larger
    /// values give slow subscribers more room before they
    /// [`Lagged`](tokio::sync::broadcast::error::RecvError::Lagged);
    /// smaller values reclaim memory if you do not subscribe.
    #[must_use]
    pub fn subscriber_capacity(mut self, capacity: usize) -> Self {
        self.subscriber_capacity = Some(capacity);
        self
    }

    /// Register a [`PermissionHandler`] to answer the CLI's tool-use
    /// permission prompts in-flight.
    ///
    /// When set, the spawn command line includes
    /// `--permission-prompt-tool stdio`, which configures the CLI to
    /// emit `control_request` messages for tool use over the duplex
    /// channel rather than blocking on a TUI prompt.
    ///
    /// Without a handler, the session does not pass
    /// `--permission-prompt-tool` and the CLI applies its default
    /// permission policy (driven by `--permission-mode`).
    ///
    /// **Known limitation:** as of claude CLI 2.1.x the CLI does not
    /// emit `control_request {subtype: "can_use_tool"}` in stream-json
    /// print mode, so this handler will not be invoked end-to-end until
    /// an upstream fix lands. The wire handling is correct; see
    /// <https://github.com/anthropics/claude-agent-sdk-python/issues/469>.
    #[must_use]
    pub fn on_permission(mut self, handler: PermissionHandler) -> Self {
        self.on_permission = Some(handler);
        self
    }

    fn build_args(&self) -> Vec<String> {
        let mut args = vec![
            "--print".to_string(),
            "--verbose".to_string(),
            "--output-format".to_string(),
            "stream-json".to_string(),
            "--input-format".to_string(),
            "stream-json".to_string(),
        ];

        self.shared.append_to(&mut args);

        if self.on_permission.is_some() {
            args.push("--permission-prompt-tool".to_string());
            args.push("stdio".to_string());
        }
        args.extend(self.additional_args.iter().cloned());

        args
    }

    /// Assemble the exact argv [`DuplexSession::spawn`] passes to the
    /// CLI binary: the client's global args followed by this option
    /// set's flags. Single assembly path shared by the spawn and
    /// [`Self::to_command_string`], so the preview cannot drift from
    /// what actually runs.
    fn spawn_command_args(&self, claude: &Claude) -> Vec<String> {
        let mut args = claude.global_args.clone();
        args.extend(self.build_args());
        args
    }

    /// Return the full spawn command as a string that could be run in
    /// a shell.
    ///
    /// The duplex analog of
    /// [`QueryCommand::to_command_string`](crate::QueryCommand::to_command_string):
    /// the binary path from the [`Claude`] client plus the exact
    /// arguments [`DuplexSession::spawn`] would pass for these
    /// options, including the client's global args. Both share one
    /// args-assembly path, so this preview always matches the real
    /// spawn. Arguments containing spaces or special shell characters
    /// are shell-quoted.
    ///
    /// # Example
    ///
    /// ```no_run
    /// use claude_wrapper::{Claude, DuplexOptions};
    ///
    /// # fn example() -> claude_wrapper::Result<()> {
    /// let claude = Claude::builder().build()?;
    ///
    /// let opts = DuplexOptions::default()
    ///     .agent("reviewer")
    ///     .setting_sources("project");
    ///
    /// println!("Would spawn: {}", opts.to_command_string(&claude));
    /// # Ok(())
    /// # }
    /// ```
    #[must_use]
    pub fn to_command_string(&self, claude: &Claude) -> String {
        let args = self.spawn_command_args(claude);
        let quoted_args = args.iter().map(|arg| shell_quote(arg)).collect::<Vec<_>>();
        format!("{} {}", claude.binary().display(), quoted_args.join(" "))
    }
}

/// The result of one turn through a [`DuplexSession`].
///
/// `result` is the raw JSON of the `{"type": "result", ...}` message
/// that closed the turn. `events` carries every other message
/// received during the turn (system, assistant, stream_event, user)
/// in arrival order, with the closing `result` excluded.
#[derive(Debug, Clone)]
pub struct TurnResult {
    /// The raw `{"type": "result", ...}` message that ended the turn.
    pub result: Value,
    /// Every other message received during the turn, in order.
    pub events: Vec<Value>,
}

impl TurnResult {
    /// Extract `result.result` as a string, if present.
    #[must_use]
    pub fn result_text(&self) -> Option<&str> {
        self.result.get("result").and_then(Value::as_str)
    }

    /// Extract `result.session_id`, if present.
    #[must_use]
    pub fn session_id(&self) -> Option<&str> {
        self.result.get("session_id").and_then(Value::as_str)
    }

    /// Extract `total_cost_usd` (preferred) or the legacy `cost_usd`
    /// field, if either is present.
    ///
    /// This is the cost of one turn. For the conversation-wide
    /// running total, record turns through a
    /// [`Conversation`](crate::conversation::Conversation).
    #[must_use]
    pub fn total_cost_usd(&self) -> Option<f64> {
        self.result
            .get("total_cost_usd")
            .or_else(|| self.result.get("cost_usd"))
            .and_then(Value::as_f64)
    }

    /// Extract `duration_ms`, if present.
    #[must_use]
    pub fn duration_ms(&self) -> Option<u64> {
        self.result.get("duration_ms").and_then(Value::as_u64)
    }
}

/// A classified inbound event broadcast to [`DuplexSession::subscribe`]
/// receivers.
///
/// Every non-`result` message coming back from the CLI is broadcast as
/// one of these variants. The closing `{"type": "result"}` message is
/// not broadcast; it resolves the in-flight [`DuplexSession::send`]
/// future and lands in [`TurnResult::result`].
///
/// Subscribers see the same set of events that accumulate in
/// [`TurnResult::events`], in the same order, just classified. Adding
/// a typed accessor for a new event type later (e.g. promoting a
/// `system` subtype into its own variant) is non-breaking against the
/// `Other` fallback.
#[derive(Debug, Clone)]
pub enum InboundEvent {
    /// First `{"type": "system", "subtype": "init"}` event for the
    /// session. Carries the CLI-assigned `session_id`.
    SystemInit {
        /// The CLI-assigned session id, useful for logging or
        /// future resume support.
        session_id: String,
    },
    /// `{"type": "assistant", ...}` -- either a complete assistant
    /// message or, in stream-json mode, a partial chunk.
    Assistant(Value),
    /// `{"type": "stream_event", ...}` -- low-level streaming event
    /// emitted while a turn is in progress.
    StreamEvent(Value),
    /// `{"type": "user", ...}` -- typically a tool result echo from
    /// the CLI side.
    User(Value),
    /// Any other event type, including non-`init` `system` events
    /// and any message types not yet recognised by this enum.
    Other(Value),
}

fn classify(msg: &Value) -> InboundEvent {
    match msg.get("type").and_then(Value::as_str) {
        Some("system") => {
            if msg.get("subtype").and_then(Value::as_str) == Some("init")
                && let Some(id) = msg.get("session_id").and_then(Value::as_str)
            {
                return InboundEvent::SystemInit {
                    session_id: id.to_string(),
                };
            }
            InboundEvent::Other(msg.clone())
        }
        Some("assistant") => InboundEvent::Assistant(msg.clone()),
        Some("stream_event") => InboundEvent::StreamEvent(msg.clone()),
        Some("user") => InboundEvent::User(msg.clone()),
        _ => InboundEvent::Other(msg.clone()),
    }
}

/// Liveness state of a [`DuplexSession`]'s background task.
///
/// Surfaced through [`DuplexSession::is_alive`],
/// [`DuplexSession::exit_status`], and
/// [`DuplexSession::wait_for_exit`] for service-shaped hosts that
/// want non-consuming visibility into whether a session is still
/// usable. The closing [`DuplexSession::close`] still returns the
/// full [`Result`] for the one caller that consumes the session.
///
/// `Failed` carries a `String` rather than the full
/// [`Error`] because the underlying watch channel requires `Clone`
/// and `Error` is not `Clone` (its `Io` variant wraps a non-`Clone`
/// `std::io::Error`). The full error remains available via
/// [`DuplexSession::close`].
#[derive(Debug, Clone)]
pub enum SessionExitStatus {
    /// The session task is still running.
    Running,
    /// The session task completed normally (close, stdout EOF without
    /// error).
    Completed,
    /// The session task ended with an error. Carries the error's
    /// `Display` rendering.
    Failed(String),
}

/// A long-lived `claude` subprocess in stream-json duplex mode.
///
/// Owns a background task that holds the child open, writes user
/// messages to its stdin, and reads NDJSON events from its stdout.
/// One turn at a time: calling [`Self::send`] while another turn is
/// in flight returns [`Error::DuplexTurnInFlight`].
///
/// See the [module docs](crate::duplex) for the full design.
#[derive(Debug)]
pub struct DuplexSession {
    outbound_tx: mpsc::UnboundedSender<OutboundMsg>,
    events_tx: broadcast::Sender<InboundEvent>,
    exit_rx: watch::Receiver<SessionExitStatus>,
    join: JoinHandle<Result<()>>,
}

#[derive(Debug)]
enum OutboundMsg {
    Send {
        prompt: String,
        reply: oneshot::Sender<Result<TurnResult>>,
    },
    PermissionResponse {
        request_id: String,
        decision: PermissionDecision,
    },
    Interrupt {
        reply: oneshot::Sender<Result<()>>,
    },
}

impl DuplexSession {
    /// Spawn a fresh `claude` subprocess in duplex mode.
    ///
    /// The child is started with
    /// `--print --verbose --input-format stream-json --output-format stream-json`
    /// plus any options applied via `opts`. The session task takes
    /// ownership of the child; dropping the returned handle (or
    /// calling [`Self::close`]) shuts the task down.
    pub async fn spawn(claude: &Claude, opts: DuplexOptions) -> Result<Self> {
        let capacity = opts
            .subscriber_capacity
            .unwrap_or(DEFAULT_SUBSCRIBER_CAPACITY);
        let permission_handler = opts.on_permission.clone();

        let command_args = opts.spawn_command_args(claude);

        debug!(
            binary = %claude.binary.display(),
            args = ?command_args,
            "spawning duplex claude session"
        );

        let mut cmd = Command::new(&claude.binary);
        cmd.args(&command_args)
            .env_remove("CLAUDECODE")
            .env_remove("CLAUDE_CODE_ENTRYPOINT")
            .envs(&claude.env)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        // Own process group (Unix) so shutdown can signal the whole
        // tree, not just the direct child (see exec::GroupKillGuard).
        // Opt out via ClaudeBuilder::process_group.
        crate::exec::apply_process_group(&mut cmd, claude.process_group);

        if let Some(ref dir) = claude.working_dir {
            cmd.current_dir(dir);
        }

        let mut child = cmd.spawn().map_err(|e| Error::Io {
            message: format!("failed to spawn claude: {e}"),
            source: e,
            working_dir: claude.working_dir.clone(),
        })?;
        let group =
            crate::exec::arm_and_notify(claude.process_group, child.id(), claude.on_spawn.as_ref());

        let stdin = child.stdin.take().expect("stdin was piped");
        let stdout = child.stdout.take().expect("stdout was piped");

        let (outbound_tx, outbound_rx) = mpsc::unbounded_channel();
        let (events_tx, _initial_rx) = broadcast::channel(capacity);
        let (exit_tx, exit_rx) = watch::channel(SessionExitStatus::Running);

        // A session-lifetime span, entered inside the spawned task. The
        // run loop interleaves events from every turn on one stdout
        // stream, so without this its lines ("dropping orphan result
        // event", parse failures, shutdown-budget kills) cannot be
        // attributed to a session at all when several are open.
        //
        // `session_id` starts empty and is recorded when the CLI's init
        // event arrives, since it is not known at spawn unless resuming.
        let session_span = tracing::debug_span!(
            "claude.session",
            session_id = tracing::field::Empty,
            model = opts.shared.model.as_deref().unwrap_or("default"),
            permission_mode = opts
                .shared
                .permission_mode
                .as_ref()
                .map(|m| m.as_arg())
                .unwrap_or("default"),
            resumed = opts.shared.resume.is_some(),
            turns = tracing::field::Empty,
            exit = tracing::field::Empty,
        );
        let join = tokio::spawn(
            run_session(
                child,
                group,
                claude.kill_grace,
                stdin,
                stdout,
                outbound_rx,
                events_tx.clone(),
                permission_handler,
                exit_tx,
            )
            .instrument(session_span),
        );

        Ok(Self {
            outbound_tx,
            events_tx,
            exit_rx,
            join,
        })
    }

    /// Send one user message and await the closing result event.
    ///
    /// Returns [`Error::DuplexTurnInFlight`] if another turn is
    /// already pending, and [`Error::DuplexClosed`] if the session
    /// task has already exited.
    ///
    /// The returned [`TurnResult`] is per-turn; nothing accumulates
    /// across calls. For cumulative cost/history and a budget
    /// ceiling, send through a
    /// [`Conversation`](crate::conversation::Conversation) instead.
    pub async fn send(&self, prompt: impl Into<String>) -> Result<TurnResult> {
        let (reply_tx, reply_rx) = oneshot::channel();
        self.outbound_tx
            .send(OutboundMsg::Send {
                prompt: prompt.into(),
                reply: reply_tx,
            })
            .map_err(|_| Error::DuplexClosed)?;
        reply_rx.await.map_err(|_| Error::DuplexClosed)?
    }

    /// Subscribe to the session's classified inbound event stream.
    ///
    /// Returns a [`broadcast::Receiver<InboundEvent>`] that receives
    /// every non-`result` event as it arrives. Each subscriber gets
    /// its own buffered view; subscribers added later miss earlier
    /// events. Slow subscribers see
    /// [`RecvError::Lagged`](tokio::sync::broadcast::error::RecvError::Lagged)
    /// rather than blocking the session task.
    ///
    /// Subscribers see the same events that accumulate in
    /// [`TurnResult::events`], in the same order.
    ///
    /// # Example
    ///
    /// ```no_run
    /// use claude_wrapper::Claude;
    /// use claude_wrapper::duplex::{DuplexOptions, DuplexSession, InboundEvent};
    ///
    /// # async fn example() -> claude_wrapper::Result<()> {
    /// let claude = Claude::builder().build()?;
    /// let session = DuplexSession::spawn(&claude, DuplexOptions::default()).await?;
    /// let mut rx = session.subscribe();
    ///
    /// // Subscribe before send so we receive every event.
    /// let _turn = session.send("hello").await?;
    ///
    /// while let Ok(event) = rx.try_recv() {
    ///     if let InboundEvent::SystemInit { session_id } = event {
    ///         println!("session id: {session_id}");
    ///     }
    /// }
    /// # Ok(())
    /// # }
    /// ```
    #[must_use]
    pub fn subscribe(&self) -> broadcast::Receiver<InboundEvent> {
        self.events_tx.subscribe()
    }

    /// Cheap, non-blocking liveness check.
    ///
    /// Returns `true` while the session task is running, `false` once
    /// it has exited (whether normally or with an error). Multiple
    /// concurrent callers are allowed, and the call does not consume
    /// the session: [`Self::close`] still works after polling.
    ///
    /// Reads the latest value from a `tokio::sync::watch` channel
    /// updated from inside the session task, so it never blocks and
    /// reflects state set just before the task returns.
    #[must_use]
    pub fn is_alive(&self) -> bool {
        matches!(*self.exit_rx.borrow(), SessionExitStatus::Running)
    }

    /// Snapshot the session task's [`SessionExitStatus`].
    ///
    /// Returns [`SessionExitStatus::Running`] while the task is still
    /// alive, [`SessionExitStatus::Completed`] after a clean exit, or
    /// [`SessionExitStatus::Failed`] with the underlying error
    /// rendered to a string.
    ///
    /// Like [`Self::is_alive`], this is a cheap non-blocking read.
    #[must_use]
    pub fn exit_status(&self) -> SessionExitStatus {
        self.exit_rx.borrow().clone()
    }

    /// Block until the session task transitions out of
    /// [`SessionExitStatus::Running`] and return the terminal status.
    ///
    /// Returns immediately if the task has already exited. Multiple
    /// concurrent callers are supported (each gets its own receiver
    /// clone), and the call does not consume the session.
    ///
    /// If the underlying watch sender is dropped without ever
    /// publishing a terminal state -- which should not happen in
    /// practice, but is treated defensively -- this returns the last
    /// observed value.
    pub async fn wait_for_exit(&self) -> SessionExitStatus {
        let mut rx = self.exit_rx.clone();
        loop {
            {
                let value = rx.borrow_and_update();
                if !matches!(*value, SessionExitStatus::Running) {
                    return value.clone();
                }
            }
            if rx.changed().await.is_err() {
                return rx.borrow().clone();
            }
        }
    }

    /// Answer a deferred permission request from a different task.
    ///
    /// Use this after the [`PermissionHandler`] returned
    /// [`PermissionDecision::Defer`] for the matching `request_id`.
    /// Passing `decision = PermissionDecision::Defer` here is a
    /// no-op (logged at `warn`); pass `Allow` or `Deny`.
    ///
    /// Returns [`Error::DuplexClosed`] if the session task has
    /// already exited.
    ///
    /// # Example
    ///
    /// ```no_run
    /// use claude_wrapper::Claude;
    /// use claude_wrapper::duplex::{
    ///     DuplexOptions, DuplexSession, PermissionDecision, PermissionHandler,
    /// };
    /// use tokio::sync::mpsc;
    ///
    /// # async fn example() -> claude_wrapper::Result<()> {
    /// // Forward request_ids out to a UI thread; answer asynchronously.
    /// let (tx, _rx) = mpsc::unbounded_channel::<String>();
    /// let handler = PermissionHandler::new(move |req| {
    ///     let tx = tx.clone();
    ///     async move {
    ///         let _ = tx.send(req.request_id);
    ///         PermissionDecision::Defer
    ///     }
    /// });
    ///
    /// let claude = Claude::builder().build()?;
    /// let session = DuplexSession::spawn(
    ///     &claude,
    ///     DuplexOptions::default().on_permission(handler),
    /// ).await?;
    ///
    /// // ...later, from the UI thread:
    /// session.respond_to_permission(
    ///     "req-abc",
    ///     PermissionDecision::Allow { updated_input: None },
    /// )?;
    /// # Ok(())
    /// # }
    /// ```
    pub fn respond_to_permission(
        &self,
        request_id: impl Into<String>,
        decision: PermissionDecision,
    ) -> Result<()> {
        if matches!(decision, PermissionDecision::Defer) {
            warn!("respond_to_permission called with Defer; ignoring");
            return Ok(());
        }
        self.outbound_tx
            .send(OutboundMsg::PermissionResponse {
                request_id: request_id.into(),
                decision,
            })
            .map_err(|_| Error::DuplexClosed)?;
        Ok(())
    }

    /// Send a clean interrupt to the CLI and wait for its
    /// acknowledgment.
    ///
    /// Writes a `control_request {subtype: "interrupt"}` and resolves
    /// when the matching `control_response` comes back. The
    /// in-flight turn (if any) closes shortly after with a truncated
    /// [`TurnResult`] -- the [`DuplexSession::send`] future for it
    /// resolves independently. Either ordering is possible; await
    /// both via `tokio::join!` if you care about both outcomes.
    ///
    /// Returns:
    /// - `Ok(())` when the CLI acknowledges with `subtype: "success"`.
    /// - [`Error::DuplexControlFailed`] when the CLI answers with an
    ///   error payload.
    /// - [`Error::DuplexClosed`] if the session task exited before
    ///   the response arrived.
    ///
    /// # Example
    ///
    /// ```no_run
    /// use std::time::Duration;
    /// use claude_wrapper::Claude;
    /// use claude_wrapper::duplex::{DuplexOptions, DuplexSession};
    ///
    /// # async fn example() -> claude_wrapper::Result<()> {
    /// let claude = Claude::builder().build()?;
    /// let session = DuplexSession::spawn(&claude, DuplexOptions::default()).await?;
    ///
    /// let send_fut = session.send("a question that triggers tool use");
    /// let interrupt_fut = async {
    ///     tokio::time::sleep(Duration::from_millis(250)).await;
    ///     session.interrupt().await
    /// };
    ///
    /// let (turn, interrupt) = tokio::join!(send_fut, interrupt_fut);
    /// let _truncated = turn?;
    /// interrupt?;
    /// # Ok(())
    /// # }
    /// ```
    pub async fn interrupt(&self) -> Result<()> {
        let (reply_tx, reply_rx) = oneshot::channel();
        self.outbound_tx
            .send(OutboundMsg::Interrupt { reply: reply_tx })
            .map_err(|_| Error::DuplexClosed)?;
        reply_rx.await.map_err(|_| Error::DuplexClosed)?
    }

    /// Close the session and wait for the underlying task to exit.
    ///
    /// Drops the outbound channel sender, which the session task
    /// observes as `recv() -> None`, then closes stdin and reaps the
    /// child.
    pub async fn close(self) -> Result<()> {
        drop(self.outbound_tx);
        drop(self.events_tx);
        match self.join.await {
            Ok(result) => result,
            Err(e) if e.is_cancelled() => Ok(()),
            Err(e) => Err(Error::Io {
                message: format!("duplex session task panicked: {e}"),
                source: std::io::Error::other(e.to_string()),
                working_dir: None,
            }),
        }
    }
}

/// Time budget for the graceful child shutdown after the run loop
/// exits. If the child is still alive after this deadline we SIGKILL
/// it so close() does not hang on a misbehaving subprocess.
const SHUTDOWN_BUDGET: Duration = Duration::from_secs(5);

#[allow(clippy::too_many_arguments)]
async fn run_session(
    mut child: Child,
    mut group: crate::exec::GroupKillGuard,
    kill_grace: Option<Duration>,
    mut stdin: ChildStdin,
    stdout: ChildStdout,
    mut outbound_rx: mpsc::UnboundedReceiver<OutboundMsg>,
    events_tx: broadcast::Sender<InboundEvent>,
    permission_handler: Option<PermissionHandler>,
    exit_tx: watch::Sender<SessionExitStatus>,
) -> Result<()> {
    let mut lines = BufReader::new(stdout).lines();
    let mut pending: Option<(oneshot::Sender<Result<TurnResult>>, Vec<Value>)> = None;
    let mut pending_control: HashMap<String, oneshot::Sender<Result<()>>> = HashMap::new();
    let mut next_control_id: u64 = 0;
    let mut stream_err: Option<Error> = None;
    // The session span is this task's own span (see DuplexSession::spawn).
    let session_span = tracing::Span::current();
    let mut turns: u64 = 0;
    let mut turn_span: Option<tracing::Span> = None;
    let mut turn_started: Option<std::time::Instant> = None;

    loop {
        tokio::select! {
            biased;

            line = lines.next_line() => match line {
                Ok(Some(l)) => {
                    if l.trim().is_empty() {
                        continue;
                    }
                    let parsed = match serde_json::from_str::<Value>(&l) {
                        Ok(v) => v,
                        Err(e) => {
                            debug!(line = %l, error = %e, "failed to parse duplex event, skipping");
                            continue;
                        }
                    };
                    // Peek before handle_inbound consumes it: the init
                    // event is where the session id first appears, and the
                    // result event carries the turn's outcome.
                    match parsed.get("type").and_then(Value::as_str) {
                        Some("system")
                            if parsed.get("subtype").and_then(Value::as_str) == Some("init") =>
                        {
                            if let Some(id) = parsed.get("session_id").and_then(Value::as_str) {
                                session_span.record("session_id", id);
                            }
                        }
                        Some("result") => {
                            if let Some(span) = turn_span.take() {
                                span.record(
                                    "is_error",
                                    parsed.get("is_error").and_then(Value::as_bool).unwrap_or(false),
                                );
                                if let Some(sub) = parsed.get("subtype").and_then(Value::as_str) {
                                    span.record("subtype", sub);
                                }
                                if let Some(c) =
                                    parsed.get("total_cost_usd").and_then(Value::as_f64)
                                {
                                    span.record("cost_usd", c);
                                }
                                if let Some(started) = turn_started.take() {
                                    span.record(
                                        "duration_ms",
                                        started.elapsed().as_millis() as u64,
                                    );
                                }
                            }
                        }
                        _ => {}
                    }
                    match handle_inbound(parsed, &mut pending, &events_tx) {
                        InboundAction::None => {}
                        InboundAction::Permission(req) => {
                            let request_id = req.request_id.clone();
                            let decision = match permission_handler.as_ref() {
                                Some(h) => h.invoke(req).await,
                                None => {
                                    warn!(
                                        request_id = %request_id,
                                        "received can_use_tool with no permission handler; auto-denying"
                                    );
                                    PermissionDecision::Deny {
                                        message:
                                            "no permission handler configured on duplex session"
                                                .into(),
                                    }
                                }
                            };
                            if matches!(decision, PermissionDecision::Defer) {
                                debug!(
                                    request_id = %request_id,
                                    "permission handler deferred; waiting for respond_to_permission"
                                );
                            } else if let Err(e) =
                                write_permission_response(&mut stdin, &request_id, &decision).await
                            {
                                warn!(error = %e, "failed to write permission response");
                            }
                        }
                        InboundAction::ControlResponse { request_id, outcome } => {
                            if let Some(reply) = pending_control.remove(&request_id) {
                                let _ = reply.send(outcome);
                            } else {
                                debug!(
                                    request_id = %request_id,
                                    "received control_response with no pending request"
                                );
                            }
                        }
                    }
                }
                Ok(None) => break,
                Err(e) => {
                    stream_err = Some(Error::Io {
                        message: "failed to read duplex stdout".to_string(),
                        source: e,
                        working_dir: None,
                    });
                    break;
                }
            },

            msg = outbound_rx.recv() => match msg {
                Some(OutboundMsg::Send { prompt, reply }) => {
                    if pending.is_some() {
                        let _ = reply.send(Err(Error::DuplexTurnInFlight));
                        continue;
                    }
                    if let Err(e) = write_user(&mut stdin, &prompt).await {
                        let _ = reply.send(Err(e));
                        continue;
                    }
                    turns += 1;
                    // One span per turn, so the events a turn produces are
                    // attributable to it rather than to the session as a
                    // whole. Outcome fields are recorded when the result
                    // event arrives above.
                    turn_span = Some(tracing::debug_span!(
                        parent: &session_span,
                        "claude.turn",
                        turn = turns,
                        is_error = tracing::field::Empty,
                        subtype = tracing::field::Empty,
                        cost_usd = tracing::field::Empty,
                        duration_ms = tracing::field::Empty,
                    ));
                    turn_started = Some(std::time::Instant::now());
                    pending = Some((reply, Vec::new()));
                }
                Some(OutboundMsg::PermissionResponse { request_id, decision }) => {
                    if let Err(e) =
                        write_permission_response(&mut stdin, &request_id, &decision).await
                    {
                        warn!(error = %e, "failed to write deferred permission response");
                    }
                }
                Some(OutboundMsg::Interrupt { reply }) => {
                    next_control_id += 1;
                    let request_id = format!("interrupt-{next_control_id}");
                    if let Err(e) =
                        write_control_request(&mut stdin, &request_id, "interrupt").await
                    {
                        let _ = reply.send(Err(e));
                        continue;
                    }
                    pending_control.insert(request_id, reply);
                }
                None => break,
            },
        }
    }

    drop(stdin);
    match tokio::time::timeout(SHUTDOWN_BUDGET, child.wait()).await {
        Ok(Ok(_status)) => {
            group.disarm();
        }
        Ok(Err(e)) => {
            warn!(error = %e, "failed to wait for duplex child");
        }
        Err(_) => {
            warn!("duplex child did not exit within shutdown budget; killing");
            // Take down the whole group (Unix), honoring the optional
            // SIGTERM grace, so subprocesses spawned for tool use die
            // too, then kill+reap the direct child.
            crate::exec::kill_group_with_grace(&mut group, kill_grace).await;
            let _ = child.kill().await;
        }
    }

    if let Some((reply, _)) = pending.take() {
        let _ = reply.send(Err(Error::DuplexClosed));
    }
    for (_, reply) in pending_control.drain() {
        let _ = reply.send(Err(Error::DuplexClosed));
    }

    let result = match stream_err {
        Some(e) => Err(e),
        None => Ok(()),
    };
    let final_state = match &result {
        Ok(()) => SessionExitStatus::Completed,
        Err(e) => SessionExitStatus::Failed(e.to_string()),
    };
    session_span.record("turns", turns);
    session_span.record(
        "exit",
        match &final_state {
            SessionExitStatus::Completed => "completed",
            SessionExitStatus::Failed(_) => "failed",
            SessionExitStatus::Running => "running",
        },
    );
    let _ = exit_tx.send(final_state);
    result
}

/// Action returned from [`handle_inbound`] for the run loop to act
/// on after the side-effects (broadcast, accumulate, resolve) are
/// done.
enum InboundAction {
    /// No further action -- side-effects were all handled inline.
    None,
    /// A `control_request {subtype: "can_use_tool"}` was received and
    /// needs the [`PermissionHandler`] invoked. The run loop awaits
    /// the handler and writes the response.
    Permission(PermissionRequest),
    /// A `control_response` matching one of our outbound
    /// `control_request`s arrived. The run loop matches `request_id`
    /// against its `pending_control` table and resolves the
    /// corresponding oneshot.
    ControlResponse {
        request_id: String,
        outcome: Result<()>,
    },
}

fn handle_inbound(
    msg: Value,
    pending: &mut Option<(oneshot::Sender<Result<TurnResult>>, Vec<Value>)>,
    events_tx: &broadcast::Sender<InboundEvent>,
) -> InboundAction {
    match msg.get("type").and_then(Value::as_str) {
        Some("result") => {
            if let Some((reply, events)) = pending.take() {
                let _ = reply.send(Ok(TurnResult {
                    result: msg,
                    events,
                }));
            } else {
                debug!("dropping orphan result event with no pending turn");
            }
            InboundAction::None
        }
        Some("control_request") => {
            // can_use_tool flows through the permission handler;
            // anything else is logged + accumulated as Other for now.
            if msg
                .get("request")
                .and_then(|r| r.get("subtype"))
                .and_then(Value::as_str)
                == Some("can_use_tool")
                && let Some(req) = parse_permission_request(&msg)
            {
                if let Some((_, events)) = pending.as_mut() {
                    events.push(msg);
                }
                return InboundAction::Permission(req);
            }
            debug!(
                ?msg,
                "received unhandled control_request; treating as Other"
            );
            let _ = events_tx.send(InboundEvent::Other(msg.clone()));
            if let Some((_, events)) = pending.as_mut() {
                events.push(msg);
            }
            InboundAction::None
        }
        Some("control_response") => {
            if let Some((request_id, outcome)) = parse_control_response(&msg) {
                return InboundAction::ControlResponse {
                    request_id,
                    outcome,
                };
            }
            debug!(
                ?msg,
                "received malformed control_response; treating as Other"
            );
            let _ = events_tx.send(InboundEvent::Other(msg.clone()));
            if let Some((_, events)) = pending.as_mut() {
                events.push(msg);
            }
            InboundAction::None
        }
        _ => {
            // Broadcast a classified copy. Send error means no
            // subscribers, which is fine -- subscribers are optional.
            let _ = events_tx.send(classify(&msg));

            if let Some((_, events)) = pending.as_mut() {
                events.push(msg);
            } else {
                debug!("dropping inbound event with no pending turn");
            }
            InboundAction::None
        }
    }
}

fn parse_permission_request(msg: &Value) -> Option<PermissionRequest> {
    let request_id = msg.get("request_id").and_then(Value::as_str)?;
    let request = msg.get("request")?;
    let tool_name = request.get("tool_name").and_then(Value::as_str)?;
    let input = request.get("input").cloned().unwrap_or(Value::Null);
    Some(PermissionRequest {
        request_id: request_id.to_string(),
        tool_name: tool_name.to_string(),
        input,
        raw: request.clone(),
    })
}

/// Pull `(request_id, outcome)` out of a `control_response` envelope.
///
/// Returns `None` if `request_id` is missing or the subtype is
/// unrecognised. `Some((id, Ok(())))` for `subtype: "success"`,
/// `Some((id, Err(DuplexControlFailed)))` for `subtype: "error"`.
fn parse_control_response(msg: &Value) -> Option<(String, Result<()>)> {
    let response = msg.get("response")?;
    let request_id = response.get("request_id").and_then(Value::as_str)?;
    let outcome = match response.get("subtype").and_then(Value::as_str) {
        Some("success") => Ok(()),
        Some("error") => {
            let message = response
                .get("error")
                .and_then(Value::as_str)
                .unwrap_or("unknown control_response error")
                .to_string();
            Err(Error::DuplexControlFailed { message })
        }
        _ => return None,
    };
    Some((request_id.to_string(), outcome))
}

async fn write_user(stdin: &mut ChildStdin, prompt: &str) -> Result<()> {
    let user_msg = serde_json::json!({
        "type": "user",
        "message": {
            "role": "user",
            "content": prompt,
        },
        "parent_tool_use_id": null,
    });
    write_line(stdin, &user_msg, "user message").await
}

async fn write_control_request(
    stdin: &mut ChildStdin,
    request_id: &str,
    subtype: &str,
) -> Result<()> {
    let envelope = serde_json::json!({
        "type": "control_request",
        "request_id": request_id,
        "request": { "subtype": subtype },
    });
    write_line(stdin, &envelope, "control_request").await
}

async fn write_permission_response(
    stdin: &mut ChildStdin,
    request_id: &str,
    decision: &PermissionDecision,
) -> Result<()> {
    let inner = match decision {
        PermissionDecision::Allow { updated_input } => {
            let mut obj = serde_json::Map::new();
            obj.insert("behavior".to_string(), Value::String("allow".to_string()));
            if let Some(input) = updated_input {
                obj.insert("updatedInput".to_string(), input.clone());
            }
            Value::Object(obj)
        }
        PermissionDecision::Deny { message } => serde_json::json!({
            "behavior": "deny",
            "message": message,
        }),
        PermissionDecision::Defer => {
            // Caller path is supposed to filter this; defensive guard.
            return Ok(());
        }
    };
    let envelope = serde_json::json!({
        "type": "control_response",
        "response": {
            "request_id": request_id,
            "subtype": "success",
            "response": inner,
        },
    });
    write_line(stdin, &envelope, "control_response").await
}

async fn write_line(stdin: &mut ChildStdin, value: &Value, what: &'static str) -> Result<()> {
    let mut line = serde_json::to_string(value).map_err(|e| Error::Json {
        message: format!("failed to serialize duplex {what}"),
        source: e,
    })?;
    line.push('\n');
    stdin
        .write_all(line.as_bytes())
        .await
        .map_err(|e| Error::Io {
            message: format!("failed to write {what} to duplex stdin"),
            source: e,
            working_dir: None,
        })?;
    stdin.flush().await.map_err(|e| Error::Io {
        message: "failed to flush duplex stdin".to_string(),
        source: e,
        working_dir: None,
    })?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn build_args_default_includes_required_flags() {
        let args = DuplexOptions::default().build_args();
        assert!(args.contains(&"--print".to_string()));
        assert!(args.contains(&"--verbose".to_string()));
        assert!(
            args.windows(2)
                .any(|w| w == ["--output-format", "stream-json"])
        );
        assert!(
            args.windows(2)
                .any(|w| w == ["--input-format", "stream-json"])
        );
    }

    #[test]
    fn build_args_includes_model() {
        let args = DuplexOptions::default().model("haiku").build_args();
        assert!(args.windows(2).any(|w| w == ["--model", "haiku"]));
    }

    #[test]
    fn build_args_includes_system_prompts() {
        let args = DuplexOptions::default()
            .system_prompt("be concise")
            .append_system_prompt("also polite")
            .build_args();
        assert!(
            args.windows(2)
                .any(|w| w == ["--system-prompt", "be concise"])
        );
        assert!(
            args.windows(2)
                .any(|w| w == ["--append-system-prompt", "also polite"])
        );
    }

    #[test]
    fn build_args_appends_raw_args_last() {
        let args = DuplexOptions::default()
            .arg("--add-dir")
            .arg("/tmp/foo")
            .build_args();
        // Last two entries should be the additional args, in order.
        assert_eq!(&args[args.len() - 2..], &["--add-dir", "/tmp/foo"]);
    }

    // ─── to_command_string / spawn parity (#703) ───

    fn preview_claude() -> Claude {
        Claude::builder()
            .binary("/usr/local/bin/claude")
            .build()
            .unwrap()
    }

    #[test]
    fn spawn_command_args_prepends_global_args() {
        let claude = Claude::builder()
            .binary("/usr/local/bin/claude")
            .arg("--debug")
            .build()
            .unwrap();
        let args = DuplexOptions::default()
            .model("haiku")
            .spawn_command_args(&claude);
        assert_eq!(args[0], "--debug");
        assert_eq!(args[1], "--print");
        assert!(args.windows(2).any(|w| w == ["--model", "haiku"]));
    }

    #[test]
    fn to_command_string_is_binary_plus_spawn_args() {
        // The preview must be exactly the binary plus the args the
        // spawn path assembles (no arg here needs quoting).
        let claude = Claude::builder()
            .binary("/usr/local/bin/claude")
            .arg("--debug")
            .build()
            .unwrap();
        let opts = DuplexOptions::default().agent("reviewer");
        let expected = format!(
            "/usr/local/bin/claude {}",
            opts.spawn_command_args(&claude).join(" ")
        );
        assert_eq!(opts.to_command_string(&claude), expected);
    }

    #[test]
    fn to_command_string_includes_persona_flags() {
        // Repro from #703: a spawn configured with an agent, allowed
        // tools, and setting sources must show all three in the echo.
        let command_str = DuplexOptions::default()
            .agent("reviewer")
            .allowed_tool("Read")
            .allowed_tool("Bash(git:*)")
            .setting_sources("project")
            .to_command_string(&preview_claude());
        assert!(command_str.starts_with("/usr/local/bin/claude"));
        assert!(command_str.contains("--agent reviewer"));
        assert!(command_str.contains("--allowed-tools 'Read,Bash(git:*)'"));
        assert!(command_str.contains("--setting-sources project"));
    }

    #[test]
    fn to_command_string_quotes_args_with_spaces() {
        let command_str = DuplexOptions::default()
            .system_prompt("be concise")
            .to_command_string(&preview_claude());
        assert!(command_str.contains("--system-prompt 'be concise'"));
    }

    #[test]
    fn to_command_string_does_not_consume_options() {
        // Borrowing preview: the same options remain usable after the
        // echo (previewed first, then handed to spawn).
        let claude = preview_claude();
        let opts = DuplexOptions::default().model("haiku");
        let first = opts.to_command_string(&claude);
        let second = opts.to_command_string(&claude);
        assert_eq!(first, second);
    }

    #[test]
    fn build_args_includes_resume_when_set() {
        let args = DuplexOptions::default().resume("abc-123").build_args();
        assert!(args.windows(2).any(|w| w == ["--resume", "abc-123"]));
    }

    #[test]
    fn build_args_omits_resume_by_default() {
        let args = DuplexOptions::default().build_args();
        assert!(
            !args.iter().any(|a| a == "--resume"),
            "--resume should not appear without an explicit resume(...) call; got {args:?}"
        );
    }

    #[test]
    fn build_args_includes_continue_when_set() {
        let args = DuplexOptions::default().continue_session().build_args();
        assert!(args.iter().any(|a| a == "--continue"));
    }

    #[test]
    fn build_args_omits_continue_by_default() {
        let args = DuplexOptions::default().build_args();
        assert!(!args.iter().any(|a| a == "--continue"));
    }

    #[test]
    fn build_args_includes_worktree_flag_without_name() {
        let args = DuplexOptions::default().worktree(None::<&str>).build_args();
        assert!(args.iter().any(|a| a == "--worktree"));
        // No name means no positional follows --worktree.
        let pos = args.iter().position(|a| a == "--worktree").unwrap();
        assert!(
            args.get(pos + 1).is_none_or(|a| a.starts_with("--")),
            "--worktree without a name should not be followed by a positional; got {args:?}"
        );
    }

    #[test]
    fn build_args_includes_worktree_flag_with_name() {
        let args = DuplexOptions::default()
            .worktree(Some("agent-xyz"))
            .build_args();
        let pos = args.iter().position(|a| a == "--worktree").unwrap();
        assert_eq!(args.get(pos + 1).map(String::as_str), Some("agent-xyz"));
    }

    #[test]
    fn build_args_omits_worktree_by_default() {
        let args = DuplexOptions::default().build_args();
        assert!(
            !args.iter().any(|a| a == "--worktree"),
            "--worktree should not appear without an explicit worktree(...) call; got {args:?}"
        );
    }

    #[test]
    fn worktree_lands_before_additional_args() {
        // Same `--` ordering bug class as resume.
        let args = DuplexOptions::default()
            .worktree(Some("foo"))
            .arg("--")
            .arg("trailing")
            .build_args();
        let wt_pos = args.iter().position(|a| a == "--worktree").unwrap();
        let dash_dash_pos = args.iter().position(|a| a == "--").unwrap();
        assert!(
            wt_pos < dash_dash_pos,
            "--worktree must precede `--` separator; got {args:?}"
        );
    }

    #[test]
    fn build_args_includes_agent_when_set() {
        let args = DuplexOptions::default().agent("rust-qa").build_args();
        assert!(
            args.windows(2).any(|w| w == ["--agent", "rust-qa"]),
            "missing --agent rust-qa in {args:?}"
        );
    }

    #[test]
    fn build_args_omits_agent_by_default() {
        let args = DuplexOptions::default().build_args();
        assert!(
            !args.iter().any(|a| a == "--agent"),
            "--agent should not appear without an explicit agent(...) call; got {args:?}"
        );
    }

    #[test]
    fn build_args_includes_agents_json_when_set() {
        let json = r#"{"reviewer":{"description":"r","prompt":"p"}}"#;
        let args = DuplexOptions::default().agents_json(json).build_args();
        let pos = args.iter().position(|a| a == "--agents").unwrap();
        assert_eq!(args.get(pos + 1).map(String::as_str), Some(json));
    }

    #[test]
    fn build_args_omits_agents_json_by_default() {
        let args = DuplexOptions::default().build_args();
        assert!(!args.iter().any(|a| a == "--agents"));
    }

    #[test]
    fn agent_and_agents_json_compose() {
        let json = r#"{"reviewer":{"description":"r","prompt":"p"}}"#;
        let args = DuplexOptions::default()
            .agents_json(json)
            .agent("reviewer")
            .build_args();
        // Both flags present.
        assert!(args.iter().any(|a| a == "--agents"));
        assert!(args.iter().any(|a| a == "--agent"));
    }

    #[test]
    fn agent_lands_before_additional_args() {
        let args = DuplexOptions::default()
            .agent("rust-qa")
            .arg("--")
            .arg("trailing")
            .build_args();
        let agent_pos = args.iter().position(|a| a == "--agent").unwrap();
        let dash_dash_pos = args.iter().position(|a| a == "--").unwrap();
        assert!(
            agent_pos < dash_dash_pos,
            "--agent must precede `--` separator; got {args:?}"
        );
    }

    #[test]
    fn agents_json_lands_before_additional_args() {
        let args = DuplexOptions::default()
            .agents_json("{}")
            .arg("--")
            .arg("trailing")
            .build_args();
        let agents_pos = args.iter().position(|a| a == "--agents").unwrap();
        let dash_dash_pos = args.iter().position(|a| a == "--").unwrap();
        assert!(
            agents_pos < dash_dash_pos,
            "--agents must precede `--` separator; got {args:?}"
        );
    }

    // -- QueryCommand knob-set parity (#672) -------------------------

    #[test]
    fn build_args_includes_session_id() {
        let args = DuplexOptions::default().session_id("sid-9").build_args();
        assert!(args.windows(2).any(|w| w == ["--session-id", "sid-9"]));
    }

    #[test]
    fn build_args_includes_setting_sources() {
        let args = DuplexOptions::default()
            .setting_sources("user,project")
            .build_args();
        assert!(
            args.windows(2)
                .any(|w| w == ["--setting-sources", "user,project"]),
            "got {args:?}"
        );
    }

    #[test]
    fn build_args_omits_setting_sources_by_default() {
        let args = DuplexOptions::default().build_args();
        assert!(!args.iter().any(|a| a == "--setting-sources"));
    }

    #[test]
    fn build_args_hermetic_emits_full_seal() {
        let args = DuplexOptions::default().hermetic().build_args();
        assert!(
            args.windows(2)
                .any(|w| w[0] == "--setting-sources" && w[1].is_empty()),
            "got {args:?}"
        );
        assert!(args.iter().any(|a| a == "--strict-mcp-config"));
        assert!(
            args.iter()
                .any(|a| a == "--exclude-dynamic-system-prompt-sections")
        );
        // A hermetic seal must never imply --bare.
        assert!(!args.iter().any(|a| a == "--bare"));
    }

    #[test]
    fn build_args_hermetic_scoped_project_keeps_user() {
        let args = DuplexOptions::default()
            .hermetic_scoped(HermeticScope::Project)
            .build_args();
        assert!(args.windows(2).any(|w| w == ["--setting-sources", "user"]));
        assert!(args.iter().any(|a| a == "--strict-mcp-config"));
    }

    #[test]
    fn build_args_includes_json_schema() {
        let schema = r#"{"type":"object"}"#;
        let args = DuplexOptions::default().json_schema(schema).build_args();
        assert!(args.windows(2).any(|w| w == ["--json-schema", schema]));
    }

    #[test]
    fn build_args_joins_allowed_tools_comma_separated() {
        let args = DuplexOptions::default()
            .allowed_tools(["Read", "Bash(git log:*)"])
            .allowed_tool("Write")
            .build_args();
        assert!(
            args.windows(2)
                .any(|w| w == ["--allowed-tools", "Read,Bash(git log:*),Write"]),
            "missing joined --allowed-tools in {args:?}"
        );
    }

    #[test]
    fn build_args_joins_disallowed_tools_comma_separated() {
        let args = DuplexOptions::default()
            .disallowed_tools(["WebSearch"])
            .disallowed_tool("WebFetch")
            .build_args();
        assert!(
            args.windows(2)
                .any(|w| w == ["--disallowed-tools", "WebSearch,WebFetch"]),
            "missing joined --disallowed-tools in {args:?}"
        );
    }

    #[test]
    fn build_args_includes_caps() {
        let args = DuplexOptions::default()
            .max_turns(4)
            .max_budget_usd(0.25)
            .build_args();
        assert!(args.windows(2).any(|w| w == ["--max-turns", "4"]));
        assert!(args.windows(2).any(|w| w == ["--max-budget-usd", "0.25"]));
    }

    #[test]
    fn build_args_includes_fallback_model_and_effort() {
        let args = DuplexOptions::default()
            .fallback_model("haiku")
            .effort(Effort::Low)
            .build_args();
        assert!(args.windows(2).any(|w| w == ["--fallback-model", "haiku"]));
        assert!(args.windows(2).any(|w| w == ["--effort", "low"]));
    }

    #[test]
    fn build_args_repeats_add_dir_and_mcp_config() {
        let args = DuplexOptions::default()
            .add_dir("/a")
            .add_dir("/b")
            .mcp_config("x.json")
            .strict_mcp_config()
            .build_args();
        assert!(args.windows(2).any(|w| w == ["--add-dir", "/a"]));
        assert!(args.windows(2).any(|w| w == ["--add-dir", "/b"]));
        assert!(args.windows(2).any(|w| w == ["--mcp-config", "x.json"]));
        assert!(args.iter().any(|a| a == "--strict-mcp-config"));
    }

    #[test]
    fn build_args_includes_no_session_persistence() {
        let args = DuplexOptions::default()
            .no_session_persistence()
            .build_args();
        assert!(args.iter().any(|a| a == "--no-session-persistence"));
    }

    // ─── #690: parity builders promoted from QueryCommand ───

    #[test]
    fn build_args_joins_tools_comma_separated() {
        let args = DuplexOptions::default()
            .tools(["Bash", "Read", "Edit"])
            .build_args();
        assert!(
            args.windows(2).any(|w| w == ["--tools", "Bash,Read,Edit"]),
            "missing joined --tools in {args:?}"
        );
    }

    #[test]
    fn build_args_repeats_file_per_spec() {
        let args = DuplexOptions::default()
            .file("file_a:doc.txt")
            .file("file_b:notes.md")
            .build_args();
        assert_eq!(args.iter().filter(|a| *a == "--file").count(), 2);
        assert!(args.iter().any(|a| a == "file_a:doc.txt"));
        assert!(args.iter().any(|a| a == "file_b:notes.md"));
    }

    #[test]
    fn build_args_includes_settings() {
        let args = DuplexOptions::default()
            .settings("/tmp/settings.json")
            .build_args();
        assert!(
            args.windows(2)
                .any(|w| w == ["--settings", "/tmp/settings.json"])
        );
    }

    #[test]
    fn build_args_includes_fork_session() {
        let args = DuplexOptions::default().fork_session().build_args();
        assert!(args.iter().any(|a| a == "--fork-session"));
    }

    #[test]
    fn build_args_includes_debug_filter_and_file() {
        let args = DuplexOptions::default()
            .debug_filter("api,hooks")
            .debug_file("/tmp/debug.log")
            .build_args();
        assert!(args.windows(2).any(|w| w == ["--debug", "api,hooks"]));
        assert!(
            args.windows(2)
                .any(|w| w == ["--debug-file", "/tmp/debug.log"])
        );
    }

    #[test]
    fn build_args_includes_betas() {
        let args = DuplexOptions::default().betas("feature-x").build_args();
        assert!(args.windows(2).any(|w| w == ["--betas", "feature-x"]));
    }

    #[test]
    fn build_args_repeats_plugin_dir_and_url() {
        let args = DuplexOptions::default()
            .plugin_dir("/plugins/a")
            .plugin_dir("/plugins/b")
            .plugin_url("https://example.com/p.zip")
            .build_args();
        assert_eq!(args.iter().filter(|a| *a == "--plugin-dir").count(), 2);
        assert!(
            args.windows(2)
                .any(|w| w == ["--plugin-url", "https://example.com/p.zip"])
        );
    }

    #[test]
    fn build_args_includes_bare_family_bool_flags() {
        let args = DuplexOptions::default()
            .tmux()
            .bare()
            .safe_mode()
            .disable_slash_commands()
            .include_hook_events()
            .exclude_dynamic_system_prompt_sections()
            .build_args();
        for flag in [
            "--tmux",
            "--bare",
            "--safe-mode",
            "--disable-slash-commands",
            "--include-hook-events",
            "--exclude-dynamic-system-prompt-sections",
        ] {
            assert!(args.iter().any(|a| a == flag), "missing {flag} in {args:?}");
        }
    }

    #[test]
    fn build_args_includes_name() {
        let args = DuplexOptions::default().name("my session").build_args();
        assert!(args.windows(2).any(|w| w == ["--name", "my session"]));
    }

    #[test]
    fn build_args_omits_promoted_parity_flags_by_default() {
        let args = DuplexOptions::default().build_args();
        for flag in [
            "--tools",
            "--file",
            "--settings",
            "--fork-session",
            "--debug",
            "--debug-file",
            "--betas",
            "--plugin-dir",
            "--plugin-url",
            "--tmux",
            "--bare",
            "--safe-mode",
            "--disable-slash-commands",
            "--include-hook-events",
            "--exclude-dynamic-system-prompt-sections",
            "--name",
        ] {
            assert!(
                !args.iter().any(|a| a == flag),
                "{flag} should be absent by default; got {args:?}"
            );
        }
    }

    #[test]
    fn parity_flags_land_before_additional_args() {
        // Same `--` ordering bug class as resume/agent.
        let args = DuplexOptions::default()
            .max_turns(2)
            .json_schema("{}")
            .arg("--")
            .arg("trailing")
            .build_args();
        let dash_dash_pos = args.iter().position(|a| a == "--").unwrap();
        for flag in ["--max-turns", "--json-schema"] {
            let pos = args.iter().position(|a| a == flag).unwrap();
            assert!(
                pos < dash_dash_pos,
                "{flag} must precede `--` separator; got {args:?}"
            );
        }
    }

    #[test]
    fn build_args_omits_parity_flags_by_default() {
        let args = DuplexOptions::default().build_args();
        for flag in [
            "--session-id",
            "--json-schema",
            "--allowed-tools",
            "--disallowed-tools",
            "--max-turns",
            "--max-budget-usd",
            "--fallback-model",
            "--effort",
            "--add-dir",
            "--mcp-config",
            "--strict-mcp-config",
            "--no-session-persistence",
        ] {
            assert!(
                !args.iter().any(|a| a == flag),
                "{flag} should not appear by default; got {args:?}"
            );
        }
    }

    #[test]
    fn resume_lands_before_additional_args() {
        // Catches the same class of bug as QueryCommand::execute_json
        // had: a flag appended after the user-supplied raw args (which
        // typically include `--`) gets eaten as a positional. Resume
        // must precede any caller-injected `arg(...)`.
        let args = DuplexOptions::default()
            .resume("xyz")
            .arg("--")
            .arg("trailing")
            .build_args();
        let resume_pos = args.iter().position(|a| a == "--resume").unwrap();
        let dash_dash_pos = args.iter().position(|a| a == "--").unwrap();
        assert!(
            resume_pos < dash_dash_pos,
            "--resume must precede `--` separator; got {args:?}"
        );
    }

    #[test]
    fn turn_result_accessors_pull_from_result() {
        let r = TurnResult {
            result: json!({
                "type": "result",
                "result": "hello",
                "session_id": "sess-123",
                "total_cost_usd": 0.0042,
                "duration_ms": 1234_u64,
            }),
            events: vec![],
        };
        assert_eq!(r.result_text(), Some("hello"));
        assert_eq!(r.session_id(), Some("sess-123"));
        assert_eq!(r.total_cost_usd(), Some(0.0042));
        assert_eq!(r.duration_ms(), Some(1234));
    }

    #[test]
    fn turn_result_total_cost_falls_back_to_legacy_field() {
        let r = TurnResult {
            result: json!({ "cost_usd": 0.5 }),
            events: vec![],
        };
        assert_eq!(r.total_cost_usd(), Some(0.5));
    }

    #[test]
    fn turn_result_accessors_return_none_when_missing() {
        let r = TurnResult {
            result: json!({}),
            events: vec![],
        };
        assert_eq!(r.result_text(), None);
        assert_eq!(r.session_id(), None);
        assert_eq!(r.total_cost_usd(), None);
        assert_eq!(r.duration_ms(), None);
    }

    #[test]
    fn handle_inbound_appends_non_result_to_pending_events() {
        let (tx, _reply_rx) = oneshot::channel::<Result<TurnResult>>();
        let (events_tx, _events_rx) = broadcast::channel(16);
        let mut pending = Some((tx, Vec::new()));
        handle_inbound(
            json!({ "type": "assistant", "message": {} }),
            &mut pending,
            &events_tx,
        );
        let (_, events) = pending.as_ref().unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(
            events[0].get("type").and_then(Value::as_str),
            Some("assistant")
        );
    }

    #[test]
    fn handle_inbound_resolves_pending_on_result() {
        let (tx, rx) = oneshot::channel::<Result<TurnResult>>();
        let (events_tx, _events_rx) = broadcast::channel(16);
        let mut pending = Some((tx, vec![json!({ "type": "assistant" })]));
        handle_inbound(
            json!({ "type": "result", "result": "ok" }),
            &mut pending,
            &events_tx,
        );
        assert!(pending.is_none());
        let received = rx.blocking_recv().unwrap().unwrap();
        assert_eq!(received.result_text(), Some("ok"));
        assert_eq!(received.events.len(), 1);
    }

    #[test]
    fn handle_inbound_drops_orphans_without_pending_turn() {
        let (events_tx, _events_rx) = broadcast::channel(16);
        let mut pending: Option<(oneshot::Sender<Result<TurnResult>>, Vec<Value>)> = None;
        handle_inbound(json!({ "type": "assistant" }), &mut pending, &events_tx);
        handle_inbound(
            json!({ "type": "result", "result": "ok" }),
            &mut pending,
            &events_tx,
        );
        assert!(pending.is_none());
    }

    #[test]
    fn handle_inbound_broadcasts_classified_event() {
        let (tx, _reply_rx) = oneshot::channel::<Result<TurnResult>>();
        let (events_tx, mut events_rx) = broadcast::channel(16);
        let mut pending = Some((tx, Vec::new()));
        handle_inbound(
            json!({ "type": "assistant", "message": { "role": "assistant" } }),
            &mut pending,
            &events_tx,
        );
        let event = events_rx.try_recv().expect("classified event broadcast");
        assert!(matches!(event, InboundEvent::Assistant(_)));
    }

    #[test]
    fn handle_inbound_does_not_broadcast_result() {
        let (tx, _reply_rx) = oneshot::channel::<Result<TurnResult>>();
        let (events_tx, mut events_rx) = broadcast::channel(16);
        let mut pending = Some((tx, Vec::new()));
        handle_inbound(
            json!({ "type": "result", "result": "ok" }),
            &mut pending,
            &events_tx,
        );
        // Result is not broadcast -- it lands in TurnResult.result.
        assert!(events_rx.try_recv().is_err());
    }

    #[test]
    fn classify_system_init_pulls_session_id() {
        let v = json!({
            "type": "system",
            "subtype": "init",
            "session_id": "sess-abc",
        });
        match classify(&v) {
            InboundEvent::SystemInit { session_id } => assert_eq!(session_id, "sess-abc"),
            other => panic!("expected SystemInit, got {other:?}"),
        }
    }

    #[test]
    fn classify_system_without_init_subtype_is_other() {
        let v = json!({ "type": "system", "subtype": "compaction" });
        assert!(matches!(classify(&v), InboundEvent::Other(_)));
    }

    #[test]
    fn classify_system_init_without_session_id_is_other() {
        let v = json!({ "type": "system", "subtype": "init" });
        assert!(matches!(classify(&v), InboundEvent::Other(_)));
    }

    #[test]
    fn classify_assistant_stream_event_user() {
        assert!(matches!(
            classify(&json!({ "type": "assistant" })),
            InboundEvent::Assistant(_)
        ));
        assert!(matches!(
            classify(&json!({ "type": "stream_event" })),
            InboundEvent::StreamEvent(_)
        ));
        assert!(matches!(
            classify(&json!({ "type": "user" })),
            InboundEvent::User(_)
        ));
    }

    #[test]
    fn classify_unknown_type_is_other() {
        assert!(matches!(
            classify(&json!({ "type": "control_request" })),
            InboundEvent::Other(_)
        ));
        assert!(matches!(
            classify(&json!({ "type": "future_thing" })),
            InboundEvent::Other(_)
        ));
        assert!(matches!(classify(&json!({})), InboundEvent::Other(_)));
    }

    #[test]
    fn build_args_does_not_emit_subscriber_capacity_flag() {
        // subscriber_capacity is runtime config, not a CLI arg.
        let args = DuplexOptions::default()
            .subscriber_capacity(64)
            .build_args();
        assert!(!args.iter().any(|a| a.contains("subscriber")));
        assert!(!args.iter().any(|a| a.contains("capacity")));
    }

    #[test]
    fn build_args_includes_permission_prompt_tool_when_handler_set() {
        let handler = PermissionHandler::new(|_req| async move {
            PermissionDecision::Allow {
                updated_input: None,
            }
        });
        let args = DuplexOptions::default().on_permission(handler).build_args();
        assert!(
            args.windows(2)
                .any(|w| w == ["--permission-prompt-tool", "stdio"])
        );
    }

    #[test]
    fn build_args_omits_permission_prompt_tool_without_handler() {
        let args = DuplexOptions::default().build_args();
        assert!(!args.iter().any(|a| a == "--permission-prompt-tool"));
    }

    #[test]
    fn build_args_emits_permission_mode_flag() {
        let args = DuplexOptions::default()
            .permission_mode(PermissionMode::AcceptEdits)
            .build_args();
        assert!(
            args.windows(2)
                .any(|w| w == ["--permission-mode", "acceptEdits"]),
            "missing --permission-mode acceptEdits in {args:?}"
        );
    }

    #[test]
    fn build_args_emits_plan_mode() {
        let args = DuplexOptions::default()
            .permission_mode(PermissionMode::Plan)
            .build_args();
        assert!(args.windows(2).any(|w| w == ["--permission-mode", "plan"]));
    }

    #[test]
    fn build_args_omits_permission_mode_by_default() {
        let args = DuplexOptions::default().build_args();
        assert!(!args.iter().any(|a| a == "--permission-mode"));
    }

    #[test]
    fn build_args_emits_dangerously_skip_permissions_flag() {
        let args = DuplexOptions::default()
            .dangerously_skip_permissions()
            .build_args();
        assert!(args.iter().any(|a| a == "--dangerously-skip-permissions"));
    }

    #[test]
    fn build_args_omits_dangerously_skip_by_default() {
        let args = DuplexOptions::default().build_args();
        assert!(!args.iter().any(|a| a == "--dangerously-skip-permissions"));
    }

    #[test]
    fn parse_permission_request_extracts_fields() {
        let msg = json!({
            "type": "control_request",
            "request_id": "req-1",
            "request": {
                "subtype": "can_use_tool",
                "tool_name": "Bash",
                "input": { "command": "ls" }
            }
        });
        let req = parse_permission_request(&msg).expect("permission request");
        assert_eq!(req.request_id, "req-1");
        assert_eq!(req.tool_name, "Bash");
        assert_eq!(req.input, json!({ "command": "ls" }));
        assert_eq!(
            req.raw.get("subtype").and_then(Value::as_str),
            Some("can_use_tool")
        );
    }

    #[test]
    fn parse_permission_request_returns_none_when_missing_request_id() {
        let msg = json!({
            "type": "control_request",
            "request": {
                "subtype": "can_use_tool",
                "tool_name": "Bash",
            }
        });
        assert!(parse_permission_request(&msg).is_none());
    }

    #[test]
    fn parse_permission_request_returns_none_when_missing_tool_name() {
        let msg = json!({
            "type": "control_request",
            "request_id": "req-1",
            "request": { "subtype": "can_use_tool" }
        });
        assert!(parse_permission_request(&msg).is_none());
    }

    #[test]
    fn parse_permission_request_handles_missing_input() {
        let msg = json!({
            "type": "control_request",
            "request_id": "req-1",
            "request": {
                "subtype": "can_use_tool",
                "tool_name": "Bash",
            }
        });
        let req = parse_permission_request(&msg).expect("request");
        assert_eq!(req.input, Value::Null);
    }

    #[test]
    fn handle_inbound_returns_permission_for_can_use_tool() {
        let (tx, _reply_rx) = oneshot::channel::<Result<TurnResult>>();
        let (events_tx, _events_rx) = broadcast::channel(16);
        let mut pending = Some((tx, Vec::new()));
        let action = handle_inbound(
            json!({
                "type": "control_request",
                "request_id": "req-1",
                "request": {
                    "subtype": "can_use_tool",
                    "tool_name": "Bash",
                    "input": { "command": "ls" }
                }
            }),
            &mut pending,
            &events_tx,
        );
        match action {
            InboundAction::Permission(req) => {
                assert_eq!(req.request_id, "req-1");
                assert_eq!(req.tool_name, "Bash");
            }
            InboundAction::None | InboundAction::ControlResponse { .. } => {
                panic!("expected Permission action");
            }
        }
        // Event should also be accumulated in the pending turn.
        let (_, events) = pending.as_ref().unwrap();
        assert_eq!(events.len(), 1);
    }

    #[test]
    fn handle_inbound_treats_unknown_control_request_as_other() {
        let (tx, _reply_rx) = oneshot::channel::<Result<TurnResult>>();
        let (events_tx, mut events_rx) = broadcast::channel(16);
        let mut pending = Some((tx, Vec::new()));
        let action = handle_inbound(
            json!({
                "type": "control_request",
                "request_id": "req-2",
                "request": { "subtype": "future_subtype" }
            }),
            &mut pending,
            &events_tx,
        );
        assert!(matches!(action, InboundAction::None));
        let event = events_rx.try_recv().expect("broadcast");
        assert!(matches!(event, InboundEvent::Other(_)));
    }

    #[tokio::test]
    async fn permission_handler_invokes_closure_async() {
        let handler = PermissionHandler::new(|req| async move {
            if req.tool_name == "Bash" {
                PermissionDecision::Deny {
                    message: "no bash".into(),
                }
            } else {
                PermissionDecision::Allow {
                    updated_input: None,
                }
            }
        });
        let req = PermissionRequest {
            request_id: "r1".into(),
            tool_name: "Bash".into(),
            input: Value::Null,
            raw: Value::Null,
        };
        match handler.invoke(req).await {
            PermissionDecision::Deny { message } => assert_eq!(message, "no bash"),
            other => panic!("expected Deny, got {other:?}"),
        }
    }

    #[test]
    fn parse_control_response_extracts_success() {
        let msg = json!({
            "type": "control_response",
            "response": {
                "request_id": "interrupt-1",
                "subtype": "success",
                "response": {}
            }
        });
        let (id, outcome) = parse_control_response(&msg).expect("parsed");
        assert_eq!(id, "interrupt-1");
        assert!(outcome.is_ok());
    }

    #[test]
    fn parse_control_response_extracts_error_with_message() {
        let msg = json!({
            "type": "control_response",
            "response": {
                "request_id": "interrupt-2",
                "subtype": "error",
                "error": "no turn in flight"
            }
        });
        let (id, outcome) = parse_control_response(&msg).expect("parsed");
        assert_eq!(id, "interrupt-2");
        match outcome {
            Err(Error::DuplexControlFailed { message }) => {
                assert_eq!(message, "no turn in flight");
            }
            other => panic!("expected DuplexControlFailed, got {other:?}"),
        }
    }

    #[test]
    fn parse_control_response_returns_none_on_missing_request_id() {
        let msg = json!({
            "type": "control_response",
            "response": { "subtype": "success" }
        });
        assert!(parse_control_response(&msg).is_none());
    }

    #[test]
    fn parse_control_response_returns_none_on_unknown_subtype() {
        let msg = json!({
            "type": "control_response",
            "response": { "request_id": "x", "subtype": "future_subtype" }
        });
        assert!(parse_control_response(&msg).is_none());
    }

    #[test]
    fn handle_inbound_returns_control_response_action() {
        let (tx, _reply_rx) = oneshot::channel::<Result<TurnResult>>();
        let (events_tx, _events_rx) = broadcast::channel(16);
        let mut pending = Some((tx, Vec::new()));
        let action = handle_inbound(
            json!({
                "type": "control_response",
                "response": {
                    "request_id": "interrupt-1",
                    "subtype": "success",
                    "response": {}
                }
            }),
            &mut pending,
            &events_tx,
        );
        match action {
            InboundAction::ControlResponse {
                request_id,
                outcome,
            } => {
                assert_eq!(request_id, "interrupt-1");
                assert!(outcome.is_ok());
            }
            InboundAction::None | InboundAction::Permission(_) => {
                panic!("expected ControlResponse action");
            }
        }
    }

    #[test]
    fn handle_inbound_treats_malformed_control_response_as_other() {
        let (tx, _reply_rx) = oneshot::channel::<Result<TurnResult>>();
        let (events_tx, mut events_rx) = broadcast::channel(16);
        let mut pending = Some((tx, Vec::new()));
        let action = handle_inbound(
            json!({
                "type": "control_response",
                "response": { "subtype": "success" }
            }),
            &mut pending,
            &events_tx,
        );
        assert!(matches!(action, InboundAction::None));
        let event = events_rx.try_recv().expect("broadcast");
        assert!(matches!(event, InboundEvent::Other(_)));
    }

    #[tokio::test]
    async fn permission_handler_clones_arc() {
        let handler = PermissionHandler::new(|_req| async move {
            PermissionDecision::Allow {
                updated_input: None,
            }
        });
        let cloned = handler.clone();
        let req = PermissionRequest {
            request_id: "r1".into(),
            tool_name: "Read".into(),
            input: Value::Null,
            raw: Value::Null,
        };
        // Both handles invoke the same underlying closure.
        let _ = handler.invoke(req.clone()).await;
        let _ = cloned.invoke(req).await;
    }

    /// Build a `DuplexSession` whose channels are wired up but whose
    /// background task is a no-op. Tests can drive the watch state
    /// machine via the returned `exit_tx` and observe the public
    /// accessors. The fake task idles on a oneshot so it stays alive
    /// for the life of the test (no JoinHandle::abort handshake
    /// needed).
    fn fake_session(
        initial: SessionExitStatus,
    ) -> (
        DuplexSession,
        watch::Sender<SessionExitStatus>,
        oneshot::Sender<()>,
    ) {
        let (outbound_tx, outbound_rx) = mpsc::unbounded_channel::<OutboundMsg>();
        let (events_tx, _events_rx) = broadcast::channel::<InboundEvent>(16);
        let (exit_tx, exit_rx) = watch::channel(initial);
        let (stop_tx, stop_rx) = oneshot::channel::<()>();

        let join = tokio::spawn(async move {
            let _outbound_rx = outbound_rx;
            let _ = stop_rx.await;
            Ok::<(), Error>(())
        });

        let session = DuplexSession {
            outbound_tx,
            events_tx,
            exit_rx,
            join,
        };
        (session, exit_tx, stop_tx)
    }

    #[tokio::test]
    async fn is_alive_true_while_running() {
        let (session, _exit_tx, _stop) = fake_session(SessionExitStatus::Running);
        assert!(session.is_alive());
    }

    #[tokio::test]
    async fn is_alive_false_after_completed() {
        let (session, exit_tx, _stop) = fake_session(SessionExitStatus::Running);
        exit_tx.send(SessionExitStatus::Completed).unwrap();
        assert!(!session.is_alive());
    }

    #[tokio::test]
    async fn is_alive_false_after_failed() {
        let (session, exit_tx, _stop) = fake_session(SessionExitStatus::Running);
        exit_tx
            .send(SessionExitStatus::Failed("boom".into()))
            .unwrap();
        assert!(!session.is_alive());
    }

    #[tokio::test]
    async fn exit_status_reports_running_initially() {
        let (session, _exit_tx, _stop) = fake_session(SessionExitStatus::Running);
        assert!(matches!(session.exit_status(), SessionExitStatus::Running));
    }

    #[tokio::test]
    async fn exit_status_reflects_completed() {
        let (session, exit_tx, _stop) = fake_session(SessionExitStatus::Running);
        exit_tx.send(SessionExitStatus::Completed).unwrap();
        assert!(matches!(
            session.exit_status(),
            SessionExitStatus::Completed
        ));
    }

    #[tokio::test]
    async fn exit_status_reflects_failed_with_message() {
        let (session, exit_tx, _stop) = fake_session(SessionExitStatus::Running);
        exit_tx
            .send(SessionExitStatus::Failed("oh no".into()))
            .unwrap();
        match session.exit_status() {
            SessionExitStatus::Failed(msg) => assert_eq!(msg, "oh no"),
            other => panic!("expected Failed, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn wait_for_exit_returns_immediately_when_already_terminal() {
        let (session, exit_tx, _stop) = fake_session(SessionExitStatus::Running);
        exit_tx.send(SessionExitStatus::Completed).unwrap();
        let status = tokio::time::timeout(Duration::from_secs(1), session.wait_for_exit())
            .await
            .expect("wait_for_exit should not block when already terminal");
        assert!(matches!(status, SessionExitStatus::Completed));
    }

    #[tokio::test]
    async fn wait_for_exit_blocks_until_state_transitions() {
        let (session, exit_tx, _stop) = fake_session(SessionExitStatus::Running);

        let waiter = async { session.wait_for_exit().await };
        let driver = async {
            tokio::time::sleep(Duration::from_millis(20)).await;
            exit_tx.send(SessionExitStatus::Completed).unwrap();
        };
        let (status, ()) = tokio::join!(waiter, driver);
        assert!(matches!(status, SessionExitStatus::Completed));
    }

    #[tokio::test]
    async fn wait_for_exit_supports_multiple_observers() {
        let (session, exit_tx, _stop) = fake_session(SessionExitStatus::Running);

        let waiter1 = async { session.wait_for_exit().await };
        let waiter2 = async { session.wait_for_exit().await };
        let driver = async {
            tokio::time::sleep(Duration::from_millis(20)).await;
            exit_tx
                .send(SessionExitStatus::Failed("crash".into()))
                .unwrap();
        };
        let (s1, s2, ()) = tokio::join!(waiter1, waiter2, driver);
        match s1 {
            SessionExitStatus::Failed(msg) => assert_eq!(msg, "crash"),
            other => panic!("waiter1 expected Failed, got {other:?}"),
        }
        match s2 {
            SessionExitStatus::Failed(msg) => assert_eq!(msg, "crash"),
            other => panic!("waiter2 expected Failed, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn wait_for_exit_returns_last_value_when_sender_dropped() {
        // Defensive: if exit_tx is dropped without ever publishing a
        // terminal value, wait_for_exit should fall back to the last
        // observed state rather than hang.
        let (session, exit_tx, _stop) = fake_session(SessionExitStatus::Running);
        let waiter = async { session.wait_for_exit().await };
        let driver = async {
            tokio::time::sleep(Duration::from_millis(20)).await;
            drop(exit_tx);
        };
        let (status, ()) = tokio::time::timeout(Duration::from_secs(1), async {
            tokio::join!(waiter, driver)
        })
        .await
        .expect("wait_for_exit must not hang when sender is dropped");
        assert!(matches!(status, SessionExitStatus::Running));
    }
}
