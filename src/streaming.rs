//! NDJSON streaming of `claude` events.
//!
//! [`stream_query`] (and its blocking peer `stream_query_sync`) run a
//! query in `stream-json` mode and hand each decoded event to a
//! caller-supplied callback as it arrives, rather than buffering the
//! whole run. Requires the `json` feature.

#[cfg(feature = "json")]
use std::collections::VecDeque;
#[cfg(feature = "json")]
use std::time::Duration;

#[cfg(all(feature = "json", feature = "async"))]
use tokio::io::{AsyncBufReadExt, AsyncReadExt, BufReader};
#[cfg(all(feature = "json", feature = "async"))]
use tokio::process::{ChildStderr, Command};
#[cfg(feature = "json")]
use tracing::{debug, warn};

#[cfg(feature = "json")]
use crate::Claude;
#[cfg(feature = "json")]
use crate::error::{Error, Result};
#[cfg(feature = "json")]
use crate::exec::CommandOutput;

#[cfg(feature = "json")]
const STREAM_DIAGNOSTIC_MAX_BYTES: usize = 16 * 1024;

/// Bounded stdout retained only for lines that are not stream events.
#[cfg(feature = "json")]
#[derive(Default)]
struct ParseFailureDiagnostics {
    lines: VecDeque<String>,
    bytes: usize,
}

#[cfg(feature = "json")]
impl ParseFailureDiagnostics {
    fn push(&mut self, line: &str) {
        // Reserve one byte per line for the separator added by
        // `into_string`. A single oversized line keeps its prefix,
        // where CLI diagnostics normally put the error category.
        let max_line_bytes = STREAM_DIAGNOSTIC_MAX_BYTES - 1;
        let line = if line.len() > max_line_bytes {
            let mut end = max_line_bytes;
            while !line.is_char_boundary(end) {
                end -= 1;
            }
            &line[..end]
        } else {
            line
        };
        let line_bytes = line.len() + 1;

        while self.bytes + line_bytes > STREAM_DIAGNOSTIC_MAX_BYTES {
            let Some(removed) = self.lines.pop_front() else {
                break;
            };
            self.bytes -= removed.len() + 1;
        }

        self.lines.push_back(line.to_string());
        self.bytes += line_bytes;
    }

    fn into_string(self) -> String {
        let mut output = String::with_capacity(self.bytes);
        for line in self.lines {
            output.push_str(&line);
            output.push('\n');
        }
        output.pop();
        output
    }
}

/// A single line from `--output-format stream-json` output.
///
/// Each line is an NDJSON object. The structure varies by message type,
/// so we provide the raw JSON value and convenience accessors.
#[cfg(feature = "json")]
#[derive(Debug, Clone, serde::Deserialize, serde::Serialize)]
pub struct StreamEvent {
    /// The raw JSON object for this event.
    #[serde(flatten)]
    pub data: serde_json::Value,
}

#[cfg(feature = "json")]
impl StreamEvent {
    /// Get the event type, if present.
    pub fn event_type(&self) -> Option<&str> {
        self.data.get("type").and_then(|v| v.as_str())
    }

    /// Get the message role, if present.
    pub fn role(&self) -> Option<&str> {
        self.data.get("role").and_then(|v| v.as_str())
    }

    /// Check if this is the final result message.
    pub fn is_result(&self) -> bool {
        self.event_type() == Some("result")
    }

    /// Extract the result text from a result event.
    pub fn result_text(&self) -> Option<&str> {
        self.data.get("result").and_then(|v| v.as_str())
    }

    /// Get the session ID if present.
    pub fn session_id(&self) -> Option<&str> {
        self.data.get("session_id").and_then(|v| v.as_str())
    }

    /// Get the cost in USD if present (usually on result events).
    ///
    /// Prefers `total_cost_usd` (the CLI's primary key) and falls back
    /// to the legacy `cost_usd` alias.
    pub fn cost_usd(&self) -> Option<f64> {
        self.data
            .get("total_cost_usd")
            .or_else(|| self.data.get("cost_usd"))
            .and_then(|v| v.as_f64())
    }

    /// Decode a partial-message event into a typed view.
    ///
    /// Returns `Some` when the event is one of the content-block lifecycle
    /// events surfaced by [`QueryCommand::include_partial_messages`] -- start,
    /// delta, or stop. Returns `None` for any other event (system, assistant,
    /// result, message-level stream events, etc).
    ///
    /// The CLI wraps each raw streaming event as
    /// `{"type":"stream_event","event":{...}}`; this accessor unwraps that
    /// envelope. Unknown block types and unknown delta types fall through to
    /// [`BlockType::Other`] / [`BlockDelta::Other`] rather than erroring, so
    /// future content-block kinds remain accessible (just untyped).
    ///
    /// # Example
    ///
    /// Pull incremental thinking text out of a partial-message event:
    ///
    /// ```
    /// use claude_wrapper::streaming::{BlockDelta, PartialMessageEvent, StreamEvent};
    /// use serde_json::json;
    ///
    /// let event: StreamEvent = serde_json::from_value(json!({
    ///     "type": "stream_event",
    ///     "event": {
    ///         "type": "content_block_delta",
    ///         "index": 0,
    ///         "delta": { "type": "thinking_delta", "thinking": "Let me think..." }
    ///     },
    ///     "session_id": "abc"
    /// })).unwrap();
    ///
    /// match event.partial_message() {
    ///     Some(PartialMessageEvent::BlockDelta { delta: BlockDelta::Thinking(t), .. }) => {
    ///         assert_eq!(t, "Let me think...");
    ///     }
    ///     _ => unreachable!(),
    /// }
    /// ```
    ///
    /// [`QueryCommand::include_partial_messages`]: crate::QueryCommand::include_partial_messages
    pub fn partial_message(&self) -> Option<PartialMessageEvent> {
        let event = if self.event_type() == Some("stream_event") {
            self.data.get("event")?
        } else {
            &self.data
        };

        let inner_type = event.get("type")?.as_str()?;
        let index = event.get("index").and_then(serde_json::Value::as_u64)?;
        let index = u32::try_from(index).ok()?;

        match inner_type {
            "content_block_start" => {
                let block_type = parse_block_type(event.get("content_block")?);
                Some(PartialMessageEvent::BlockStart { index, block_type })
            }
            "content_block_delta" => {
                let delta = parse_block_delta(event.get("delta")?);
                Some(PartialMessageEvent::BlockDelta { index, delta })
            }
            "content_block_stop" => Some(PartialMessageEvent::BlockStop { index }),
            _ => None,
        }
    }
}

/// A decoded partial-message event from a streaming `claude` call.
///
/// Surfaced by [`StreamEvent::partial_message`] when `--include-partial-messages`
/// is set. The three variants correspond to the Anthropic streaming content-block
/// lifecycle: a block starts, gets one or more deltas, then stops.
#[cfg(feature = "json")]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PartialMessageEvent {
    /// A new content block is starting. `block_type` says what kind.
    BlockStart {
        /// Position of this block within the assistant message.
        index: u32,
        /// What kind of block is starting (text, thinking, tool use, ...).
        block_type: BlockType,
    },
    /// Incremental content for an in-progress block.
    BlockDelta {
        /// Index of the block this delta applies to (matches a prior [`BlockStart`]).
        ///
        /// [`BlockStart`]: PartialMessageEvent::BlockStart
        index: u32,
        /// The incremental payload.
        delta: BlockDelta,
    },
    /// The block at `index` is complete.
    BlockStop {
        /// Index of the block that just finished.
        index: u32,
    },
}

/// The kind of content block reported by a [`PartialMessageEvent::BlockStart`].
///
/// Mirrors the `content_block.type` field from the Anthropic streaming API.
/// New block kinds added upstream surface as [`BlockType::Other`] -- callers
/// can still recover the type name from the carried string.
#[cfg(feature = "json")]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BlockType {
    /// Regular assistant text -- followed by `text_delta` deltas.
    Text,
    /// Extended-thinking block -- followed by `thinking_delta` deltas.
    Thinking,
    /// A tool invocation -- followed by `input_json_delta` deltas streaming the JSON input.
    ToolUse {
        /// Tool-call id, used to correlate the eventual tool result.
        id: String,
        /// Name of the tool being called.
        name: String,
    },
    /// Any block type not yet modelled. Carries the raw `type` string.
    Other(String),
}

/// The incremental payload carried by a [`PartialMessageEvent::BlockDelta`].
///
/// Mirrors the `delta.type` field from the Anthropic streaming API.
/// Less-common delta kinds (signature, citations, compaction, ...) collapse to
/// [`BlockDelta::Other`]; callers that need them can fall back to
/// [`StreamEvent::data`].
#[cfg(feature = "json")]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BlockDelta {
    /// Chunk of assistant text.
    Text(String),
    /// Chunk of extended-thinking text.
    Thinking(String),
    /// Chunk of streaming tool-input JSON. Concatenate across deltas to
    /// reconstruct the full input -- individual chunks are not standalone JSON.
    InputJson(String),
    /// Any delta type not modelled above (e.g. `signature_delta`,
    /// `citations_delta`). Read from [`StreamEvent::data`] for the raw payload.
    Other,
}

#[cfg(feature = "json")]
fn parse_block_type(content_block: &serde_json::Value) -> BlockType {
    let Some(ty) = content_block
        .get("type")
        .and_then(serde_json::Value::as_str)
    else {
        return BlockType::Other(String::new());
    };
    match ty {
        "text" => BlockType::Text,
        "thinking" => BlockType::Thinking,
        "tool_use" => {
            let id = content_block
                .get("id")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("")
                .to_string();
            let name = content_block
                .get("name")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("")
                .to_string();
            BlockType::ToolUse { id, name }
        }
        other => BlockType::Other(other.to_string()),
    }
}

#[cfg(feature = "json")]
fn parse_block_delta(delta: &serde_json::Value) -> BlockDelta {
    let Some(ty) = delta.get("type").and_then(serde_json::Value::as_str) else {
        return BlockDelta::Other;
    };
    match ty {
        "text_delta" => delta
            .get("text")
            .and_then(serde_json::Value::as_str)
            .map(|s| BlockDelta::Text(s.to_string()))
            .unwrap_or(BlockDelta::Other),
        "thinking_delta" => delta
            .get("thinking")
            .and_then(serde_json::Value::as_str)
            .map(|s| BlockDelta::Thinking(s.to_string()))
            .unwrap_or(BlockDelta::Other),
        "input_json_delta" => delta
            .get("partial_json")
            .and_then(serde_json::Value::as_str)
            .map(|s| BlockDelta::InputJson(s.to_string()))
            .unwrap_or(BlockDelta::Other),
        _ => BlockDelta::Other,
    }
}

/// Execute a command with streaming output, calling a handler for each NDJSON line.
///
/// This spawns the claude process and reads stdout line-by-line, parsing each
/// as a JSON event and passing it to the handler. Useful for progress tracking
/// and real-time output processing.
///
/// Dropping the returned future mid-flight kills the spawned `claude`
/// process and, on Unix, its whole process group (SIGKILL): an
/// abandoned run does not keep executing in the background, and the
/// subprocesses it spawned for tool use die with it. Events already
/// dispatched to the handler are not rolled back.
///
/// # Example
///
/// ```no_run
/// use claude_wrapper::{Claude, QueryCommand, OutputFormat};
/// use claude_wrapper::streaming::{StreamEvent, stream_query};
///
/// # async fn example() -> claude_wrapper::Result<()> {
/// let claude = Claude::builder().build()?;
///
/// let cmd = QueryCommand::new("explain quicksort")
///     .output_format(OutputFormat::StreamJson);
///
/// let output = stream_query(&claude, &cmd, |event: StreamEvent| {
///     if let Some(t) = event.event_type() {
///         println!("[{t}] {:?}", event.data);
///     }
/// }).await?;
/// # Ok(())
/// # }
/// ```
#[cfg(all(feature = "json", feature = "async"))]
pub async fn stream_query<F>(
    claude: &Claude,
    cmd: &crate::command::query::QueryCommand,
    handler: F,
) -> Result<CommandOutput>
where
    F: FnMut(StreamEvent),
{
    stream_query_impl(claude, cmd, handler, claude.timeout).await
}

/// Unified streaming implementation with optional timeout.
///
/// Reads stderr concurrently in a background task so a chatty child
/// cannot deadlock by filling the stderr pipe buffer, and so any
/// captured stderr is available even on timeout or IO error.
///
/// On timeout, the child is killed and reaped (`kill().await` sends
/// SIGKILL and waits), and whatever stderr was produced is logged at
/// warn level. The returned `Error::Timeout` does not carry partial
/// output -- streamed stdout events were already dispatched to the
/// handler as they arrived.
#[cfg(all(feature = "json", feature = "async"))]
async fn stream_query_impl<F>(
    claude: &Claude,
    cmd: &crate::command::query::QueryCommand,
    mut handler: F,
    timeout: Option<Duration>,
) -> Result<CommandOutput>
where
    F: FnMut(StreamEvent),
{
    use crate::command::ClaudeCommand;

    let args = cmd.args();

    let mut command_args = Vec::new();
    command_args.extend(claude.global_args.clone());
    command_args.extend(args);

    // A stream has three outcomes and the failure one is the easy one
    // to miss, so `outcome` is recorded on every exit path: completed,
    // failed, or timeout. `events` counts what the handler actually
    // saw, which is the difference between "produced nothing" and
    // "never started".
    let span = tracing::debug_span!(
        "claude.stream",
        command = crate::exec::span_command(&command_args),
        binary = %claude.binary.display(),
        cwd = claude.working_dir.as_deref().map(|d| d.display().to_string()),
        timeout_secs = timeout.map(|t| t.as_secs()),
        outcome = tracing::field::Empty,
        events = tracing::field::Empty,
        exit_code = tracing::field::Empty,
        duration_ms = tracing::field::Empty,
    );
    let _enter = span.enter();
    let started = std::time::Instant::now();
    let mut event_count: u64 = 0;

    debug!(
        binary = %claude.binary.display(),
        args = ?command_args,
        timeout = ?timeout,
        "streaming claude command"
    );

    let mut cmd = Command::new(&claude.binary);
    cmd.args(&command_args)
        .env_remove("CLAUDECODE")
        .envs(&claude.env)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .stdin(std::process::Stdio::null())
        // Dropping the in-flight future must kill the child, not leave
        // the CLI running unattended (see the `exec` module docs).
        .kill_on_drop(true);
    // Own process group (Unix) so cancellation can signal the whole
    // tree, not just the direct child (see exec::GroupKillGuard). Opt
    // out via ClaudeBuilder::process_group.
    crate::exec::apply_process_group(&mut cmd, claude.process_group);

    if let Some(ref dir) = claude.working_dir {
        cmd.current_dir(dir);
    }

    let mut child = cmd.spawn().map_err(|e| Error::Io {
        message: format!("failed to spawn claude: {e}"),
        source: e,
        working_dir: claude.working_dir.clone(),
    })?;
    let mut group =
        crate::exec::arm_and_notify(claude.process_group, child.id(), claude.on_spawn.as_ref());

    let stdout = child.stdout.take().expect("stdout was piped");
    let mut stderr = child.stderr.take().expect("stderr was piped");

    let mut reader = BufReader::new(stdout).lines();

    // Run stdout line reading and stderr draining concurrently so a
    // chatty child can't deadlock by filling the stderr pipe buffer.
    // tokio::join! polls both futures on the same task (no tokio::spawn
    // needed, so we avoid pulling in the `rt` feature).
    let drain = drain_stderr(&mut stderr);
    // Wrap the caller's handler so the span can report how many events
    // were dispatched without the handler needing to care.
    let mut counting_handler = |event: StreamEvent| {
        event_count += 1;
        handler(event);
    };
    let read_future = read_lines(
        &mut reader,
        &mut counting_handler,
        claude.working_dir.clone(),
    );
    let combined = async {
        let (line_result, stderr_str) = tokio::join!(read_future, drain);
        (line_result, stderr_str)
    };

    let (line_result, stderr_str) = match timeout {
        Some(d) => match tokio::time::timeout(d, combined).await {
            Ok(pair) => pair,
            Err(_) => {
                // Timeout: take down the whole group, honoring the
                // optional SIGTERM grace, then kill+reap the direct
                // child, and try to drain whatever stderr remains.
                // The group kill takes down subprocesses that could
                // otherwise hold our pipe fds open; the capped drain
                // below stays as a backstop.
                crate::exec::kill_group_with_grace(&mut group, claude.kill_grace).await;
                let _ = child.kill().await;
                let drain_budget = Duration::from_millis(200);
                let stderr_str = tokio::time::timeout(drain_budget, drain_stderr(&mut stderr))
                    .await
                    .unwrap_or_default();
                if !stderr_str.is_empty() {
                    warn!(stderr = %stderr_str, "stderr from timed-out streaming process");
                }
                span.record("outcome", "timeout");
                span.record("events", event_count);
                span.record("duration_ms", started.elapsed().as_millis() as u64);
                return Err(Error::Timeout {
                    timeout_seconds: d.as_secs(),
                });
            }
        },
        None => combined.await,
    };

    // If reading lines failed partway through (IO error, not timeout),
    // clean up the child (and its group) before returning.
    let stdout_diagnostics = match line_result {
        Ok(diagnostics) => diagnostics.into_string(),
        Err(e) => {
            group.kill_now();
            let _ = child.kill().await;
            return Err(e);
        }
    };

    let status = child.wait().await.map_err(|e| Error::Io {
        message: "failed to wait for claude process".to_string(),
        source: e,
        working_dir: claude.working_dir.clone(),
    })?;
    group.disarm();

    let exit_code = status.code().unwrap_or(-1);

    span.record("events", event_count);
    span.record("exit_code", exit_code);
    span.record("duration_ms", started.elapsed().as_millis() as u64);

    if !status.success() {
        span.record("outcome", "failed");
        return Err(Error::from_command_failure(
            format!("{} {}", claude.binary.display(), command_args.join(" ")),
            exit_code,
            stdout_diagnostics,
            stderr_str,
            claude.working_dir.clone(),
        ));
    }

    span.record("outcome", "completed");
    Ok(CommandOutput {
        stdout: String::new(), // already consumed via streaming
        stderr: stderr_str,
        exit_code,
        success: true,
    })
}

#[cfg(all(feature = "json", feature = "async"))]
async fn drain_stderr(stderr: &mut ChildStderr) -> String {
    let mut buf = Vec::new();
    let _ = stderr.read_to_end(&mut buf).await;
    String::from_utf8_lossy(&buf).into_owned()
}

#[cfg(all(feature = "json", feature = "async"))]
async fn read_lines<F>(
    reader: &mut tokio::io::Lines<BufReader<tokio::process::ChildStdout>>,
    handler: &mut F,
    working_dir: Option<std::path::PathBuf>,
) -> Result<ParseFailureDiagnostics>
where
    F: FnMut(StreamEvent),
{
    let mut diagnostics = ParseFailureDiagnostics::default();
    while let Some(line) = reader.next_line().await.map_err(|e| Error::Io {
        message: "failed to read stdout line".to_string(),
        source: e,
        working_dir: working_dir.clone(),
    })? {
        if line.trim().is_empty() {
            continue;
        }
        match serde_json::from_str::<StreamEvent>(&line) {
            Ok(event) => handler(event),
            Err(e) => {
                debug!(line = %line, error = %e, "failed to parse stream event, skipping");
                diagnostics.push(&line);
            }
        }
    }

    Ok(diagnostics)
}

// ---------- sync streaming ----------

/// Blocking mirror of [`stream_query`]. Reads NDJSON lines from the
/// child's stdout on a worker thread, dispatches each parsed event
/// to `handler` on the caller's thread, and drains stderr on a
/// separate worker thread so the child can't deadlock on a full pipe.
///
/// Requires both `sync` and `json` features.
///
/// The handler is invoked on the caller's thread — no `Send` bound —
/// so it can capture non-`Send` state. If a timeout is configured on
/// the [`Claude`] client, the child's whole process group (Unix) is
/// SIGKILLed and the child reaped once the deadline passes; partial
/// events already dispatched to the handler are not rolled back.
///
/// # Example
///
/// ```no_run
/// # #[cfg(all(feature = "sync", feature = "json"))]
/// # {
/// use claude_wrapper::{Claude, OutputFormat, QueryCommand};
/// use claude_wrapper::streaming::{StreamEvent, stream_query_sync};
///
/// # fn example() -> claude_wrapper::Result<()> {
/// let claude = Claude::builder().build()?;
/// let cmd = QueryCommand::new("explain quicksort")
///     .output_format(OutputFormat::StreamJson);
///
/// stream_query_sync(&claude, &cmd, |event: StreamEvent| {
///     if let Some(t) = event.event_type() {
///         println!("[{t}] {:?}", event.data);
///     }
/// })?;
/// # Ok(())
/// # }
/// # }
/// ```
#[cfg(all(feature = "sync", feature = "json"))]
pub fn stream_query_sync<F>(
    claude: &Claude,
    cmd: &crate::command::query::QueryCommand,
    mut handler: F,
) -> Result<CommandOutput>
where
    F: FnMut(StreamEvent),
{
    use std::io::{BufRead as _, Read as _};
    use std::process::{Command as StdCommand, Stdio};
    use std::sync::mpsc;
    use std::thread;
    use std::time::Instant;

    use crate::command::ClaudeCommand;

    let args = cmd.args();
    let mut command_args = Vec::new();
    command_args.extend(claude.global_args.clone());
    command_args.extend(args);

    debug!(
        binary = %claude.binary.display(),
        args = ?command_args,
        timeout = ?claude.timeout,
        "streaming claude command (sync)"
    );

    let mut cmd_builder = StdCommand::new(&claude.binary);
    cmd_builder
        .args(&command_args)
        .env_remove("CLAUDECODE")
        .env_remove("CLAUDE_CODE_ENTRYPOINT")
        .envs(&claude.env)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    // Own process group (Unix) so a kill can signal the whole tree,
    // not just the direct child (see exec::GroupKillGuard). Opt out
    // via ClaudeBuilder::process_group.
    crate::exec::apply_process_group_sync(&mut cmd_builder, claude.process_group);

    if let Some(ref dir) = claude.working_dir {
        cmd_builder.current_dir(dir);
    }

    let mut child = cmd_builder.spawn().map_err(|e| Error::Io {
        message: format!("failed to spawn claude: {e}"),
        source: e,
        working_dir: claude.working_dir.clone(),
    })?;
    let mut group = crate::exec::arm_and_notify(
        claude.process_group,
        Some(child.id()),
        claude.on_spawn.as_ref(),
    );

    let stdout = child.stdout.take().expect("stdout was piped");
    let stderr = child.stderr.take().expect("stderr was piped");

    // Reader thread: parse NDJSON lines and push StreamEvents through
    // the channel. Handler runs on the caller's thread so it doesn't
    // need Send. Bubbles IO errors out via the thread's return value.
    let (tx, rx) = mpsc::channel::<StreamEvent>();
    let reader_wd = claude.working_dir.clone();
    let reader_thread = thread::spawn(move || -> Result<ParseFailureDiagnostics> {
        let reader = std::io::BufReader::new(stdout);
        let mut diagnostics = ParseFailureDiagnostics::default();
        for line_res in reader.lines() {
            let line = line_res.map_err(|e| Error::Io {
                message: "failed to read stdout line".to_string(),
                source: e,
                working_dir: reader_wd.clone(),
            })?;
            if line.trim().is_empty() {
                continue;
            }
            match serde_json::from_str::<StreamEvent>(&line) {
                Ok(event) => {
                    if tx.send(event).is_err() {
                        // Receiver gone — main thread has bailed out.
                        return Ok(diagnostics);
                    }
                }
                Err(e) => {
                    debug!(line = %line, error = %e, "failed to parse stream event, skipping");
                    diagnostics.push(&line);
                }
            }
        }
        Ok(diagnostics)
    });

    let stderr_thread = thread::spawn(move || -> String {
        let mut buf = Vec::new();
        let mut stderr = stderr;
        let _ = stderr.read_to_end(&mut buf);
        String::from_utf8_lossy(&buf).into_owned()
    });

    // Main loop: dispatch events on the caller's thread, honouring the
    // configured timeout. Break on disconnect (reader done) or timeout.
    let deadline = claude.timeout.map(|d| Instant::now() + d);
    let mut timed_out = false;

    loop {
        let recv_result = match deadline {
            Some(d) => {
                let now = Instant::now();
                if now >= d {
                    timed_out = true;
                    break;
                }
                rx.recv_timeout(d - now)
            }
            None => rx.recv().map_err(|_| mpsc::RecvTimeoutError::Disconnected),
        };

        match recv_result {
            Ok(event) => handler(event),
            Err(mpsc::RecvTimeoutError::Timeout) => {
                timed_out = true;
                break;
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        }
    }

    if timed_out {
        // Take down the whole group first, honoring the optional
        // SIGTERM grace, so grandchildren holding our pipe fds die
        // too, then kill+reap the direct child.
        crate::exec::kill_group_with_grace_sync(&mut group, claude.kill_grace);
        let _ = child.kill();
        let _ = child.wait();
        // Both worker threads can block indefinitely if an orphaned
        // grandchild inherited our pipe fds and keeps the write end
        // open (e.g. a `bash` script whose `sleep` subprocess outlives
        // the SIGKILLed shell). Cap the joins so the timeout error
        // still returns promptly; any thread that misses the deadline
        // leaks its JoinHandle, which is acceptable for this edge.
        let budget = Duration::from_millis(200);
        let stderr_str = join_with_budget(stderr_thread, budget).unwrap_or_default();
        let _ = join_with_budget(reader_thread, budget);
        if !stderr_str.is_empty() {
            warn!(stderr = %stderr_str, "stderr from timed-out streaming process");
        }
        return Err(Error::Timeout {
            timeout_seconds: claude.timeout.map(|d| d.as_secs()).unwrap_or_default(),
        });
    }

    // Normal completion: collect reader result (may carry IO error).
    let reader_result = reader_thread
        .join()
        .unwrap_or_else(|_| Ok(ParseFailureDiagnostics::default()));
    let stdout_diagnostics = match reader_result {
        Ok(diagnostics) => diagnostics.into_string(),
        Err(e) => {
            group.kill_now();
            let _ = child.kill();
            let _ = child.wait();
            let _ = stderr_thread.join();
            return Err(e);
        }
    };

    let status = child.wait().map_err(|e| Error::Io {
        message: "failed to wait for claude process".to_string(),
        source: e,
        working_dir: claude.working_dir.clone(),
    })?;
    group.disarm();
    let stderr_str = stderr_thread.join().unwrap_or_default();
    let exit_code = status.code().unwrap_or(-1);

    if !status.success() {
        return Err(Error::from_command_failure(
            format!("{} {}", claude.binary.display(), command_args.join(" ")),
            exit_code,
            stdout_diagnostics,
            stderr_str,
            claude.working_dir.clone(),
        ));
    }

    Ok(CommandOutput {
        stdout: String::new(),
        stderr: stderr_str,
        exit_code,
        success: true,
    })
}

/// Join a worker thread with a time budget. Returns `Some(value)` if
/// the thread finished in time, `None` if the deadline passed first.
/// A missed deadline leaks the `JoinHandle`; the thread completes
/// eventually and its value is dropped.
#[cfg(all(feature = "sync", feature = "json"))]
fn join_with_budget<T: Send + 'static>(
    handle: std::thread::JoinHandle<T>,
    budget: Duration,
) -> Option<T> {
    use std::sync::mpsc;
    use std::thread;

    let (tx, rx) = mpsc::channel::<T>();
    thread::spawn(move || {
        if let Ok(v) = handle.join() {
            let _ = tx.send(v);
        }
    });
    rx.recv_timeout(budget).ok()
}

#[cfg(all(test, feature = "json"))]
mod tests {
    use super::*;
    use serde_json::json;

    fn parse(v: serde_json::Value) -> StreamEvent {
        serde_json::from_value(v).expect("valid StreamEvent")
    }

    fn wrap(inner: serde_json::Value) -> StreamEvent {
        parse(json!({
            "type": "stream_event",
            "event": inner,
            "session_id": "sess-1",
            "parent_tool_use_id": null,
            "uuid": "11111111-1111-1111-1111-111111111111"
        }))
    }

    #[test]
    fn parse_failure_diagnostics_are_bounded_and_keep_recent_lines() {
        let mut diagnostics = ParseFailureDiagnostics::default();
        diagnostics.push(&"x".repeat(STREAM_DIAGNOSTIC_MAX_BYTES));
        diagnostics.push("Not authenticated. Run `claude login`.");

        let output = diagnostics.into_string();
        assert!(output.len() <= STREAM_DIAGNOSTIC_MAX_BYTES);
        assert_eq!(output, "Not authenticated. Run `claude login`.");
    }

    #[test]
    fn partial_message_text_block_lifecycle() {
        let start = wrap(json!({
            "type": "content_block_start",
            "index": 0,
            "content_block": { "type": "text", "text": "" }
        }));
        assert_eq!(
            start.partial_message(),
            Some(PartialMessageEvent::BlockStart {
                index: 0,
                block_type: BlockType::Text,
            })
        );

        let delta = wrap(json!({
            "type": "content_block_delta",
            "index": 0,
            "delta": { "type": "text_delta", "text": "Hello" }
        }));
        assert_eq!(
            delta.partial_message(),
            Some(PartialMessageEvent::BlockDelta {
                index: 0,
                delta: BlockDelta::Text("Hello".into()),
            })
        );

        let stop = wrap(json!({ "type": "content_block_stop", "index": 0 }));
        assert_eq!(
            stop.partial_message(),
            Some(PartialMessageEvent::BlockStop { index: 0 })
        );
    }

    #[test]
    fn partial_message_thinking_block_lifecycle() {
        let start = wrap(json!({
            "type": "content_block_start",
            "index": 1,
            "content_block": { "type": "thinking", "thinking": "", "signature": "" }
        }));
        assert_eq!(
            start.partial_message(),
            Some(PartialMessageEvent::BlockStart {
                index: 1,
                block_type: BlockType::Thinking,
            })
        );

        let delta = wrap(json!({
            "type": "content_block_delta",
            "index": 1,
            "delta": { "type": "thinking_delta", "thinking": "weighing options" }
        }));
        assert_eq!(
            delta.partial_message(),
            Some(PartialMessageEvent::BlockDelta {
                index: 1,
                delta: BlockDelta::Thinking("weighing options".into()),
            })
        );

        let stop = wrap(json!({ "type": "content_block_stop", "index": 1 }));
        assert_eq!(
            stop.partial_message(),
            Some(PartialMessageEvent::BlockStop { index: 1 })
        );
    }

    #[test]
    fn partial_message_tool_use_block_carries_id_and_name() {
        let start = wrap(json!({
            "type": "content_block_start",
            "index": 2,
            "content_block": {
                "type": "tool_use",
                "id": "toolu_abc",
                "name": "Bash",
                "input": {}
            }
        }));
        assert_eq!(
            start.partial_message(),
            Some(PartialMessageEvent::BlockStart {
                index: 2,
                block_type: BlockType::ToolUse {
                    id: "toolu_abc".into(),
                    name: "Bash".into(),
                },
            })
        );

        let delta = wrap(json!({
            "type": "content_block_delta",
            "index": 2,
            "delta": { "type": "input_json_delta", "partial_json": "{\"cmd\":" }
        }));
        assert_eq!(
            delta.partial_message(),
            Some(PartialMessageEvent::BlockDelta {
                index: 2,
                delta: BlockDelta::InputJson("{\"cmd\":".into()),
            })
        );
    }

    #[test]
    fn partial_message_unknown_kinds_fall_through_to_other() {
        let unknown_block = wrap(json!({
            "type": "content_block_start",
            "index": 3,
            "content_block": { "type": "redacted_thinking", "data": "..." }
        }));
        assert_eq!(
            unknown_block.partial_message(),
            Some(PartialMessageEvent::BlockStart {
                index: 3,
                block_type: BlockType::Other("redacted_thinking".into()),
            })
        );

        let unknown_delta = wrap(json!({
            "type": "content_block_delta",
            "index": 3,
            "delta": { "type": "signature_delta", "signature": "sig" }
        }));
        assert_eq!(
            unknown_delta.partial_message(),
            Some(PartialMessageEvent::BlockDelta {
                index: 3,
                delta: BlockDelta::Other,
            })
        );
    }

    #[test]
    fn partial_message_returns_none_for_non_partial_events() {
        let result = parse(json!({
            "type": "result",
            "result": "done",
            "session_id": "sess-1",
            "total_cost_usd": 0.01
        }));
        assert!(result.partial_message().is_none());

        let assistant = parse(json!({
            "type": "assistant",
            "message": { "role": "assistant", "content": [] },
            "session_id": "sess-1"
        }));
        assert!(assistant.partial_message().is_none());

        let message_start = wrap(json!({
            "type": "message_start",
            "message": { "id": "msg_1", "role": "assistant", "content": [] }
        }));
        assert!(message_start.partial_message().is_none());
    }

    #[test]
    fn partial_message_accepts_unwrapped_event() {
        let raw = parse(json!({
            "type": "content_block_delta",
            "index": 0,
            "delta": { "type": "text_delta", "text": "hi" }
        }));
        assert_eq!(
            raw.partial_message(),
            Some(PartialMessageEvent::BlockDelta {
                index: 0,
                delta: BlockDelta::Text("hi".into()),
            })
        );
    }
}
