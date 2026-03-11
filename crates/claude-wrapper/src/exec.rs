use std::time::Duration;

use tokio::process::Command;
use tracing::debug;

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

    let output = cmd.output().await?;

    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
    let exit_code = output.status.code().unwrap_or(-1);

    if !output.status.success() {
        return Err(Error::CommandFailed {
            command: format!("{} {}", binary.display(), args.join(" ")),
            exit_code,
            stdout,
            stderr,
        });
    }

    Ok(CommandOutput {
        stdout,
        stderr,
        exit_code,
        success: true,
    })
}

async fn run_with_timeout(
    binary: &std::path::Path,
    args: &[String],
    env: &std::collections::HashMap<String, String>,
    working_dir: Option<&std::path::Path>,
    timeout: Duration,
) -> Result<CommandOutput> {
    tokio::time::timeout(timeout, run_internal(binary, args, env, working_dir))
        .await
        .map_err(|_| Error::Timeout {
            timeout_seconds: timeout.as_secs(),
        })?
}
