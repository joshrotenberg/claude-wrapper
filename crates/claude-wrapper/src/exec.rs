use std::time::Duration;

#[cfg(feature = "async")]
use tokio::io::AsyncReadExt;
#[cfg(feature = "async")]
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
#[cfg(feature = "async")]
pub async fn run_claude(claude: &Claude, args: Vec<String>) -> Result<CommandOutput> {
    run_claude_with_retry(claude, args, None).await
}

/// Run a claude command with an optional per-command retry policy override.
#[cfg(feature = "async")]
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

/// Run claude, writing `stdin_content` to the child's stdin rather than
/// passing the prompt as argv.
///
/// stdin mode does not retry -- the stdin pipe is consumed after the first
/// attempt and cannot be rewound for a subsequent try.
#[cfg(feature = "async")]
pub async fn run_claude_with_stdin_prompt(
    claude: &Claude,
    args: Vec<String>,
    stdin_content: String,
) -> Result<CommandOutput> {
    run_claude_with_stdin_prompt_internal(claude, args, stdin_content).await
}

#[cfg(feature = "async")]
async fn run_claude_with_stdin_prompt_internal(
    claude: &Claude,
    args: Vec<String>,
    stdin_content: String,
) -> Result<CommandOutput> {
    let mut command_args = Vec::new();
    command_args.extend(claude.global_args.clone());
    command_args.extend(args);

    debug!(binary = %claude.binary.display(), args = ?command_args, "executing claude command (stdin prompt)");

    let binary = &claude.binary;
    let env = &claude.env;
    let working_dir = claude.working_dir.as_deref();

    if let Some(timeout) = claude.timeout {
        run_with_timeout_stdin(
            binary,
            &command_args,
            env,
            working_dir,
            timeout,
            stdin_content,
        )
        .await
    } else {
        run_internal_stdin(binary, &command_args, env, working_dir, stdin_content).await
    }
}

#[cfg(feature = "async")]
async fn run_internal_stdin(
    binary: &std::path::Path,
    args: &[String],
    env: &std::collections::HashMap<String, String>,
    working_dir: Option<&std::path::Path>,
    stdin_content: String,
) -> Result<CommandOutput> {
    use tokio::io::AsyncWriteExt;

    let mut cmd = Command::new(binary);
    cmd.args(args);
    cmd.stdin(std::process::Stdio::piped());
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

    // Write the prompt to stdin, then drop the handle so the child sees EOF.
    if let Some(mut stdin) = child.stdin.take() {
        stdin
            .write_all(stdin_content.as_bytes())
            .await
            .map_err(|e| Error::Io {
                message: format!("failed to write to claude stdin: {e}"),
                source: e,
                working_dir: working_dir.map(|p| p.to_path_buf()),
            })?;
        // Drop stdin so the child sees EOF.
    }

    let mut stdout_handle = child.stdout.take().expect("stdout was piped");
    let mut stderr_handle = child.stderr.take().expect("stderr was piped");

    let (status, stdout_str, stderr_str) = tokio::join!(
        child.wait(),
        drain(&mut stdout_handle),
        drain(&mut stderr_handle),
    );

    let status = status.map_err(|e| Error::Io {
        message: "failed to wait for claude process".to_string(),
        source: e,
        working_dir: working_dir.map(|p| p.to_path_buf()),
    })?;

    let exit_code = status.code().unwrap_or(-1);

    if !status.success() {
        return Err(Error::from_command_failure(
            format!("{} {}", binary.display(), args.join(" ")),
            exit_code,
            stdout_str,
            stderr_str,
            working_dir.map(|p| p.to_path_buf()),
        ));
    }

    Ok(CommandOutput {
        stdout: stdout_str,
        stderr: stderr_str,
        exit_code,
        success: true,
    })
}

#[cfg(feature = "async")]
async fn run_with_timeout_stdin(
    binary: &std::path::Path,
    args: &[String],
    env: &std::collections::HashMap<String, String>,
    working_dir: Option<&std::path::Path>,
    timeout: Duration,
    stdin_content: String,
) -> Result<CommandOutput> {
    use tokio::io::AsyncWriteExt;

    let mut cmd = Command::new(binary);
    cmd.args(args);
    cmd.stdin(std::process::Stdio::piped());
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

    // Write the prompt to stdin, then drop the handle so the child sees EOF.
    if let Some(mut stdin) = child.stdin.take() {
        stdin
            .write_all(stdin_content.as_bytes())
            .await
            .map_err(|e| Error::Io {
                message: format!("failed to write to claude stdin: {e}"),
                source: e,
                working_dir: working_dir.map(|p| p.to_path_buf()),
            })?;
        // Drop stdin so the child sees EOF.
    }

    let mut stdout_handle = child.stdout.take().expect("stdout was piped");
    let mut stderr_handle = child.stderr.take().expect("stderr was piped");

    let wait_and_drain = async {
        let (status, stdout_str, stderr_str) = tokio::join!(
            child.wait(),
            drain(&mut stdout_handle),
            drain(&mut stderr_handle),
        );
        (status, stdout_str, stderr_str)
    };

    match tokio::time::timeout(timeout, wait_and_drain).await {
        Ok((Ok(status), stdout, stderr)) => {
            let exit_code = status.code().unwrap_or(-1);

            if !status.success() {
                return Err(Error::from_command_failure(
                    format!("{} {}", binary.display(), args.join(" ")),
                    exit_code,
                    stdout,
                    stderr,
                    working_dir.map(|p| p.to_path_buf()),
                ));
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
            let _ = child.kill().await;
            let drain_budget = Duration::from_millis(200);
            let stdout_str = tokio::time::timeout(drain_budget, drain(&mut stdout_handle))
                .await
                .unwrap_or_default();
            let stderr_str = tokio::time::timeout(drain_budget, drain(&mut stderr_handle))
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

#[cfg(feature = "async")]
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
#[cfg(feature = "async")]
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

#[cfg(feature = "async")]
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
        return Err(Error::from_command_failure(
            format!("{} {}", binary.display(), args.join(" ")),
            exit_code,
            stdout,
            stderr,
            working_dir.map(|p| p.to_path_buf()),
        ));
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
#[cfg(feature = "async")]
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
                return Err(Error::from_command_failure(
                    format!("{} {}", binary.display(), args.join(" ")),
                    exit_code,
                    stdout,
                    stderr,
                    working_dir.map(|p| p.to_path_buf()),
                ));
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

#[cfg(feature = "async")]
async fn drain<R: AsyncReadExt + Unpin>(reader: &mut R) -> String {
    let mut buf = Vec::new();
    let _ = reader.read_to_end(&mut buf).await;
    String::from_utf8_lossy(&buf).into_owned()
}

// ---------- sync twins ----------

/// Blocking mirror of [`run_claude`]. Available with the `sync` feature.
#[cfg(feature = "sync")]
pub fn run_claude_sync(claude: &Claude, args: Vec<String>) -> Result<CommandOutput> {
    run_claude_with_retry_sync(claude, args, None)
}

/// Blocking mirror of [`run_claude_with_retry`].
#[cfg(feature = "sync")]
pub fn run_claude_with_retry_sync(
    claude: &Claude,
    args: Vec<String>,
    retry_override: Option<&crate::retry::RetryPolicy>,
) -> Result<CommandOutput> {
    let policy = retry_override.or(claude.retry_policy.as_ref());

    match policy {
        Some(policy) => {
            crate::retry::with_retry_sync(policy, || run_claude_once_sync(claude, args.clone()))
        }
        None => run_claude_once_sync(claude, args),
    }
}

/// Blocking mirror of [`run_claude_with_stdin_prompt`].
///
/// stdin mode does not retry -- the stdin pipe is consumed after the first
/// attempt and cannot be rewound.
#[cfg(feature = "sync")]
pub fn run_claude_with_stdin_prompt_sync(
    claude: &Claude,
    args: Vec<String>,
    stdin_content: String,
) -> Result<CommandOutput> {
    let mut command_args = Vec::new();
    command_args.extend(claude.global_args.clone());
    command_args.extend(args);

    debug!(binary = %claude.binary.display(), args = ?command_args, "executing claude command (stdin prompt, sync)");

    if let Some(timeout) = claude.timeout {
        run_with_timeout_stdin_sync(
            &claude.binary,
            &command_args,
            &claude.env,
            claude.working_dir.as_deref(),
            timeout,
            stdin_content,
        )
    } else {
        run_internal_stdin_sync(
            &claude.binary,
            &command_args,
            &claude.env,
            claude.working_dir.as_deref(),
            stdin_content,
        )
    }
}

#[cfg(feature = "sync")]
fn run_internal_stdin_sync(
    binary: &std::path::Path,
    args: &[String],
    env: &std::collections::HashMap<String, String>,
    working_dir: Option<&std::path::Path>,
    stdin_content: String,
) -> Result<CommandOutput> {
    use std::io::Write;
    use std::process::{Command as StdCommand, Stdio};

    let mut cmd = StdCommand::new(binary);
    cmd.args(args);
    cmd.stdin(Stdio::piped());
    cmd.stdout(Stdio::piped());
    cmd.stderr(Stdio::piped());
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

    // Write the prompt to stdin, then drop the handle so the child sees EOF.
    if let Some(mut stdin) = child.stdin.take() {
        stdin
            .write_all(stdin_content.as_bytes())
            .map_err(|e| Error::Io {
                message: format!("failed to write to claude stdin: {e}"),
                source: e,
                working_dir: working_dir.map(|p| p.to_path_buf()),
            })?;
        stdin.flush().map_err(|e| Error::Io {
            message: format!("failed to flush claude stdin: {e}"),
            source: e,
            working_dir: working_dir.map(|p| p.to_path_buf()),
        })?;
        // Drop stdin so the child sees EOF.
    }

    let output = child.wait_with_output().map_err(|e| Error::Io {
        message: "failed to wait for claude process".to_string(),
        source: e,
        working_dir: working_dir.map(|p| p.to_path_buf()),
    })?;

    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
    let exit_code = output.status.code().unwrap_or(-1);

    if !output.status.success() {
        return Err(Error::from_command_failure(
            format!("{} {}", binary.display(), args.join(" ")),
            exit_code,
            stdout,
            stderr,
            working_dir.map(|p| p.to_path_buf()),
        ));
    }

    Ok(CommandOutput {
        stdout,
        stderr,
        exit_code,
        success: true,
    })
}

#[cfg(feature = "sync")]
fn run_with_timeout_stdin_sync(
    binary: &std::path::Path,
    args: &[String],
    env: &std::collections::HashMap<String, String>,
    working_dir: Option<&std::path::Path>,
    timeout: Duration,
    stdin_content: String,
) -> Result<CommandOutput> {
    use std::io::Write;
    use std::process::{Command as StdCommand, Stdio};
    use std::thread;
    use wait_timeout::ChildExt;

    let mut cmd = StdCommand::new(binary);
    cmd.args(args);
    cmd.stdin(Stdio::piped());
    cmd.stdout(Stdio::piped());
    cmd.stderr(Stdio::piped());
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

    // Write the prompt to stdin, then drop the handle so the child sees EOF.
    if let Some(mut stdin) = child.stdin.take() {
        stdin
            .write_all(stdin_content.as_bytes())
            .map_err(|e| Error::Io {
                message: format!("failed to write to claude stdin: {e}"),
                source: e,
                working_dir: working_dir.map(|p| p.to_path_buf()),
            })?;
        stdin.flush().map_err(|e| Error::Io {
            message: format!("failed to flush claude stdin: {e}"),
            source: e,
            working_dir: working_dir.map(|p| p.to_path_buf()),
        })?;
        // Drop stdin so the child sees EOF.
    }

    let stdout = child.stdout.take().expect("stdout was piped");
    let stderr = child.stderr.take().expect("stderr was piped");

    let stdout_thread = thread::spawn(move || drain_sync(stdout));
    let stderr_thread = thread::spawn(move || drain_sync(stderr));

    match child.wait_timeout(timeout).map_err(|e| Error::Io {
        message: "failed to wait for claude process".to_string(),
        source: e,
        working_dir: working_dir.map(|p| p.to_path_buf()),
    })? {
        Some(status) => {
            let stdout = stdout_thread.join().unwrap_or_default();
            let stderr = stderr_thread.join().unwrap_or_default();
            let exit_code = status.code().unwrap_or(-1);

            if !status.success() {
                return Err(Error::from_command_failure(
                    format!("{} {}", binary.display(), args.join(" ")),
                    exit_code,
                    stdout,
                    stderr,
                    working_dir.map(|p| p.to_path_buf()),
                ));
            }

            Ok(CommandOutput {
                stdout,
                stderr,
                exit_code,
                success: true,
            })
        }
        None => {
            let _ = child.kill();
            let _ = child.wait();
            let (stdout_str, stderr_str) =
                join_with_deadline(stdout_thread, stderr_thread, Duration::from_millis(200));
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

#[cfg(feature = "sync")]
fn run_claude_once_sync(claude: &Claude, args: Vec<String>) -> Result<CommandOutput> {
    let mut command_args = Vec::new();
    command_args.extend(claude.global_args.clone());
    command_args.extend(args);

    debug!(binary = %claude.binary.display(), args = ?command_args, "executing claude command (sync)");

    if let Some(timeout) = claude.timeout {
        run_with_timeout_sync(
            &claude.binary,
            &command_args,
            &claude.env,
            claude.working_dir.as_deref(),
            timeout,
        )
    } else {
        run_internal_sync(
            &claude.binary,
            &command_args,
            &claude.env,
            claude.working_dir.as_deref(),
        )
    }
}

/// Blocking mirror of [`run_claude_allow_exit_codes`].
#[cfg(feature = "sync")]
pub fn run_claude_allow_exit_codes_sync(
    claude: &Claude,
    args: Vec<String>,
    allowed_codes: &[i32],
) -> Result<CommandOutput> {
    match run_claude_sync(claude, args) {
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

#[cfg(feature = "sync")]
fn run_internal_sync(
    binary: &std::path::Path,
    args: &[String],
    env: &std::collections::HashMap<String, String>,
    working_dir: Option<&std::path::Path>,
) -> Result<CommandOutput> {
    use std::process::{Command as StdCommand, Stdio};

    let mut cmd = StdCommand::new(binary);
    cmd.args(args);
    cmd.stdin(Stdio::null());
    cmd.env_remove("CLAUDECODE");
    cmd.env_remove("CLAUDE_CODE_ENTRYPOINT");

    if let Some(dir) = working_dir {
        cmd.current_dir(dir);
    }

    for (key, value) in env {
        cmd.env(key, value);
    }

    let output = cmd.output().map_err(|e| Error::Io {
        message: format!("failed to spawn claude: {e}"),
        source: e,
        working_dir: working_dir.map(|p| p.to_path_buf()),
    })?;

    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
    let exit_code = output.status.code().unwrap_or(-1);

    if !output.status.success() {
        return Err(Error::from_command_failure(
            format!("{} {}", binary.display(), args.join(" ")),
            exit_code,
            stdout,
            stderr,
            working_dir.map(|p| p.to_path_buf()),
        ));
    }

    Ok(CommandOutput {
        stdout,
        stderr,
        exit_code,
        success: true,
    })
}

/// Blocking run with a timeout. Mirrors [`run_with_timeout`]: spawns
/// the child, drains stdout/stderr on dedicated threads so neither
/// pipe buffer can fill up while we wait, then uses
/// [`wait_timeout::ChildExt::wait_timeout`] to enforce the deadline.
/// On timeout, the child is SIGKILLed and reaped; partial output is
/// logged at warn but the returned [`Error::Timeout`] does not carry it.
#[cfg(feature = "sync")]
fn run_with_timeout_sync(
    binary: &std::path::Path,
    args: &[String],
    env: &std::collections::HashMap<String, String>,
    working_dir: Option<&std::path::Path>,
    timeout: Duration,
) -> Result<CommandOutput> {
    use std::process::{Command as StdCommand, Stdio};
    use std::thread;
    use wait_timeout::ChildExt;

    let mut cmd = StdCommand::new(binary);
    cmd.args(args);
    cmd.stdin(Stdio::null());
    cmd.stdout(Stdio::piped());
    cmd.stderr(Stdio::piped());
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

    // Detach stdout/stderr onto their own threads so neither can block
    // the child by filling its pipe buffer. Each thread owns its half
    // and drops it on completion, which closes the parent's fd and
    // lets read_to_end() return EOF once the child exits.
    let stdout = child.stdout.take().expect("stdout was piped");
    let stderr = child.stderr.take().expect("stderr was piped");

    let stdout_thread = thread::spawn(move || drain_sync(stdout));
    let stderr_thread = thread::spawn(move || drain_sync(stderr));

    match child.wait_timeout(timeout).map_err(|e| Error::Io {
        message: "failed to wait for claude process".to_string(),
        source: e,
        working_dir: working_dir.map(|p| p.to_path_buf()),
    })? {
        Some(status) => {
            let stdout = stdout_thread.join().unwrap_or_default();
            let stderr = stderr_thread.join().unwrap_or_default();
            let exit_code = status.code().unwrap_or(-1);

            if !status.success() {
                return Err(Error::from_command_failure(
                    format!("{} {}", binary.display(), args.join(" ")),
                    exit_code,
                    stdout,
                    stderr,
                    working_dir.map(|p| p.to_path_buf()),
                ));
            }

            Ok(CommandOutput {
                stdout,
                stderr,
                exit_code,
                success: true,
            })
        }
        None => {
            // Timeout: SIGKILL and reap. If the child has spawned
            // subprocesses that inherited our pipe fds, the drain
            // threads can block indefinitely; cap the join with a
            // short budget so the timeout error returns promptly.
            let _ = child.kill();
            let _ = child.wait();

            let (stdout_str, stderr_str) =
                join_with_deadline(stdout_thread, stderr_thread, Duration::from_millis(200));

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

#[cfg(feature = "sync")]
fn drain_sync<R: std::io::Read>(mut reader: R) -> String {
    let mut buf = Vec::new();
    let _ = reader.read_to_end(&mut buf);
    String::from_utf8_lossy(&buf).into_owned()
}

/// Wait for both drain threads to finish, returning "" for any that
/// miss the deadline. Threads aren't cancellable in std; if the child's
/// subprocesses are still holding a pipe fd open after kill(), the
/// drain thread leaks. That's a pathological case; the common timeout
/// path with a responsive child joins in microseconds.
#[cfg(feature = "sync")]
fn join_with_deadline(
    stdout_thread: std::thread::JoinHandle<String>,
    stderr_thread: std::thread::JoinHandle<String>,
    budget: Duration,
) -> (String, String) {
    use std::sync::mpsc;
    use std::thread;

    let (tx, rx) = mpsc::channel::<(&'static str, String)>();

    let tx_out = tx.clone();
    let tx_err = tx;

    thread::spawn(move || {
        let s = stdout_thread.join().unwrap_or_default();
        let _ = tx_out.send(("stdout", s));
    });
    thread::spawn(move || {
        let s = stderr_thread.join().unwrap_or_default();
        let _ = tx_err.send(("stderr", s));
    });

    let mut stdout = String::new();
    let mut stderr = String::new();
    let deadline = std::time::Instant::now() + budget;

    for _ in 0..2 {
        let now = std::time::Instant::now();
        if now >= deadline {
            break;
        }
        match rx.recv_timeout(deadline - now) {
            Ok(("stdout", s)) => stdout = s,
            Ok(("stderr", s)) => stderr = s,
            Ok(_) => unreachable!(),
            Err(_) => break,
        }
    }

    (stdout, stderr)
}
