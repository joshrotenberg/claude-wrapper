use std::time::Duration;

use tokio::io::{AsyncBufReadExt, AsyncReadExt, BufReader};
use tokio::process::{ChildStderr, Command};
use tracing::{debug, warn};

use crate::Claude;
use crate::error::{Error, Result};
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
    pub fn cost_usd(&self) -> Option<f64> {
        self.data.get("cost_usd").and_then(|v| v.as_f64())
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
#[cfg(feature = "json")]
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
#[cfg(feature = "json")]
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

#[cfg(feature = "json")]
async fn drain_stderr(stderr: &mut ChildStderr) -> String {
    let mut buf = Vec::new();
    let _ = stderr.read_to_end(&mut buf).await;
    String::from_utf8_lossy(&buf).into_owned()
}

#[cfg(feature = "json")]
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
