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
}

/// Execute a command with streaming output, calling a handler for each NDJSON line.
///
/// This spawns the claude process and reads stdout line-by-line, parsing each
/// as a JSON event and passing it to the handler. Useful for progress tracking
/// and real-time output processing.
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
        .stdin(std::process::Stdio::null());

    if let Some(ref dir) = claude.working_dir {
        cmd.current_dir(dir);
    }

    let mut child = cmd.spawn().map_err(|e| Error::Io {
        message: format!("failed to spawn claude: {e}"),
        source: e,
        working_dir: claude.working_dir.clone(),
    })?;

    let stdout = child.stdout.take().expect("stdout was piped");
    let mut stderr = child.stderr.take().expect("stderr was piped");

    let mut reader = BufReader::new(stdout).lines();

    // Run stdout line reading and stderr draining concurrently so a
    // chatty child can't deadlock by filling the stderr pipe buffer.
    // tokio::join! polls both futures on the same task (no tokio::spawn
    // needed, so we avoid pulling in the `rt` feature).
    let drain = drain_stderr(&mut stderr);
    let read_future = read_lines(&mut reader, &mut handler, claude.working_dir.clone());
    let combined = async {
        let (line_result, stderr_str) = tokio::join!(read_future, drain);
        (line_result, stderr_str)
    };

    let (line_result, stderr_str) = match timeout {
        Some(d) => match tokio::time::timeout(d, combined).await {
            Ok(pair) => pair,
            Err(_) => {
                // Timeout: kill the child (reaps via start_kill + wait)
                // and try to drain whatever stderr remains. kill() only
                // targets the direct child, so a subprocess tree holding
                // our pipe fds could block the drain -- cap it with a
                // short deadline.
                let _ = child.kill().await;
                let drain_budget = Duration::from_millis(200);
                let stderr_str = tokio::time::timeout(drain_budget, drain_stderr(&mut stderr))
                    .await
                    .unwrap_or_default();
                if !stderr_str.is_empty() {
                    warn!(stderr = %stderr_str, "stderr from timed-out streaming process");
                }
                return Err(Error::Timeout {
                    timeout_seconds: d.as_secs(),
                });
            }
        },
        None => combined.await,
    };

    // If reading lines failed partway through (IO error, not timeout),
    // clean up the child before returning.
    if let Err(e) = line_result {
        let _ = child.kill().await;
        return Err(e);
    }

    let status = child.wait().await.map_err(|e| Error::Io {
        message: "failed to wait for claude process".to_string(),
        source: e,
        working_dir: claude.working_dir.clone(),
    })?;

    let exit_code = status.code().unwrap_or(-1);

    if !status.success() {
        return Err(Error::CommandFailed {
            command: format!("{} {}", claude.binary.display(), command_args.join(" ")),
            exit_code,
            stdout: String::new(),
            stderr: stderr_str,
            working_dir: claude.working_dir.clone(),
        });
    }

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
) -> Result<()>
where
    F: FnMut(StreamEvent),
{
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
            }
        }
    }

    Ok(())
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
/// the [`Claude`] client, the child is SIGKILLed and reaped once the
/// deadline passes; partial events already dispatched to the handler
/// are not rolled back.
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

    if let Some(ref dir) = claude.working_dir {
        cmd_builder.current_dir(dir);
    }

    let mut child = cmd_builder.spawn().map_err(|e| Error::Io {
        message: format!("failed to spawn claude: {e}"),
        source: e,
        working_dir: claude.working_dir.clone(),
    })?;

    let stdout = child.stdout.take().expect("stdout was piped");
    let stderr = child.stderr.take().expect("stderr was piped");

    // Reader thread: parse NDJSON lines and push StreamEvents through
    // the channel. Handler runs on the caller's thread so it doesn't
    // need Send. Bubbles IO errors out via the thread's return value.
    let (tx, rx) = mpsc::channel::<StreamEvent>();
    let reader_wd = claude.working_dir.clone();
    let reader_thread = thread::spawn(move || -> Result<()> {
        let reader = std::io::BufReader::new(stdout);
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
                        return Ok(());
                    }
                }
                Err(e) => {
                    debug!(line = %line, error = %e, "failed to parse stream event, skipping");
                }
            }
        }
        Ok(())
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
    let reader_result = reader_thread.join().unwrap_or(Ok(()));
    if let Err(e) = reader_result {
        let _ = child.kill();
        let _ = child.wait();
        let _ = stderr_thread.join();
        return Err(e);
    }

    let status = child.wait().map_err(|e| Error::Io {
        message: "failed to wait for claude process".to_string(),
        source: e,
        working_dir: claude.working_dir.clone(),
    })?;
    let stderr_str = stderr_thread.join().unwrap_or_default();
    let exit_code = status.code().unwrap_or(-1);

    if !status.success() {
        return Err(Error::CommandFailed {
            command: format!("{} {}", claude.binary.display(), command_args.join(" ")),
            exit_code,
            stdout: String::new(),
            stderr: stderr_str,
            working_dir: claude.working_dir.clone(),
        });
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
