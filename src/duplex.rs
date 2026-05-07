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
//! Most consumers of this crate should keep using [`QueryCommand`] and
//! [`Session`] -- one subprocess per turn, continuity via `--resume`.
//! That is the right shape for short-lived processes (CLIs, build
//! scripts, batch jobs, lambdas) which have no long-running runtime
//! to host a session.
//!
//! [`DuplexSession`] is for the inverse case: an agent server, IDE
//! backend, daemon, or chat UI where holding a `claude` subprocess
//! open across turns amortizes init cost and unlocks capabilities
//! that are awkward or impossible from a transient subprocess
//! (mid-turn permission decisions, hook flow, clean interrupts).
//! Those capabilities ship in subsequent PRs; this PR is the
//! minimum happy path.
//!
//! [`QueryCommand`]: crate::QueryCommand
//! [`Session`]: crate::session::Session
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
use tokio::sync::{broadcast, mpsc, oneshot};
use tokio::task::JoinHandle;
use tracing::{debug, warn};

use crate::Claude;
use crate::error::{Error, Result};

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
/// Builder methods cover the most common spawn-time options. The
/// spawn call always includes
/// `--print --verbose --input-format stream-json --output-format stream-json`
/// regardless of these options.
#[derive(Debug, Default, Clone)]
pub struct DuplexOptions {
    model: Option<String>,
    system_prompt: Option<String>,
    append_system_prompt: Option<String>,
    additional_args: Vec<String>,
    subscriber_capacity: Option<usize>,
    on_permission: Option<PermissionHandler>,
}

impl DuplexOptions {
    /// Set the model for this session (`--model`).
    #[must_use]
    pub fn model(mut self, model: impl Into<String>) -> Self {
        self.model = Some(model.into());
        self
    }

    /// Set the system prompt for this session (`--system-prompt`).
    #[must_use]
    pub fn system_prompt(mut self, prompt: impl Into<String>) -> Self {
        self.system_prompt = Some(prompt.into());
        self
    }

    /// Append to the default system prompt (`--append-system-prompt`).
    #[must_use]
    pub fn append_system_prompt(mut self, prompt: impl Into<String>) -> Self {
        self.append_system_prompt = Some(prompt.into());
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
    #[must_use]
    pub fn on_permission(mut self, handler: PermissionHandler) -> Self {
        self.on_permission = Some(handler);
        self
    }

    fn into_args(self) -> Vec<String> {
        let mut args = vec![
            "--print".to_string(),
            "--verbose".to_string(),
            "--output-format".to_string(),
            "stream-json".to_string(),
            "--input-format".to_string(),
            "stream-json".to_string(),
        ];

        if let Some(m) = self.model {
            args.push("--model".to_string());
            args.push(m);
        }
        if let Some(p) = self.system_prompt {
            args.push("--system-prompt".to_string());
            args.push(p);
        }
        if let Some(p) = self.append_system_prompt {
            args.push("--append-system-prompt".to_string());
            args.push(p);
        }
        if self.on_permission.is_some() {
            args.push("--permission-prompt-tool".to_string());
            args.push("stdio".to_string());
        }
        args.extend(self.additional_args);

        args
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

        let mut command_args = Vec::new();
        command_args.extend(claude.global_args.clone());
        command_args.extend(opts.into_args());

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

        if let Some(ref dir) = claude.working_dir {
            cmd.current_dir(dir);
        }

        let mut child = cmd.spawn().map_err(|e| Error::Io {
            message: format!("failed to spawn claude: {e}"),
            source: e,
            working_dir: claude.working_dir.clone(),
        })?;

        let stdin = child.stdin.take().expect("stdin was piped");
        let stdout = child.stdout.take().expect("stdout was piped");

        let (outbound_tx, outbound_rx) = mpsc::unbounded_channel();
        let (events_tx, _initial_rx) = broadcast::channel(capacity);

        let join = tokio::spawn(run_session(
            child,
            stdin,
            stdout,
            outbound_rx,
            events_tx.clone(),
            permission_handler,
        ));

        Ok(Self {
            outbound_tx,
            events_tx,
            join,
        })
    }

    /// Send one user message and await the closing result event.
    ///
    /// Returns [`Error::DuplexTurnInFlight`] if another turn is
    /// already pending, and [`Error::DuplexClosed`] if the session
    /// task has already exited.
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

async fn run_session(
    mut child: Child,
    mut stdin: ChildStdin,
    stdout: ChildStdout,
    mut outbound_rx: mpsc::UnboundedReceiver<OutboundMsg>,
    events_tx: broadcast::Sender<InboundEvent>,
    permission_handler: Option<PermissionHandler>,
) -> Result<()> {
    let mut lines = BufReader::new(stdout).lines();
    let mut pending: Option<(oneshot::Sender<Result<TurnResult>>, Vec<Value>)> = None;
    let mut pending_control: HashMap<String, oneshot::Sender<Result<()>>> = HashMap::new();
    let mut next_control_id: u64 = 0;
    let mut stream_err: Option<Error> = None;

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
        Ok(Ok(_status)) => {}
        Ok(Err(e)) => {
            warn!(error = %e, "failed to wait for duplex child");
        }
        Err(_) => {
            warn!("duplex child did not exit within shutdown budget; killing");
            let _ = child.kill().await;
        }
    }

    if let Some((reply, _)) = pending.take() {
        let _ = reply.send(Err(Error::DuplexClosed));
    }
    for (_, reply) in pending_control.drain() {
        let _ = reply.send(Err(Error::DuplexClosed));
    }

    match stream_err {
        Some(e) => Err(e),
        None => Ok(()),
    }
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
    fn into_args_default_includes_required_flags() {
        let args = DuplexOptions::default().into_args();
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
    fn into_args_includes_model() {
        let args = DuplexOptions::default().model("haiku").into_args();
        assert!(args.windows(2).any(|w| w == ["--model", "haiku"]));
    }

    #[test]
    fn into_args_includes_system_prompts() {
        let args = DuplexOptions::default()
            .system_prompt("be concise")
            .append_system_prompt("also polite")
            .into_args();
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
    fn into_args_appends_raw_args_last() {
        let args = DuplexOptions::default()
            .arg("--add-dir")
            .arg("/tmp/foo")
            .into_args();
        // Last two entries should be the additional args, in order.
        assert_eq!(&args[args.len() - 2..], &["--add-dir", "/tmp/foo"]);
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
    fn into_args_does_not_emit_subscriber_capacity_flag() {
        // subscriber_capacity is runtime config, not a CLI arg.
        let args = DuplexOptions::default().subscriber_capacity(64).into_args();
        assert!(!args.iter().any(|a| a.contains("subscriber")));
        assert!(!args.iter().any(|a| a.contains("capacity")));
    }

    #[test]
    fn into_args_includes_permission_prompt_tool_when_handler_set() {
        let handler = PermissionHandler::new(|_req| async move {
            PermissionDecision::Allow {
                updated_input: None,
            }
        });
        let args = DuplexOptions::default().on_permission(handler).into_args();
        assert!(
            args.windows(2)
                .any(|w| w == ["--permission-prompt-tool", "stdio"])
        );
    }

    #[test]
    fn into_args_omits_permission_prompt_tool_without_handler() {
        let args = DuplexOptions::default().into_args();
        assert!(!args.iter().any(|a| a == "--permission-prompt-tool"));
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
}
