use std::time::Duration;

use tokio::io::AsyncReadExt;
use tokio::process::Command;
use tracing::{debug, warn};

use crate::Claude;
use crate::error::{Error, Result};

/// Raw output from a claude CLI invocation.
#[derive(Debug, Clone)]
pub struct CommandOutput {
    pub stdout: String,
    pub stderr: String,
    pub exit_code: i32,
    pub success: bool,
}

/// Run a claude command with the given arguments.
///
/// If the [`Claude`] client has a retry policy set, transient errors will be
/// retried according to that policy. A per-command retry policy can be passed
/// to override the client default.
pub async fn run_claude(claude: &Claude, args: Vec<String>) -> Result<CommandOutput> {
    run_claude_with_retry(claude, args, None).await
}

/// Run a claude command with an optional per-command retry policy override.
pub async fn run_claude_with_retry(
    claude: &Claude,
    args: Vec<String>,
    retry_override: Option<&crate::retry::RetryPolicy>,
) -> Result<CommandOutput> {
    let policy = retry_override.or(claude.retry_policy.as_ref());

    match policy {
        Some(policy) => {
            crate::retry::with_retry(policy, || run_claude_once(claude, args.clone())).await
        }
        None => run_claude_once(claude, args).await,
    }
}

async fn run_claude_once(claude: &Claude, args: Vec<String>) -> Result<CommandOutput> {
    let mut command_args = Vec::new();

    // Global args first (before subcommand)
    command_args.extend(claude.global_args.clone());

    // Then command-specific args
    command_args.extend(args);

    debug!(binary = %claude.binary.display(), args = ?command_args, "executing claude command");

    let output = if let Some(timeout) = claude.timeout {
        run_with_timeout(
            &claude.binary,
            &command_args,
            &claude.env,
            claude.working_dir.as_deref(),
            timeout,
        )
        .await?
    } else {
        run_internal(
            &claude.binary,
            &command_args,
            &claude.env,
            claude.working_dir.as_deref(),
        )
        .await?
    };

    Ok(output)
}

/// Run a claude command and allow specific non-zero exit codes.
pub async fn run_claude_allow_exit_codes(
    claude: &Claude,
    args: Vec<String>,
    allowed_codes: &[i32],
) -> Result<CommandOutput> {
    let output = run_claude(claude, args).await;

    match output {
        Err(Error::CommandFailed {
            exit_code,
            stdout,
            stderr,
            ..
        }) if allowed_codes.contains(&exit_code) => Ok(CommandOutput {
            stdout,
            stderr,
            exit_code,
            success: false,
        }),
        other => other,
    }
}

async fn run_internal(
    binary: &std::path::Path,
    args: &[String],
    env: &std::collections::HashMap<String, String>,
    working_dir: Option<&std::path::Path>,
) -> Result<CommandOutput> {
    let mut cmd = Command::new(binary);
    cmd.args(args);

    // Prevent child from inheriting/blocking on parent's stdin.
    cmd.stdin(std::process::Stdio::null());

    // Remove Claude Code env vars to prevent nested session detection
    cmd.env_remove("CLAUDECODE");
    cmd.env_remove("CLAUDE_CODE_ENTRYPOINT");

    if let Some(dir) = working_dir {
        cmd.current_dir(dir);
    }

    for (key, value) in env {
        cmd.env(key, value);
    }

    let output = cmd.output().await.map_err(|e| Error::Io {
        message: format!("failed to spawn claude: {e}"),
        source: e,
        working_dir: working_dir.map(|p| p.to_path_buf()),
    })?;

    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
    let exit_code = output.status.code().unwrap_or(-1);

    if !output.status.success() {
        return Err(Error::CommandFailed {
            command: format!("{} {}", binary.display(), args.join(" ")),
            exit_code,
            stdout,
            stderr,
            working_dir: working_dir.map(|p| p.to_path_buf()),
        });
    }

    Ok(CommandOutput {
        stdout,
        stderr,
        exit_code,
        success: true,
    })
}

/// Run a command with a timeout, killing and reaping the child on expiration.
///
/// Spawns the child explicitly (rather than wrapping `Command::output()` in a
/// `tokio::time::timeout`) so that we retain the handle and can SIGKILL the
/// child and wait for it when the timeout fires. Stdout and stderr are drained
/// concurrently with `child.wait()` via `tokio::join!` so neither pipe buffer
/// can fill up and deadlock the child.
///
/// On timeout, partial stdout/stderr captured before the kill is logged at
/// warn level; the returned `Error::Timeout` itself does not carry the
/// partial output.
async fn run_with_timeout(
    binary: &std::path::Path,
    args: &[String],
    env: &std::collections::HashMap<String, String>,
    working_dir: Option<&std::path::Path>,
    timeout: Duration,
) -> Result<CommandOutput> {
    let mut cmd = Command::new(binary);
    cmd.args(args);
    cmd.stdin(std::process::Stdio::null());
    cmd.stdout(std::process::Stdio::piped());
    cmd.stderr(std::process::Stdio::piped());
    cmd.env_remove("CLAUDECODE");
    cmd.env_remove("CLAUDE_CODE_ENTRYPOINT");

    if let Some(dir) = working_dir {
        cmd.current_dir(dir);
    }

    for (key, value) in env {
        cmd.env(key, value);
    }

    let mut child = cmd.spawn().map_err(|e| Error::Io {
        message: format!("failed to spawn claude: {e}"),
        source: e,
        working_dir: working_dir.map(|p| p.to_path_buf()),
    })?;

    let mut stdout = child.stdout.take().expect("stdout was piped");
    let mut stderr = child.stderr.take().expect("stderr was piped");

    // Drain stdout and stderr concurrently with the process wait so
    // neither pipe buffer can fill up and deadlock the child.
    // tokio::join! polls all three on the same task; no tokio::spawn
    // (and therefore no `rt` feature) required.
    let wait_and_drain = async {
        let (status, stdout_str, stderr_str) =
            tokio::join!(child.wait(), drain(&mut stdout), drain(&mut stderr));
        (status, stdout_str, stderr_str)
    };

    match tokio::time::timeout(timeout, wait_and_drain).await {
        Ok((Ok(status), stdout, stderr)) => {
            let exit_code = status.code().unwrap_or(-1);

            if !status.success() {
                return Err(Error::CommandFailed {
                    command: format!("{} {}", binary.display(), args.join(" ")),
                    exit_code,
                    stdout,
                    stderr,
                    working_dir: working_dir.map(|p| p.to_path_buf()),
                });
            }

            Ok(CommandOutput {
                stdout,
                stderr,
                exit_code,
                success: true,
            })
        }
        Ok((Err(e), _stdout, _stderr)) => Err(Error::Io {
            message: "failed to wait for claude process".to_string(),
            source: e,
            working_dir: working_dir.map(|p| p.to_path_buf()),
        }),
        Err(_) => {
            // Timeout: kill the child (reaps via start_kill + wait).
            // Note that kill() only targets the direct child; if it has
            // spawned its own subprocesses that are holding our pipe
            // fds open, draining would block. Cap the drain with a
            // short deadline so the timeout error returns promptly.
            let _ = child.kill().await;
            let drain_budget = Duration::from_millis(200);
            let stdout_str = tokio::time::timeout(drain_budget, drain(&mut stdout))
                .await
                .unwrap_or_default();
            let stderr_str = tokio::time::timeout(drain_budget, drain(&mut stderr))
                .await
                .unwrap_or_default();
            if !stdout_str.is_empty() || !stderr_str.is_empty() {
                warn!(
                    stdout = %stdout_str,
                    stderr = %stderr_str,
                    "partial output from timed-out process",
                );
            }
            Err(Error::Timeout {
                timeout_seconds: timeout.as_secs(),
            })
        }
    }
}

async fn drain<R: AsyncReadExt + Unpin>(reader: &mut R) -> String {
    let mut buf = Vec::new();
    let _ = reader.read_to_end(&mut buf).await;
    String::from_utf8_lossy(&buf).into_owned()
}
