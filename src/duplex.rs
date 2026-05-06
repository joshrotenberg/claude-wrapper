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
//! # Phased rollout
//!
//! This module is rolling out in four PRs tracked in
//! <https://github.com/joshrotenberg/claude-wrapper/issues/561>. The
//! current surface is PR 1: `spawn`, `send`, `close`. Subscribers,
//! mid-turn permission decisions, and `interrupt` land in
//! subsequent PRs.

use std::process::Stdio;
use std::time::Duration;

use serde_json::Value;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, ChildStdout, Command};
use tokio::sync::{mpsc, oneshot};
use tokio::task::JoinHandle;
use tracing::{debug, warn};

use crate::Claude;
use crate::error::{Error, Result};

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
    join: JoinHandle<Result<()>>,
}

#[derive(Debug)]
enum OutboundMsg {
    Send {
        prompt: String,
        reply: oneshot::Sender<Result<TurnResult>>,
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

        let join = tokio::spawn(run_session(child, stdin, stdout, outbound_rx));

        Ok(Self { outbound_tx, join })
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

    /// Close the session and wait for the underlying task to exit.
    ///
    /// Drops the outbound channel sender, which the session task
    /// observes as `recv() -> None`, then closes stdin and reaps the
    /// child.
    pub async fn close(self) -> Result<()> {
        drop(self.outbound_tx);
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
) -> Result<()> {
    let mut lines = BufReader::new(stdout).lines();
    let mut pending: Option<(oneshot::Sender<Result<TurnResult>>, Vec<Value>)> = None;
    let mut stream_err: Option<Error> = None;

    loop {
        tokio::select! {
            biased;

            line = lines.next_line() => match line {
                Ok(Some(l)) => {
                    if l.trim().is_empty() {
                        continue;
                    }
                    match serde_json::from_str::<Value>(&l) {
                        Ok(v) => handle_inbound(v, &mut pending),
                        Err(e) => {
                            debug!(line = %l, error = %e, "failed to parse duplex event, skipping");
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

    match stream_err {
        Some(e) => Err(e),
        None => Ok(()),
    }
}

fn handle_inbound(
    msg: Value,
    pending: &mut Option<(oneshot::Sender<Result<TurnResult>>, Vec<Value>)>,
) {
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
        }
        _ => {
            if let Some((_, events)) = pending.as_mut() {
                events.push(msg);
            } else {
                debug!("dropping inbound event with no pending turn");
            }
        }
    }
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
    let mut line = serde_json::to_string(&user_msg).map_err(|e| Error::Json {
        message: "failed to serialize duplex user message".to_string(),
        source: e,
    })?;
    line.push('\n');
    stdin
        .write_all(line.as_bytes())
        .await
        .map_err(|e| Error::Io {
            message: "failed to write user message to duplex stdin".to_string(),
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
        let (tx, _rx) = oneshot::channel::<Result<TurnResult>>();
        let mut pending = Some((tx, Vec::new()));
        handle_inbound(json!({ "type": "assistant", "message": {} }), &mut pending);
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
        let mut pending = Some((tx, vec![json!({ "type": "assistant" })]));
        handle_inbound(json!({ "type": "result", "result": "ok" }), &mut pending);
        assert!(pending.is_none());
        let received = rx.blocking_recv().unwrap().unwrap();
        assert_eq!(received.result_text(), Some("ok"));
        assert_eq!(received.events.len(), 1);
    }

    #[test]
    fn handle_inbound_drops_orphans_without_pending_turn() {
        let mut pending: Option<(oneshot::Sender<Result<TurnResult>>, Vec<Value>)> = None;
        handle_inbound(json!({ "type": "assistant" }), &mut pending);
        handle_inbound(json!({ "type": "result", "result": "ok" }), &mut pending);
        assert!(pending.is_none());
    }
}
