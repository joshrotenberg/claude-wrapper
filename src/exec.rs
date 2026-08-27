//! Process spawning and execution for the `claude` CLI.
//!
//! Builds and runs the child process behind every command: applies the
//! [`Claude`] client's binary path, working directory, environment, and
//! timeout, scrubs the `CLAUDECODE` env var so nested runs are not
//! detected as recursive, drains stdout/stderr without deadlocking, and
//! maps failures onto [`Error`] via
//! [`from_command_failure`](crate::error::Error::from_command_failure).
//! Both the async (tokio) and blocking (`sync` feature) paths live here.
//!
//! Every spawn places the child in its own process group on Unix, and
//! every async spawn sets `kill_on_drop(true)`. Dropping an in-flight
//! execute future (a lost `tokio::select!` race, a caller-side timeout)
//! SIGKILLs the whole group (via the crate-internal `GroupKillGuard`),
//! so subprocesses the
//! CLI spawned for tool use (shells, MCP servers, test runners) die
//! with it rather than being reparented and running on. The same
//! group-kill runs when a configured timeout fires, on both the async
//! and blocking paths. The blocking paths cannot be dropped mid-flight,
//! so they have no drop-side equivalent.
//!
//! Consequence of the group split: the child no longer shares the
//! host's terminal process group, so terminal-generated signals
//! (Ctrl-C) do not reach it directly; terminating a run is the
//! wrapper's job, via drop, timeout, or an explicit kill.
//! Terminal-attached hosts that want the terminal to stay the
//! supervisor can opt out with
//! [`ClaudeBuilder::process_group(false)`](crate::ClaudeBuilder::process_group),
//! trading the tree kill away: kills then reach only the direct child.

#[cfg(any(feature = "async", feature = "sync"))]
use std::time::Duration;

#[cfg(feature = "async")]
use tokio::io::AsyncReadExt;
#[cfg(feature = "async")]
use tokio::process::Command;
#[cfg(any(feature = "async", feature = "sync"))]
use tracing::{debug, warn};

use crate::Claude;
#[cfg(any(feature = "async", feature = "sync"))]
use crate::error::{Error, Result};

/// Assemble the full argv passed to the CLI binary: the client's
/// global args followed by the command's own args.
///
/// Single assembly path shared by every exec entry point and
/// [`QueryCommand::to_command_string`](crate::QueryCommand::to_command_string),
/// so a rendered preview cannot drift from what actually spawns.
pub(crate) fn full_command_args(claude: &Claude, args: Vec<String>) -> Vec<String> {
    let mut command_args = claude.global_args.clone();
    command_args.extend(args);
    command_args
}

/// Apply the client's environment policy to one CLI child command.
///
/// Kept as the single environment assembly point for buffered, streaming,
/// sync, timeout, retry, stdin, and duplex spawns. Explicit entries are
/// applied after clearing and after the nested-session scrub, so callers can
/// deliberately restore an entry when required.
#[cfg(any(feature = "async", feature = "sync"))]
pub(crate) fn apply_child_environment(
    cmd: &mut std::process::Command,
    clear_env: bool,
    env: &std::collections::HashMap<String, String>,
) {
    if clear_env {
        cmd.env_clear();
    }
    cmd.env_remove("CLAUDECODE");
    cmd.env_remove("CLAUDE_CODE_ENTRYPOINT");
    cmd.envs(env);
}

/// The subcommand label for a span, derived from the argv.
///
/// The first token is the subcommand for subcommand-style invocations
/// (`mcp`, `plugin`, `doctor`). For print-mode runs it is the leading
/// flag (`--print`), which is equally informative. Never a value:
/// values always follow a flag, and the first token cannot be one.
///
/// Deliberately not the whole argv. Prompts arrive as argv positionals
/// and must never reach a span field.
#[cfg(any(feature = "async", feature = "sync"))]
pub(crate) fn span_command(args: &[String]) -> &str {
    args.first().map(String::as_str).unwrap_or("<none>")
}

/// Open the span covering one CLI invocation.
///
/// `exit_code` and `duration_ms` are declared empty and recorded when
/// the call finishes, so a subscriber sees them on close. Carries the
/// binary and working directory, never the prompt and never the env.
#[cfg(any(feature = "async", feature = "sync"))]
pub(crate) fn exec_span(claude: &Claude, args: &[String], mode: &'static str) -> tracing::Span {
    tracing::debug_span!(
        "claude.exec",
        command = span_command(args),
        mode,
        binary = %claude.binary.display(),
        cwd = claude.working_dir.as_deref().map(|d| d.display().to_string()),
        exit_code = tracing::field::Empty,
        duration_ms = tracing::field::Empty,
    )
}

/// Record the outcome of an invocation on its span.
#[cfg(any(feature = "async", feature = "sync"))]
pub(crate) fn record_exec_outcome(
    span: &tracing::Span,
    exit_code: i32,
    started: std::time::Instant,
) {
    span.record("exit_code", exit_code);
    span.record("duration_ms", started.elapsed().as_millis() as u64);
}

/// Raw output from a claude CLI invocation.
#[derive(Debug, Clone)]
pub struct CommandOutput {
    /// Captured standard output.
    pub stdout: String,
    /// Captured standard error.
    pub stderr: String,
    /// Process exit code.
    pub exit_code: i32,
    /// Whether the process exited successfully (exit code 0).
    pub success: bool,
}

/// Kills the child's entire process group when dropped, unless disarmed.
///
/// Every spawn puts the child in its own process group on Unix
/// (`process_group(0)`), so the group id equals the child's pid.
/// `kill_on_drop` and `Child::kill` only reach the direct child; this
/// guard extends cancellation to the subprocesses the CLI spawns for
/// tool use (shells, MCP servers, test runners), which would otherwise
/// be reparented and keep running.
///
/// Callers must [`disarm`](Self::disarm) the guard once the child's
/// exit status has been observed: past that point the pid can be reaped
/// and recycled, and signalling a recycled group would hit unrelated
/// processes. While the child is unreaped (running or zombie) its pid
/// cannot be recycled, so firing is safe.
///
/// On non-Unix targets the guard is a no-op.
/// Arm the group-kill guard and tell the observer the child exists.
///
/// Kept together so the two cannot disagree about whether the child leads its
/// own group: the `pgid` reported is `Some` exactly when the guard is armed,
/// which is exactly when the pid is safe to `killpg`.
/// The spawn-policy knobs every spawn path carries together.
///
/// Bundled because they always travel as a set and because threading them
/// individually pushed the timeout paths past clippy's argument threshold:
/// whether the child leads its own group, how long to wait before escalating a
/// kill, whether the child should die with its parent, and who to tell that it
/// exists.
#[cfg(any(feature = "async", feature = "sync"))]
#[derive(Clone, Copy)]
pub(crate) struct SpawnPolicy<'a> {
    pub(crate) process_group: bool,
    pub(crate) kill_grace: Option<Duration>,
    pub(crate) output_limit: Option<usize>,
    pub(crate) die_with_parent: bool,
    pub(crate) on_spawn: Option<&'a crate::SpawnObserver>,
}

#[cfg(any(feature = "async", feature = "sync"))]
impl SpawnPolicy<'_> {
    /// The policy a [`Claude`] client describes.
    pub(crate) fn of(claude: &Claude) -> SpawnPolicy<'_> {
        SpawnPolicy {
            process_group: claude.process_group,
            kill_grace: claude.kill_grace,
            output_limit: claude.output_limit,
            die_with_parent: claude.die_with_parent,
            on_spawn: claude.on_spawn.as_ref(),
        }
    }
}

#[cfg(any(feature = "async", feature = "sync"))]
pub(crate) fn arm_and_notify(
    process_group: bool,
    pid: Option<u32>,
    on_spawn: Option<&crate::SpawnObserver>,
) -> GroupKillGuard {
    if let (Some(pid), Some(observer)) = (pid, on_spawn) {
        observer(crate::SpawnInfo {
            pid,
            pgid: process_group.then_some(pid),
        });
    }
    GroupKillGuard::new_if(process_group, pid)
}

#[cfg(any(feature = "async", feature = "sync"))]
pub(crate) struct GroupKillGuard {
    #[cfg(unix)]
    pgid: Option<i32>,
}

#[cfg(any(feature = "async", feature = "sync"))]
impl GroupKillGuard {
    /// Arm a guard only when the child was placed in its own process
    /// group; otherwise the child's pid is not a group id and must
    /// never be signalled (see
    /// [`ClaudeBuilder::process_group`](crate::ClaudeBuilder::process_group)).
    pub(crate) fn new_if(enabled: bool, pid: Option<u32>) -> Self {
        Self::new(if enabled { pid } else { None })
    }

    /// Arm a guard for the child with the given pid (as returned by
    /// `Child::id`). A `None` pid (child already reaped) leaves the
    /// guard disarmed.
    pub(crate) fn new(pid: Option<u32>) -> Self {
        #[cfg(unix)]
        {
            Self {
                pgid: pid.and_then(|p| i32::try_from(p).ok()),
            }
        }
        #[cfg(not(unix))]
        {
            let _ = pid;
            Self {}
        }
    }

    /// Stop the guard from firing: the child's exit status has been
    /// observed, so the group id is no longer safe to signal.
    pub(crate) fn disarm(&mut self) {
        #[cfg(unix)]
        {
            self.pgid = None;
        }
    }

    /// True while the guard can still signal the group.
    pub(crate) fn is_armed(&self) -> bool {
        #[cfg(unix)]
        {
            self.pgid.is_some()
        }
        #[cfg(not(unix))]
        {
            false
        }
    }

    /// SIGTERM the whole group (Unix) so the CLI can flush its
    /// transcript and session state. Does not disarm: callers follow
    /// up with [`kill_now`](Self::kill_now) once the grace elapses.
    pub(crate) fn term_now(&self) {
        #[cfg(unix)]
        if let Some(pgid) = self.pgid {
            // SAFETY: plain FFI call with no pointers or invariants;
            // failure (e.g. ESRCH once the group is gone) is ignored.
            let _ = unsafe { libc::killpg(pgid, libc::SIGTERM) };
        }
    }

    /// SIGKILL the group immediately and disarm.
    pub(crate) fn kill_now(&mut self) {
        #[cfg(unix)]
        if let Some(pgid) = self.pgid.take() {
            // SAFETY: plain FFI call with no pointers or invariants;
            // failure (e.g. ESRCH once the group is gone) is ignored.
            let _ = unsafe { libc::killpg(pgid, libc::SIGKILL) };
        }
    }
}

#[cfg(any(feature = "async", feature = "sync"))]
impl Drop for GroupKillGuard {
    fn drop(&mut self) {
        self.kill_now();
    }
}

/// Whether [`ClaudeBuilder::die_with_parent`](crate::ClaudeBuilder::die_with_parent)
/// does anything on this platform.
///
/// `true` only on Linux, which is the only target with a kernel-level
/// parent-death signal (`PR_SET_PDEATHSIG`). Elsewhere the option is accepted
/// and has no effect, so a supervisor that needs the guarantee everywhere must
/// check this and run its own watchdog rather than assume coverage it does not
/// have.
#[must_use]
pub const fn die_with_parent_supported() -> bool {
    cfg!(target_os = "linux")
}

/// Ask the kernel to SIGKILL the child when this process dies.
///
/// Linux only. Two things make this correct rather than merely present:
///
/// - **The fork/prctl race.** `PR_SET_PDEATHSIG` is set by the child *after*
///   the fork. If the parent dies in that window the signal never arrives and
///   the child orphans anyway, which is the exact case this exists to prevent.
///   So the hook re-reads `getppid()` immediately afterwards and exits if the
///   parent already changed.
/// - **Async-signal-safety.** Everything called here (`prctl`, `getppid`,
///   `_exit`) is on the post-fork allowlist. Anything that allocates or takes a
///   lock would risk deadlocking the child.
///
/// The signal is also cleared across `execve` only for setuid binaries, which
/// `claude` is not, so it survives into the CLI itself.
#[cfg(all(unix, any(feature = "async", feature = "sync")))]
fn pdeathsig_hook() -> impl FnMut() -> std::io::Result<()> + Send + Sync + 'static {
    // Read the parent pid before the fork: inside the child, "the parent we
    // meant" is this value, not whatever getppid happens to return later.
    let parent = std::process::id();
    move || {
        #[cfg(target_os = "linux")]
        {
            // SAFETY: async-signal-safe calls only, as required post-fork.
            unsafe {
                if libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGKILL) != 0 {
                    return Err(std::io::Error::last_os_error());
                }
                // Lost the race: the parent died before the signal was armed.
                if libc::getppid() as u32 != parent {
                    libc::_exit(1);
                }
            }
        }
        #[cfg(not(target_os = "linux"))]
        {
            let _ = parent;
        }
        Ok(())
    }
}

/// Apply the parent-death policy to an async spawn. No-op off Linux; see
/// [`die_with_parent_supported`].
#[cfg(feature = "async")]
pub(crate) fn apply_die_with_parent(cmd: &mut Command, enabled: bool) {
    #[cfg(unix)]
    if enabled {
        // SAFETY: the hook is async-signal-safe; see `pdeathsig_hook`.
        unsafe {
            cmd.pre_exec(pdeathsig_hook());
        }
    }
    #[cfg(not(unix))]
    {
        let _ = (cmd, enabled);
    }
}

/// Blocking mirror of [`apply_die_with_parent`].
#[cfg(feature = "sync")]
pub(crate) fn apply_die_with_parent_sync(cmd: &mut std::process::Command, enabled: bool) {
    #[cfg(unix)]
    if enabled {
        use std::os::unix::process::CommandExt;
        // SAFETY: the hook is async-signal-safe; see `pdeathsig_hook`.
        unsafe {
            cmd.pre_exec(pdeathsig_hook());
        }
    }
    #[cfg(not(unix))]
    {
        let _ = (cmd, enabled);
    }
}

/// Apply the client's process-group policy to an async spawn: place
/// the child in its own group (Unix) unless the builder opted out via
/// [`ClaudeBuilder::process_group`](crate::ClaudeBuilder::process_group).
#[cfg(feature = "async")]
pub(crate) fn apply_process_group(cmd: &mut Command, enabled: bool) {
    #[cfg(unix)]
    if enabled {
        cmd.process_group(0);
    }
    #[cfg(not(unix))]
    {
        let _ = (cmd, enabled);
    }
}

/// Blocking mirror of [`apply_process_group`].
#[cfg(feature = "sync")]
pub(crate) fn apply_process_group_sync(cmd: &mut std::process::Command, enabled: bool) {
    #[cfg(unix)]
    if enabled {
        use std::os::unix::process::CommandExt;
        cmd.process_group(0);
    }
    #[cfg(not(unix))]
    {
        let _ = (cmd, enabled);
    }
}

/// Escalated group kill for the waitable paths: SIGTERM the group,
/// wait out `grace` without reaping (the zombie child keeps the group
/// id reserved, so a recycled pid can never be signalled), then
/// SIGKILL whatever remains. With no grace configured, or when the
/// child is not in its own process group, this is an immediate
/// SIGKILL. Drop-path cancellation cannot wait and always SIGKILLs
/// immediately via the guard's `Drop`.
#[cfg(feature = "async")]
pub(crate) async fn kill_group_with_grace(group: &mut GroupKillGuard, grace: Option<Duration>) {
    if let Some(g) = grace
        && !g.is_zero()
        && group.is_armed()
    {
        group.term_now();
        tokio::time::sleep(g).await;
    }
    group.kill_now();
}

#[cfg(feature = "async")]
enum StopReason {
    Timeout { timeout_seconds: u64 },
    Cancelled,
}

#[cfg(feature = "async")]
impl StopReason {
    fn into_error(self) -> Error {
        match self {
            Self::Timeout { timeout_seconds } => Error::Timeout { timeout_seconds },
            Self::Cancelled => Error::Cancelled,
        }
    }
}

/// A stop that never fires, for the paths with neither a deadline nor a
/// cancellation signal.
#[cfg(feature = "async")]
fn never_stop() -> StopFuture<'static> {
    Box::pin(std::future::pending())
}

#[cfg(feature = "async")]
type StopFuture<'a> = std::pin::Pin<Box<dyn std::future::Future<Output = StopReason> + Send + 'a>>;

#[cfg(feature = "async")]
fn timeout_stop(timeout: Duration) -> StopFuture<'static> {
    Box::pin(async move {
        tokio::time::sleep(timeout).await;
        StopReason::Timeout {
            timeout_seconds: timeout.as_secs(),
        }
    })
}

#[cfg(feature = "async")]
fn cancellation_or_timeout<'a, C>(cancel: C, timeout: Option<Duration>) -> StopFuture<'a>
where
    C: std::future::Future<Output = ()> + Send + 'a,
{
    Box::pin(async move {
        match timeout {
            Some(timeout) => tokio::select! {
                () = cancel => StopReason::Cancelled,
                () = tokio::time::sleep(timeout) => StopReason::Timeout {
                    timeout_seconds: timeout.as_secs(),
                },
            },
            None => {
                cancel.await;
                StopReason::Cancelled
            }
        }
    })
}

#[cfg(feature = "async")]
async fn stop_and_reap(
    child: &mut tokio::process::Child,
    group: &mut GroupKillGuard,
    grace: Option<Duration>,
    working_dir: Option<&std::path::Path>,
) -> Result<()> {
    kill_group_with_grace(group, grace).await;
    if child
        .try_wait()
        .map_err(|e| wait_error(e, working_dir))?
        .is_none()
        && let Err(error) = child.start_kill()
        && error.kind() != std::io::ErrorKind::InvalidInput
    {
        return Err(wait_error(error, working_dir));
    }
    child.wait().await.map_err(|e| wait_error(e, working_dir))?;
    Ok(())
}

#[cfg(feature = "async")]
fn wait_error(error: std::io::Error, working_dir: Option<&std::path::Path>) -> Error {
    Error::Io {
        message: "failed to wait for claude process".to_string(),
        source: error,
        working_dir: working_dir.map(|p| p.to_path_buf()),
    }
}

/// Blocking mirror of [`kill_group_with_grace`].
#[cfg(feature = "sync")]
pub(crate) fn kill_group_with_grace_sync(group: &mut GroupKillGuard, grace: Option<Duration>) {
    if let Some(g) = grace
        && !g.is_zero()
        && group.is_armed()
    {
        group.term_now();
        std::thread::sleep(g);
    }
    group.kill_now();
}

/// Run a claude command with the given arguments.
///
/// If the [`Claude`] client has a retry policy set, transient errors will be
/// retried according to that policy. A per-command retry policy can be passed
/// to override the client default.
///
/// Dropping the returned future mid-flight kills the spawned `claude`
/// process and, on Unix, its whole process group (SIGKILL): an
/// abandoned run does not keep executing in the background, and the
/// subprocesses it spawned for tool use die with it.
#[cfg(feature = "async")]
pub async fn run_claude(claude: &Claude, args: Vec<String>) -> Result<CommandOutput> {
    run_claude_with_retry(claude, args, None).await
}

/// Run a Claude command with an explicit cancellation signal.
///
/// Retry does not apply. When `cancel` resolves, the wrapper terminates the
/// owned process group and reaps the direct child before returning
/// [`Error::Cancelled`]. A configured client timeout uses the same path.
#[cfg(feature = "async")]
pub async fn run_claude_cancellable<C>(
    claude: &Claude,
    args: Vec<String>,
    cancel: C,
) -> Result<CommandOutput>
where
    C: std::future::Future<Output = ()> + Send,
{
    let command_args = full_command_args(claude, args);
    let span = exec_span(claude, &command_args, "cancellable");
    let _enter = span.enter();
    let started = std::time::Instant::now();
    let stop = cancellation_or_timeout(cancel, claude.timeout);
    let output = run_with_stop(
        &claude.binary,
        &command_args,
        &claude.env,
        claude.clear_env,
        claude.working_dir.as_deref(),
        stop,
        SpawnPolicy::of(claude),
    )
    .await?;
    record_exec_outcome(&span, output.exit_code, started);
    Ok(output)
}

/// Run a claude command with an optional per-command retry policy override.
///
/// Dropping the returned future kills the child; see [`run_claude`].
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
///
/// Dropping the returned future kills the child; see [`run_claude`].
#[cfg(feature = "async")]
pub async fn run_claude_with_stdin_prompt(
    claude: &Claude,
    args: Vec<String>,
    stdin_content: String,
) -> Result<CommandOutput> {
    run_claude_with_stdin_prompt_internal(claude, args, stdin_content).await
}

/// Run Claude with a stdin prompt and an explicit cancellation signal.
///
/// Cancellation, timeout, and stdin communication failures settle process
/// ownership before returning. Retry does not apply.
#[cfg(feature = "async")]
pub async fn run_claude_with_stdin_prompt_cancellable<C>(
    claude: &Claude,
    args: Vec<String>,
    stdin_content: String,
    cancel: C,
) -> Result<CommandOutput>
where
    C: std::future::Future<Output = ()> + Send,
{
    let command_args = full_command_args(claude, args);
    let span = exec_span(claude, &command_args, "stdin-cancellable");
    let _enter = span.enter();
    let started = std::time::Instant::now();
    let stop = cancellation_or_timeout(cancel, claude.timeout);
    let output = run_stdin_with_stop(
        &claude.binary,
        &command_args,
        &claude.env,
        claude.clear_env,
        claude.working_dir.as_deref(),
        stdin_content,
        stop,
        SpawnPolicy::of(claude),
    )
    .await?;
    record_exec_outcome(&span, output.exit_code, started);
    Ok(output)
}

#[cfg(feature = "async")]
async fn run_claude_with_stdin_prompt_internal(
    claude: &Claude,
    args: Vec<String>,
    stdin_content: String,
) -> Result<CommandOutput> {
    let command_args = full_command_args(claude, args);

    let span = exec_span(claude, &command_args, "stdin");
    let _enter = span.enter();
    let started = std::time::Instant::now();
    debug!(binary = %claude.binary.display(), args = ?command_args, "executing claude command (stdin prompt)");

    let binary = &claude.binary;
    let env = &claude.env;
    let clear_env = claude.clear_env;
    let working_dir = claude.working_dir.as_deref();

    let result = if let Some(timeout) = claude.timeout {
        run_with_timeout_stdin(
            binary,
            &command_args,
            env,
            clear_env,
            working_dir,
            timeout,
            stdin_content,
            SpawnPolicy::of(claude),
        )
        .await
    } else {
        run_internal_stdin(
            binary,
            &command_args,
            env,
            clear_env,
            working_dir,
            stdin_content,
            SpawnPolicy::of(claude),
        )
        .await
    };

    if let Ok(output) = &result {
        record_exec_outcome(&span, output.exit_code, started);
    }
    result
}

/// Run a command with a stdin prompt, no deadline and no cancellation.
///
/// Delegates to [`run_stdin_with_stop`] with a stop that never fires,
/// for the same reason as [`run_internal`]. This also makes the write
/// concurrent with the drain on this path rather than sequential, so a
/// prompt large enough to fill the stdin pipe cannot deadlock against a
/// child that is blocked writing stdout.
#[cfg(feature = "async")]
async fn run_internal_stdin(
    binary: &std::path::Path,
    args: &[String],
    env: &std::collections::HashMap<String, String>,
    clear_env: bool,
    working_dir: Option<&std::path::Path>,
    stdin_content: String,
    policy: SpawnPolicy<'_>,
) -> Result<CommandOutput> {
    run_stdin_with_stop(
        binary,
        args,
        env,
        clear_env,
        working_dir,
        stdin_content,
        never_stop(),
        policy,
    )
    .await
}

#[cfg(feature = "async")]
#[allow(clippy::too_many_arguments)]
async fn run_with_timeout_stdin(
    binary: &std::path::Path,
    args: &[String],
    env: &std::collections::HashMap<String, String>,
    clear_env: bool,
    working_dir: Option<&std::path::Path>,
    timeout: Duration,
    stdin_content: String,
    policy: SpawnPolicy<'_>,
) -> Result<CommandOutput> {
    run_stdin_with_stop(
        binary,
        args,
        env,
        clear_env,
        working_dir,
        stdin_content,
        timeout_stop(timeout),
        policy,
    )
    .await
}

#[cfg(feature = "async")]
#[allow(clippy::too_many_arguments)]
async fn run_stdin_with_stop(
    binary: &std::path::Path,
    args: &[String],
    env: &std::collections::HashMap<String, String>,
    clear_env: bool,
    working_dir: Option<&std::path::Path>,
    stdin_content: String,
    stop: StopFuture<'_>,
    policy: SpawnPolicy<'_>,
) -> Result<CommandOutput> {
    let SpawnPolicy {
        process_group,
        kill_grace,
        output_limit,
        die_with_parent,
        on_spawn,
    } = policy;
    use tokio::io::AsyncWriteExt;

    let mut cmd = Command::new(binary);
    cmd.args(args);
    cmd.stdin(std::process::Stdio::piped());
    cmd.stdout(std::process::Stdio::piped());
    cmd.stderr(std::process::Stdio::piped());
    // Dropping the in-flight future must kill the child, not leave the
    // CLI running unattended (see the module docs).
    cmd.kill_on_drop(true);
    // Own process group (Unix) so cancellation can signal the whole
    // tree, not just the direct child (see GroupKillGuard). Opt out
    // via ClaudeBuilder::process_group.
    apply_process_group(&mut cmd, process_group);
    apply_die_with_parent(&mut cmd, die_with_parent);
    apply_child_environment(cmd.as_std_mut(), clear_env, env);

    if let Some(dir) = working_dir {
        cmd.current_dir(dir);
    }

    let mut child = spawn_retrying_txtbsy(&mut cmd)
        .await
        .map_err(|e| Error::Io {
            message: format!("failed to spawn claude: {e}"),
            source: e,
            working_dir: working_dir.map(|p| p.to_path_buf()),
        })?;
    let mut group = arm_and_notify(process_group, child.id(), on_spawn);

    let child_stdin = child.stdin.take();
    let mut stdout_handle = child.stdout.take().expect("stdout was piped");
    let mut stderr_handle = child.stderr.take().expect("stderr was piped");
    let write = async move {
        let Some(mut stdin) = child_stdin else {
            return Ok(());
        };
        stdin
            .write_all(stdin_content.as_bytes())
            .await
            .map_err(|e| Error::Io {
                message: format!("failed to write to claude stdin: {e}"),
                source: e,
                working_dir: working_dir.map(|p| p.to_path_buf()),
            })
    };
    let read_stdout = capture_stream(
        &mut stdout_handle,
        output_limit,
        crate::OutputStream::Stdout,
        working_dir,
    );
    let read_stderr = capture_stream(
        &mut stderr_handle,
        output_limit,
        crate::OutputStream::Stderr,
        working_dir,
    );
    let wait = async { child.wait().await.map_err(|e| wait_error(e, working_dir)) };
    let run = async {
        let ((), status, stdout, stderr) = tokio::try_join!(write, wait, read_stdout, read_stderr)?;
        Ok::<_, Error>((status, stdout, stderr))
    };

    match tokio::select! {
        outcome = run => Ok(outcome),
        reason = stop => Err(reason),
    } {
        Ok(Ok((status, stdout, stderr))) => {
            group.disarm();
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
        Ok(Err(error)) => {
            stop_and_reap(&mut child, &mut group, kill_grace, working_dir).await?;
            Err(error)
        }
        Err(reason) => {
            // Take down the whole group first (subprocesses may hold our pipe
            // fds), honoring the optional SIGTERM grace, then kill+reap the
            // direct child.
            stop_and_reap(&mut child, &mut group, kill_grace, working_dir).await?;
            Err(reason.into_error())
        }
    }
}

#[cfg(feature = "async")]
async fn run_claude_once(claude: &Claude, args: Vec<String>) -> Result<CommandOutput> {
    let command_args = full_command_args(claude, args);

    let span = exec_span(claude, &command_args, "oneshot");
    let _enter = span.enter();
    let started = std::time::Instant::now();
    debug!(binary = %claude.binary.display(), args = ?command_args, "executing claude command");

    let output = if let Some(timeout) = claude.timeout {
        run_with_timeout(
            &claude.binary,
            &command_args,
            &claude.env,
            claude.clear_env,
            claude.working_dir.as_deref(),
            timeout,
            SpawnPolicy::of(claude),
        )
        .await?
    } else {
        run_internal(
            &claude.binary,
            &command_args,
            &claude.env,
            claude.clear_env,
            claude.working_dir.as_deref(),
            SpawnPolicy::of(claude),
        )
        .await?
    };

    record_exec_outcome(&span, output.exit_code, started);
    Ok(output)
}

/// Run a claude command and allow specific non-zero exit codes.
///
/// Dropping the returned future kills the child; see [`run_claude`].
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

/// Run a command with no deadline and no cancellation.
///
/// Delegates to [`run_with_stop`] with a stop that never fires, so the
/// spawn setup, concurrent drain, capture ceiling, and kill-and-reap
/// handling live in exactly one place. Without a stop future the only
/// reachable kill site is a capture ceiling breach, which is why this
/// path now honours `kill_grace`.
#[cfg(feature = "async")]
async fn run_internal(
    binary: &std::path::Path,
    args: &[String],
    env: &std::collections::HashMap<String, String>,
    clear_env: bool,
    working_dir: Option<&std::path::Path>,
    policy: SpawnPolicy<'_>,
) -> Result<CommandOutput> {
    run_with_stop(
        binary,
        args,
        env,
        clear_env,
        working_dir,
        never_stop(),
        policy,
    )
    .await
}

/// Run a command with a timeout, killing the child's whole process
/// group (Unix) and reaping the child on expiration.
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
    clear_env: bool,
    working_dir: Option<&std::path::Path>,
    timeout: Duration,
    policy: SpawnPolicy<'_>,
) -> Result<CommandOutput> {
    run_with_stop(
        binary,
        args,
        env,
        clear_env,
        working_dir,
        timeout_stop(timeout),
        policy,
    )
    .await
}

#[cfg(feature = "async")]
async fn run_with_stop(
    binary: &std::path::Path,
    args: &[String],
    env: &std::collections::HashMap<String, String>,
    clear_env: bool,
    working_dir: Option<&std::path::Path>,
    stop: StopFuture<'_>,
    policy: SpawnPolicy<'_>,
) -> Result<CommandOutput> {
    let SpawnPolicy {
        process_group,
        kill_grace,
        output_limit,
        die_with_parent,
        on_spawn,
    } = policy;
    let mut cmd = Command::new(binary);
    cmd.args(args);
    cmd.stdin(std::process::Stdio::null());
    cmd.stdout(std::process::Stdio::piped());
    cmd.stderr(std::process::Stdio::piped());
    // Dropping the in-flight future must kill the child, not leave the
    // CLI running unattended (see the module docs).
    cmd.kill_on_drop(true);
    // Own process group (Unix) so cancellation can signal the whole
    // tree, not just the direct child (see GroupKillGuard). Opt out
    // via ClaudeBuilder::process_group.
    apply_process_group(&mut cmd, process_group);
    apply_die_with_parent(&mut cmd, die_with_parent);
    apply_child_environment(cmd.as_std_mut(), clear_env, env);

    if let Some(dir) = working_dir {
        cmd.current_dir(dir);
    }

    let mut child = spawn_retrying_txtbsy(&mut cmd)
        .await
        .map_err(|e| Error::Io {
            message: format!("failed to spawn claude: {e}"),
            source: e,
            working_dir: working_dir.map(|p| p.to_path_buf()),
        })?;
    let mut group = arm_and_notify(process_group, child.id(), on_spawn);

    let mut stdout = child.stdout.take().expect("stdout was piped");
    let mut stderr = child.stderr.take().expect("stderr was piped");

    // Drain stdout and stderr concurrently with the process wait so
    // neither pipe buffer can fill up and deadlock the child.
    // tokio::try_join! polls all three on the same task; no tokio::spawn
    // (and therefore no `rt` feature) required. try_join rather than
    // join so a read failure or a ceiling breach abandons the run at
    // once instead of waiting on a child that is no longer being read.
    let wait_and_drain = async {
        tokio::try_join!(
            async { child.wait().await.map_err(|e| wait_error(e, working_dir)) },
            capture_stream(
                &mut stdout,
                output_limit,
                crate::OutputStream::Stdout,
                working_dir
            ),
            capture_stream(
                &mut stderr,
                output_limit,
                crate::OutputStream::Stderr,
                working_dir
            ),
        )
    };

    match tokio::select! {
        outcome = wait_and_drain => Ok(outcome),
        reason = stop => Err(reason),
    } {
        Ok(Ok((status, stdout, stderr))) => {
            group.disarm();
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
        // A wait failure, a read failure, or a ceiling breach. The
        // child is still ours in every case, so settle it on the same
        // path a stop takes before surfacing what went wrong.
        Ok(Err(error)) => {
            stop_and_reap(&mut child, &mut group, kill_grace, working_dir).await?;
            Err(error)
        }
        Err(reason) => {
            // Take down the whole group, honoring the optional SIGTERM grace,
            // then kill+reap the direct child. The group kill takes down
            // subprocesses that could otherwise hold our pipe fds open
            // forever; the capped drain below stays as a backstop.
            stop_and_reap(&mut child, &mut group, kill_grace, working_dir).await?;
            let stdout_str = tokio::time::timeout(
                PARTIAL_CAPTURE_BUDGET,
                capture_partial(&mut stdout, output_limit),
            )
            .await
            .unwrap_or_default();
            let stderr_str = tokio::time::timeout(
                PARTIAL_CAPTURE_BUDGET,
                capture_partial(&mut stderr, output_limit),
            )
            .await
            .unwrap_or_default();
            if !stdout_str.is_empty() || !stderr_str.is_empty() {
                warn!(
                    stdout = %stdout_str,
                    stderr = %stderr_str,
                    "partial output from stopped process",
                );
            }
            Err(reason.into_error())
        }
    }
}

/// Read size for a bounded capture. Only used when a ceiling is set;
/// the unbounded path stays on `read_to_end` and its own growth policy.
#[cfg(any(feature = "async", feature = "sync"))]
const CAPTURE_CHUNK: usize = 8 * 1024;

/// How much of an over-ceiling stream is kept to log. The captured
/// prefix can be as large as the ceiling itself, which is not something
/// to hand to a logger, but the first few KiB usually name the runaway.
#[cfg(any(feature = "async", feature = "sync"))]
const BREACH_LOG_BYTES: usize = 2 * 1024;

/// How long to wait for partial output after a kill, on both the async
/// and blocking paths. The child is already gone; this only feeds a
/// warn log, so it is deliberately short.
#[cfg(any(feature = "async", feature = "sync"))]
const PARTIAL_CAPTURE_BUDGET: Duration = Duration::from_millis(200);

/// Result of reading one captured stream to EOF or to its ceiling.
#[cfg(any(feature = "async", feature = "sync"))]
enum Captured {
    /// EOF was reached within the ceiling.
    Complete(String),
    /// The child wrote past the ceiling. Carries at most
    /// [`BREACH_LOG_BYTES`] of the prefix, for diagnostics only: the
    /// rest is dropped rather than returned, since returning it is the
    /// truncated success this exists to avoid.
    Exceeded { head: String },
}

/// Read `reader` to EOF, or stop once it passes `limit` bytes.
///
/// With no limit this is `read_to_end`, unchanged. With a limit the
/// buffer never exceeds `limit`, so peak memory for a run is bounded by
/// the ceiling times the two captured streams.
#[cfg(feature = "async")]
async fn capture<R: AsyncReadExt + Unpin>(
    reader: &mut R,
    limit: Option<usize>,
) -> std::io::Result<Captured> {
    let Some(limit) = limit else {
        let mut buf = Vec::new();
        reader.read_to_end(&mut buf).await?;
        return Ok(Captured::Complete(
            String::from_utf8_lossy(&buf).into_owned(),
        ));
    };

    let mut buf = Vec::new();
    let mut chunk = [0u8; CAPTURE_CHUNK];
    loop {
        let read = reader.read(&mut chunk).await?;
        if read == 0 {
            return Ok(Captured::Complete(
                String::from_utf8_lossy(&buf).into_owned(),
            ));
        }
        if buf.len() + read > limit {
            buf.truncate(BREACH_LOG_BYTES.min(buf.len()));
            return Ok(Captured::Exceeded {
                head: String::from_utf8_lossy(&buf).into_owned(),
            });
        }
        buf.extend_from_slice(&chunk[..read]);
    }
}

/// Capture one stream of a live child, reporting a ceiling breach and a
/// read failure as typed errors.
///
/// The read failure is the reason this returns a `Result` at all: the
/// previous `drain` discarded it with `let _`, which left a capture
/// failure indistinguishable from an empty stream.
#[cfg(feature = "async")]
async fn capture_stream<R: AsyncReadExt + Unpin>(
    reader: &mut R,
    limit: Option<usize>,
    stream: crate::OutputStream,
    working_dir: Option<&std::path::Path>,
) -> Result<String> {
    match capture(reader, limit).await {
        Ok(Captured::Complete(text)) => Ok(text),
        Ok(Captured::Exceeded { head }) => {
            let limit_bytes = limit.unwrap_or_default();
            warn!(
                %stream,
                limit_bytes,
                head = %head,
                "captured output exceeded its ceiling; terminating the child",
            );
            Err(Error::OutputLimitExceeded {
                stream,
                limit_bytes,
            })
        }
        Err(source) => Err(Error::Io {
            message: format!("failed to read claude {stream}: {source}"),
            source,
            working_dir: working_dir.map(|p| p.to_path_buf()),
        }),
    }
}

/// Capture whatever is left in a pipe after the child has been killed.
///
/// Best-effort by construction: the process is already gone and this
/// only feeds a warn log, so a read failure and a ceiling breach both
/// degrade to whatever text is in hand.
#[cfg(feature = "async")]
async fn capture_partial<R: AsyncReadExt + Unpin>(reader: &mut R, limit: Option<usize>) -> String {
    match capture(reader, limit).await {
        Ok(Captured::Complete(text) | Captured::Exceeded { head: text }) => text,
        Err(_) => String::new(),
    }
}

/// Total wall-clock time to keep retrying a spawn that reports `ETXTBSY`.
///
/// Measured as elapsed time rather than a sum of backoffs so a saturated
/// host (a CI job running build + clippy + tests at once) still gets the
/// full window: the busy descriptor can stay open longer than the old
/// 500ms budget under that load, which surfaced as a spurious spawn
/// failure. This is only ever spent when a real `ETXTBSY` occurs, which
/// does not happen against an already-installed binary in production.
#[cfg(any(feature = "async", feature = "sync"))]
const TXTBSY_RETRY_BUDGET: Duration = Duration::from_secs(3);

/// Per-attempt backoff ceiling while retrying `ETXTBSY`.
///
/// Backoff grows exponentially but is capped so retries stay frequent for
/// the whole budget: the busy window can clear at any instant, and a large
/// tail sleep (the old loop reached 1-2s) would keep spawning stalled long
/// after the descriptor closed.
#[cfg(any(feature = "async", feature = "sync"))]
const TXTBSY_MAX_BACKOFF: Duration = Duration::from_millis(25);

/// Spawn `cmd`, retrying briefly on `ETXTBSY` (`ExecutableFileBusy`).
///
/// `execve` fails with `ETXTBSY` when another process holds the target file
/// open for writing. In a multithreaded program this happens transiently even
/// for a file this process has finished writing: if another thread `fork`s
/// while a writable descriptor to the binary is still open, the child inherits
/// that descriptor and holds it until its own `exec` completes. Any `execve`
/// of the file in that window sees a writer and fails. The condition always
/// clears on its own, so retry within a bounded wall-clock budget rather than
/// surfacing a spurious spawn failure.
#[cfg(feature = "async")]
async fn spawn_retrying_txtbsy(cmd: &mut Command) -> std::io::Result<tokio::process::Child> {
    let start = std::time::Instant::now();
    let mut backoff = Duration::from_millis(1);
    loop {
        match cmd.spawn() {
            Err(e)
                if e.kind() == std::io::ErrorKind::ExecutableFileBusy
                    && start.elapsed() < TXTBSY_RETRY_BUDGET =>
            {
                tokio::time::sleep(backoff).await;
                backoff = (backoff * 2).min(TXTBSY_MAX_BACKOFF);
            }
            other => return other,
        }
    }
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
    let command_args = full_command_args(claude, args);

    let span = exec_span(claude, &command_args, "stdin-sync");
    let _enter = span.enter();
    let started = std::time::Instant::now();
    debug!(binary = %claude.binary.display(), args = ?command_args, "executing claude command (stdin prompt, sync)");

    let result = if let Some(timeout) = claude.timeout {
        run_with_deadline_stdin_sync(
            &claude.binary,
            &command_args,
            &claude.env,
            claude.clear_env,
            claude.working_dir.as_deref(),
            Some(timeout),
            stdin_content,
            SpawnPolicy::of(claude),
        )
    } else {
        run_internal_stdin_sync(
            &claude.binary,
            &command_args,
            &claude.env,
            claude.clear_env,
            claude.working_dir.as_deref(),
            stdin_content,
            SpawnPolicy::of(claude),
        )
    };

    if let Ok(output) = &result {
        record_exec_outcome(&span, output.exit_code, started);
    }
    result
}

/// Blocking run with a stdin prompt and no deadline. Delegates to
/// [`run_with_deadline_stdin_sync`], for the same reason as
/// [`run_internal_sync`].
#[cfg(feature = "sync")]
fn run_internal_stdin_sync(
    binary: &std::path::Path,
    args: &[String],
    env: &std::collections::HashMap<String, String>,
    clear_env: bool,
    working_dir: Option<&std::path::Path>,
    stdin_content: String,
    policy: SpawnPolicy<'_>,
) -> Result<CommandOutput> {
    run_with_deadline_stdin_sync(
        binary,
        args,
        env,
        clear_env,
        working_dir,
        None,
        stdin_content,
        policy,
    )
}

/// Blocking mirror of [`run_stdin_with_stop`]. See
/// [`run_with_deadline_sync`] for the wait and kill behavior; this adds
/// writing the prompt to the child's stdin.
#[cfg(feature = "sync")]
#[allow(clippy::too_many_arguments)]
fn run_with_deadline_stdin_sync(
    binary: &std::path::Path,
    args: &[String],
    env: &std::collections::HashMap<String, String>,
    clear_env: bool,
    working_dir: Option<&std::path::Path>,
    deadline: Option<Duration>,
    stdin_content: String,
    policy: SpawnPolicy<'_>,
) -> Result<CommandOutput> {
    let SpawnPolicy {
        process_group,
        kill_grace,
        output_limit,
        die_with_parent,
        on_spawn,
    } = policy;
    use std::io::Write;
    use std::process::{Command as StdCommand, Stdio};

    let mut cmd = StdCommand::new(binary);
    cmd.args(args);
    cmd.stdin(Stdio::piped());
    cmd.stdout(Stdio::piped());
    cmd.stderr(Stdio::piped());
    // Own process group (Unix) so a kill can signal the whole tree,
    // not just the direct child (see GroupKillGuard). Opt out via
    // ClaudeBuilder::process_group.
    apply_process_group_sync(&mut cmd, process_group);
    apply_die_with_parent_sync(&mut cmd, die_with_parent);
    apply_child_environment(&mut cmd, clear_env, env);

    if let Some(dir) = working_dir {
        cmd.current_dir(dir);
    }

    let mut child = spawn_retrying_txtbsy_sync(&mut cmd).map_err(|e| Error::Io {
        message: format!("failed to spawn claude: {e}"),
        source: e,
        working_dir: working_dir.map(|p| p.to_path_buf()),
    })?;
    let mut group = arm_and_notify(process_group, Some(child.id()), on_spawn);

    // Start the capture threads before writing, so a prompt larger than
    // the stdin pipe buffer cannot deadlock against a child that is
    // blocked writing stdout.
    let capture = SyncCapture::start(
        child.stdout.take().expect("stdout was piped"),
        child.stderr.take().expect("stderr was piped"),
        output_limit,
    );

    // Write the prompt to stdin, then drop the handle so the child sees EOF.
    if let Some(mut stdin) = child.stdin.take() {
        let written = stdin
            .write_all(stdin_content.as_bytes())
            .and_then(|()| stdin.flush());
        if let Err(source) = written {
            stop_and_reap_sync(&mut child, &mut group, kill_grace);
            log_partial(capture, "partial output from process with failed stdin");
            return Err(Error::Io {
                message: format!("failed to write to claude stdin: {source}"),
                source,
                working_dir: working_dir.map(|p| p.to_path_buf()),
            });
        }
        // Drop stdin so the child sees EOF.
    }

    finish_sync_run(
        &mut child,
        &mut group,
        capture,
        SyncRun {
            binary,
            args,
            working_dir,
            deadline,
            kill_grace,
            output_limit,
        },
    )
}

#[cfg(feature = "sync")]
fn run_claude_once_sync(claude: &Claude, args: Vec<String>) -> Result<CommandOutput> {
    let command_args = full_command_args(claude, args);

    let span = exec_span(claude, &command_args, "oneshot-sync");
    let _enter = span.enter();
    let started = std::time::Instant::now();
    debug!(binary = %claude.binary.display(), args = ?command_args, "executing claude command (sync)");

    let result = if let Some(timeout) = claude.timeout {
        run_with_deadline_sync(
            &claude.binary,
            &command_args,
            &claude.env,
            claude.clear_env,
            claude.working_dir.as_deref(),
            Some(timeout),
            SpawnPolicy::of(claude),
        )
    } else {
        run_internal_sync(
            &claude.binary,
            &command_args,
            &claude.env,
            claude.clear_env,
            claude.working_dir.as_deref(),
            SpawnPolicy::of(claude),
        )
    };

    if let Ok(output) = &result {
        record_exec_outcome(&span, output.exit_code, started);
    }
    result
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

/// Blocking run with no deadline.
///
/// Delegates to [`run_with_deadline_sync`] so the spawn setup, capture
/// ceiling, and kill-and-reap handling live in one place. Without a
/// deadline the only reachable kill site is a ceiling breach, which is
/// why this path now honours `kill_grace`.
#[cfg(feature = "sync")]
fn run_internal_sync(
    binary: &std::path::Path,
    args: &[String],
    env: &std::collections::HashMap<String, String>,
    clear_env: bool,
    working_dir: Option<&std::path::Path>,
    policy: SpawnPolicy<'_>,
) -> Result<CommandOutput> {
    run_with_deadline_sync(binary, args, env, clear_env, working_dir, None, policy)
}

/// Blocking mirror of [`run_with_stop`]. Spawns the child, drains
/// stdout and stderr on dedicated threads so neither pipe buffer can
/// fill up while we wait, then waits with an optional deadline.
///
/// On a timeout or a capture ceiling breach, the child's whole process
/// group (Unix) is killed after the optional SIGTERM grace and the child
/// is reaped. Partial output is logged at warn; neither
/// [`Error::Timeout`] nor [`Error::OutputLimitExceeded`] carries it.
#[cfg(feature = "sync")]
fn run_with_deadline_sync(
    binary: &std::path::Path,
    args: &[String],
    env: &std::collections::HashMap<String, String>,
    clear_env: bool,
    working_dir: Option<&std::path::Path>,
    deadline: Option<Duration>,
    policy: SpawnPolicy<'_>,
) -> Result<CommandOutput> {
    let SpawnPolicy {
        process_group,
        kill_grace,
        output_limit,
        die_with_parent,
        on_spawn,
    } = policy;
    use std::process::{Command as StdCommand, Stdio};

    let mut cmd = StdCommand::new(binary);
    cmd.args(args);
    cmd.stdin(Stdio::null());
    cmd.stdout(Stdio::piped());
    cmd.stderr(Stdio::piped());
    // Own process group (Unix) so a kill can signal the whole tree,
    // not just the direct child (see GroupKillGuard). Opt out via
    // ClaudeBuilder::process_group.
    apply_process_group_sync(&mut cmd, process_group);
    apply_die_with_parent_sync(&mut cmd, die_with_parent);
    apply_child_environment(&mut cmd, clear_env, env);

    if let Some(dir) = working_dir {
        cmd.current_dir(dir);
    }

    let mut child = spawn_retrying_txtbsy_sync(&mut cmd).map_err(|e| Error::Io {
        message: format!("failed to spawn claude: {e}"),
        source: e,
        working_dir: working_dir.map(|p| p.to_path_buf()),
    })?;
    let mut group = arm_and_notify(process_group, Some(child.id()), on_spawn);

    let capture = SyncCapture::start(
        child.stdout.take().expect("stdout was piped"),
        child.stderr.take().expect("stderr was piped"),
        output_limit,
    );

    finish_sync_run(
        &mut child,
        &mut group,
        capture,
        SyncRun {
            binary,
            args,
            working_dir,
            deadline,
            kill_grace,
            output_limit,
        },
    )
}

#[cfg(feature = "sync")]
fn capture_sync<R: std::io::Read>(
    mut reader: R,
    limit: Option<usize>,
) -> std::io::Result<Captured> {
    let Some(limit) = limit else {
        let mut buf = Vec::new();
        reader.read_to_end(&mut buf)?;
        return Ok(Captured::Complete(
            String::from_utf8_lossy(&buf).into_owned(),
        ));
    };

    let mut buf = Vec::new();
    let mut chunk = [0u8; CAPTURE_CHUNK];
    loop {
        let read = reader.read(&mut chunk)?;
        if read == 0 {
            return Ok(Captured::Complete(
                String::from_utf8_lossy(&buf).into_owned(),
            ));
        }
        if buf.len() + read > limit {
            buf.truncate(BREACH_LOG_BYTES.min(buf.len()));
            return Ok(Captured::Exceeded {
                head: String::from_utf8_lossy(&buf).into_owned(),
            });
        }
        buf.extend_from_slice(&chunk[..read]);
    }
}

/// Which stream first passed its ceiling, shared between the capture
/// threads and the thread waiting on the child.
///
/// A blocking wait cannot be woken, so the waiter polls this instead:
/// once a capture thread stops reading, the child blocks on a full pipe
/// and would otherwise sit there until the timeout (or forever, on a
/// path with no timeout).
#[cfg(feature = "sync")]
mod breach {
    pub(super) const NONE: u8 = 0;
    pub(super) const STDOUT: u8 = 1;
    pub(super) const STDERR: u8 = 2;

    pub(super) fn stream(code: u8) -> Option<crate::OutputStream> {
        match code {
            STDOUT => Some(crate::OutputStream::Stdout),
            STDERR => Some(crate::OutputStream::Stderr),
            _ => None,
        }
    }
}

/// How often the waiter checks for a ceiling breach. Only paid when a
/// ceiling is set; without one the waiter blocks outright. The child is
/// stalled on a full pipe for at most this long before being killed,
/// which costs latency but no memory.
#[cfg(feature = "sync")]
const BREACH_POLL_INTERVAL: Duration = Duration::from_millis(250);

/// The two capture threads and the breach code they publish.
#[cfg(feature = "sync")]
struct SyncCapture {
    stdout: std::thread::JoinHandle<std::io::Result<Captured>>,
    stderr: std::thread::JoinHandle<std::io::Result<Captured>>,
    limit: Option<usize>,
    breached: std::sync::Arc<std::sync::atomic::AtomicU8>,
}

#[cfg(feature = "sync")]
impl SyncCapture {
    /// Detach stdout and stderr onto their own threads so neither can
    /// block the child by filling its pipe buffer. Each thread owns its
    /// half and drops it on completion, which closes the parent's fd and
    /// lets the read return EOF once the child exits.
    fn start(
        stdout: std::process::ChildStdout,
        stderr: std::process::ChildStderr,
        limit: Option<usize>,
    ) -> Self {
        let breached = std::sync::Arc::new(std::sync::atomic::AtomicU8::new(breach::NONE));
        Self {
            stdout: spawn_capture_thread(stdout, limit, breach::STDOUT, &breached),
            stderr: spawn_capture_thread(stderr, limit, breach::STDERR, &breached),
            limit,
            breached,
        }
    }

    /// The flag to poll while waiting, or `None` when no ceiling is set
    /// and the waiter should block instead.
    fn watch(&self) -> Option<&std::sync::atomic::AtomicU8> {
        self.limit.is_some().then(|| self.breached.as_ref())
    }

    /// Join both threads and resolve them into the captured pair, a
    /// ceiling breach, or a read failure. Used on the path where the
    /// child exited on its own, so the threads are already at EOF.
    fn settle(self, working_dir: Option<&std::path::Path>) -> Result<(String, String)> {
        let limit = self.limit;
        let stdout = join_capture(self.stdout, crate::OutputStream::Stdout, limit, working_dir);
        let stderr = join_capture(self.stderr, crate::OutputStream::Stderr, limit, working_dir);
        Ok((stdout?, stderr?))
    }

    /// Best-effort text from both threads, for logging after a kill.
    /// Threads are not cancellable in std, so a thread still blocked on
    /// a pipe fd held open by a surviving grandchild contributes "".
    fn partial(self, budget: Duration) -> (String, String) {
        let (stdout, stderr) = join_with_deadline(self.stdout, self.stderr, budget);
        (partial_text(stdout), partial_text(stderr))
    }
}

#[cfg(feature = "sync")]
fn spawn_capture_thread<R: std::io::Read + Send + 'static>(
    reader: R,
    limit: Option<usize>,
    code: u8,
    breached: &std::sync::Arc<std::sync::atomic::AtomicU8>,
) -> std::thread::JoinHandle<std::io::Result<Captured>> {
    let breached = std::sync::Arc::clone(breached);
    std::thread::spawn(move || {
        let outcome = capture_sync(reader, limit);
        if matches!(outcome, Ok(Captured::Exceeded { .. })) {
            // First breach wins, so the reported stream is the one that
            // actually stopped the run.
            let _ = breached.compare_exchange(
                breach::NONE,
                code,
                std::sync::atomic::Ordering::AcqRel,
                std::sync::atomic::Ordering::Acquire,
            );
        }
        outcome
    })
}

#[cfg(feature = "sync")]
fn join_capture(
    handle: std::thread::JoinHandle<std::io::Result<Captured>>,
    stream: crate::OutputStream,
    limit: Option<usize>,
    working_dir: Option<&std::path::Path>,
) -> Result<String> {
    match handle.join() {
        Ok(Ok(Captured::Complete(text))) => Ok(text),
        Ok(Ok(Captured::Exceeded { head })) => {
            let limit_bytes = limit.unwrap_or_default();
            warn!(
                %stream,
                limit_bytes,
                head = %head,
                "captured output exceeded its ceiling; terminating the child",
            );
            Err(Error::OutputLimitExceeded {
                stream,
                limit_bytes,
            })
        }
        Ok(Err(source)) => Err(Error::Io {
            message: format!("failed to read claude {stream}: {source}"),
            source,
            working_dir: working_dir.map(|p| p.to_path_buf()),
        }),
        Err(_) => Err(Error::Io {
            message: format!("claude {stream} capture thread panicked"),
            source: std::io::Error::other(format!("{stream} capture thread panicked")),
            working_dir: working_dir.map(|p| p.to_path_buf()),
        }),
    }
}

#[cfg(feature = "sync")]
fn partial_text(outcome: Option<std::io::Result<Captured>>) -> String {
    match outcome {
        Some(Ok(Captured::Complete(text) | Captured::Exceeded { head: text })) => text,
        Some(Err(_)) | None => String::new(),
    }
}

/// Outcome of waiting on a blocking child.
#[cfg(feature = "sync")]
enum SyncWait {
    Exited(std::process::ExitStatus),
    TimedOut,
    /// A capture thread passed its ceiling, so the child is stalled on a
    /// full pipe and has to be taken down.
    Breached(crate::OutputStream),
}

/// Wait for `child`, up to `deadline` if there is one, giving up early
/// if `watch` reports a ceiling breach.
#[cfg(feature = "sync")]
fn wait_for_child_sync(
    child: &mut std::process::Child,
    deadline: Option<Duration>,
    watch: Option<&std::sync::atomic::AtomicU8>,
) -> std::io::Result<SyncWait> {
    use std::sync::atomic::Ordering;
    use wait_timeout::ChildExt;

    // No ceiling: nothing can interrupt this, so block rather than
    // waking up to poll a flag that will never be set.
    let Some(watch) = watch else {
        return Ok(match deadline {
            Some(deadline) => child
                .wait_timeout(deadline)?
                .map_or(SyncWait::TimedOut, SyncWait::Exited),
            None => SyncWait::Exited(child.wait()?),
        });
    };

    let expiry = deadline.map(|d| std::time::Instant::now() + d);
    loop {
        if let Some(stream) = breach::stream(watch.load(Ordering::Acquire)) {
            return Ok(SyncWait::Breached(stream));
        }
        let mut tick = BREACH_POLL_INTERVAL;
        if let Some(expiry) = expiry {
            let remaining = expiry.saturating_duration_since(std::time::Instant::now());
            if remaining.is_zero() {
                return Ok(SyncWait::TimedOut);
            }
            tick = tick.min(remaining);
        }
        if let Some(status) = child.wait_timeout(tick)? {
            return Ok(SyncWait::Exited(status));
        }
    }
}

/// The per-run settings [`finish_sync_run`] needs, bundled because
/// threading them individually pushes it past clippy's argument
/// threshold.
#[cfg(feature = "sync")]
struct SyncRun<'a> {
    binary: &'a std::path::Path,
    args: &'a [String],
    working_dir: Option<&'a std::path::Path>,
    deadline: Option<Duration>,
    kill_grace: Option<Duration>,
    output_limit: Option<usize>,
}

/// Wait out a spawned blocking child whose capture threads are already
/// running, and turn the result into a [`CommandOutput`] or an error.
///
/// Shared by the plain and stdin-prompt blocking paths, which differ
/// only in what they do between spawning and waiting.
#[cfg(feature = "sync")]
fn finish_sync_run(
    child: &mut std::process::Child,
    group: &mut GroupKillGuard,
    capture: SyncCapture,
    run: SyncRun<'_>,
) -> Result<CommandOutput> {
    let waited = match wait_for_child_sync(child, run.deadline, capture.watch()) {
        Ok(waited) => waited,
        Err(source) => {
            stop_and_reap_sync(child, group, run.kill_grace);
            return Err(Error::Io {
                message: "failed to wait for claude process".to_string(),
                source,
                working_dir: run.working_dir.map(|p| p.to_path_buf()),
            });
        }
    };

    match waited {
        SyncWait::Exited(status) => {
            group.disarm();
            // A ceiling breach can still surface here when the child
            // exited in the same instant it tripped, which is why the
            // capture outcome is checked regardless of exit status.
            let (stdout, stderr) = capture.settle(run.working_dir)?;
            let exit_code = status.code().unwrap_or(-1);

            if !status.success() {
                return Err(Error::from_command_failure(
                    format!("{} {}", run.binary.display(), run.args.join(" ")),
                    exit_code,
                    stdout,
                    stderr,
                    run.working_dir.map(|p| p.to_path_buf()),
                ));
            }

            Ok(CommandOutput {
                stdout,
                stderr,
                exit_code,
                success: true,
            })
        }
        // Take down the whole group first (subprocesses may hold our
        // pipe fds), honoring the optional SIGTERM grace, then kill and
        // reap the direct child.
        SyncWait::TimedOut => {
            stop_and_reap_sync(child, group, run.kill_grace);
            log_partial(capture, "partial output from timed-out process");
            Err(Error::Timeout {
                timeout_seconds: run.deadline.unwrap_or_default().as_secs(),
            })
        }
        SyncWait::Breached(stream) => {
            stop_and_reap_sync(child, group, run.kill_grace);
            log_partial(capture, "partial output from over-limit process");
            Err(Error::OutputLimitExceeded {
                stream,
                limit_bytes: run.output_limit.unwrap_or_default(),
            })
        }
    }
}

#[cfg(feature = "sync")]
fn log_partial(capture: SyncCapture, message: &'static str) {
    let (stdout, stderr) = capture.partial(PARTIAL_CAPTURE_BUDGET);
    if !stdout.is_empty() || !stderr.is_empty() {
        warn!(stdout = %stdout, stderr = %stderr, "{message}");
    }
}

/// Kill the child's whole process group (honoring the grace) and reap
/// the direct child. Blocking mirror of [`stop_and_reap`].
#[cfg(feature = "sync")]
fn stop_and_reap_sync(
    child: &mut std::process::Child,
    group: &mut GroupKillGuard,
    grace: Option<Duration>,
) {
    kill_group_with_grace_sync(group, grace);
    let _ = child.kill();
    let _ = child.wait();
}

/// Blocking mirror of [`spawn_retrying_txtbsy`]. See that function for why
/// `ETXTBSY` is retried rather than surfaced.
#[cfg(feature = "sync")]
fn spawn_retrying_txtbsy_sync(
    cmd: &mut std::process::Command,
) -> std::io::Result<std::process::Child> {
    let start = std::time::Instant::now();
    let mut backoff = Duration::from_millis(1);
    loop {
        match cmd.spawn() {
            Err(e)
                if e.kind() == std::io::ErrorKind::ExecutableFileBusy
                    && start.elapsed() < TXTBSY_RETRY_BUDGET =>
            {
                std::thread::sleep(backoff);
                backoff = (backoff * 2).min(TXTBSY_MAX_BACKOFF);
            }
            other => return other,
        }
    }
}

/// Run `cmd` to completion, retrying on `ETXTBSY` like
/// [`spawn_retrying_txtbsy_sync`].
///
/// The blocking no-timeout capture path calls `Command::output` (spawn,
/// wait, and collect in one step) rather than holding a `Child`, so it
/// needs the same retry wrapped around `output` itself. The `ETXTBSY`
/// still occurs at the `execve` inside `output`.
#[cfg(feature = "sync")]
/// Run to completion, reporting the child to `on_spawn` first.
///
/// Wait for both capture threads to finish, returning `None` for any
/// that misses the deadline. Threads aren't cancellable in std; if the
/// child's subprocesses are still holding a pipe fd open after kill(),
/// the capture thread leaks. That's a pathological case; the common
/// timeout path with a responsive child joins in microseconds.
#[cfg(feature = "sync")]
fn join_with_deadline<T: Send + 'static>(
    stdout_thread: std::thread::JoinHandle<T>,
    stderr_thread: std::thread::JoinHandle<T>,
    budget: Duration,
) -> (Option<T>, Option<T>) {
    use std::sync::mpsc;
    use std::thread;

    let (tx, rx) = mpsc::channel::<(&'static str, T)>();

    let tx_out = tx.clone();
    let tx_err = tx;

    thread::spawn(move || {
        if let Ok(value) = stdout_thread.join() {
            let _ = tx_out.send(("stdout", value));
        }
    });
    thread::spawn(move || {
        if let Ok(value) = stderr_thread.join() {
            let _ = tx_err.send(("stderr", value));
        }
    });

    let mut stdout = None;
    let mut stderr = None;
    let deadline = std::time::Instant::now() + budget;

    for _ in 0..2 {
        let now = std::time::Instant::now();
        if now >= deadline {
            break;
        }
        match rx.recv_timeout(deadline - now) {
            Ok(("stdout", value)) => stdout = Some(value),
            Ok(("stderr", value)) => stderr = Some(value),
            Ok(_) => unreachable!(),
            Err(_) => break,
        }
    }

    (stdout, stderr)
}

// Fake-binary-driven tests for the spawn/execute paths. Unix-only: they
// write and run a small bash `claude` stand-in, which cannot execute on
// Windows. CI runs `cargo test --lib` on Windows too, so the module is
// gated on `unix` to compile out there; ubuntu/macOS (and `llvm-cov`)
// exercise it. `tempfile` is a dev-dependency, so it is always available
// under `#[cfg(test)]` regardless of the crate feature.
#[cfg(all(test, unix, any(feature = "async", feature = "sync")))]
mod tests {
    use super::*;
    use std::io::Write;
    use std::os::unix::fs::PermissionsExt;

    use crate::Claude;

    /// Write `body` as an executable bash `claude` stand-in in a fresh
    /// tempdir. Returns the dir (keep it bound so it outlives the run)
    /// and the script path.
    fn fake_script(body: &str) -> (tempfile::TempDir, std::path::PathBuf) {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("fake-claude.sh");
        // Close the writable handle before returning so the window in which a
        // concurrent test's fork could inherit a writable fd to this script
        // (and make our later execve fail with ETXTBSY) is as short as
        // possible. Spawn itself retries ETXTBSY; this just makes it rarer.
        {
            let mut f = std::fs::File::create(&path).expect("create script");
            write!(f, "#!/usr/bin/env bash\n{body}\n").expect("write script");
            f.sync_all().expect("sync script");
        }
        let perms = std::fs::Permissions::from_mode(0o755);
        std::fs::set_permissions(&path, perms).expect("chmod");
        (dir, path)
    }

    fn client(path: &std::path::Path) -> Claude {
        Claude::builder()
            .binary(path)
            .build()
            .expect("build client")
    }

    #[test]
    fn full_command_args_puts_global_args_first() {
        let claude = Claude::builder()
            .binary("/usr/local/bin/claude")
            .arg("--debug")
            .arg("--verbose")
            .build()
            .expect("build client");
        let args = full_command_args(&claude, vec!["--print".to_string(), "hi".to_string()]);
        assert_eq!(args, ["--debug", "--verbose", "--print", "hi"]);
    }

    #[test]
    fn full_command_args_without_global_args_is_passthrough() {
        let claude = Claude::builder()
            .binary("/usr/local/bin/claude")
            .build()
            .expect("build client");
        let args = full_command_args(&claude, vec!["--print".to_string()]);
        assert_eq!(args, ["--print"]);
    }

    // Serializes the env-scrub tests, which mutate process-global env.
    static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    fn set_scrub_vars() {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        // SAFETY: the synchronous mutation is serialized by ENV_LOCK and
        // not held across any await; no other test reads these vars.
        unsafe {
            std::env::set_var("CLAUDECODE", "1");
            std::env::set_var("CLAUDE_CODE_ENTRYPOINT", "cli");
        }
    }

    fn clear_scrub_vars() {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        // SAFETY: see set_scrub_vars.
        unsafe {
            std::env::remove_var("CLAUDECODE");
            std::env::remove_var("CLAUDE_CODE_ENTRYPOINT");
        }
    }

    // ---------- async ----------

    #[cfg(feature = "async")]
    #[tokio::test]
    async fn async_success_maps_output() {
        let (_dir, path) = fake_script(r#"echo "hi there"; exit 0"#);
        let out = run_claude(&client(&path), vec!["--version".into()])
            .await
            .expect("success");
        assert!(out.success);
        assert_eq!(out.exit_code, 0);
        assert!(out.stdout.contains("hi there"));
    }

    #[cfg(feature = "async")]
    #[tokio::test]
    async fn async_nonzero_exit_maps_command_failed() {
        let (_dir, path) = fake_script(r#"echo "boom" >&2; exit 3"#);
        let err = run_claude(&client(&path), vec![]).await.unwrap_err();
        match err {
            Error::CommandFailed {
                exit_code, stderr, ..
            } => {
                assert_eq!(exit_code, 3);
                assert!(stderr.contains("boom"));
            }
            other => panic!("expected CommandFailed, got {other:?}"),
        }
    }

    #[cfg(feature = "async")]
    #[tokio::test]
    async fn async_rail_stop_maps_max_turns() {
        let (_dir, path) = fake_script(
            r#"echo '{"type":"result","subtype":"error_max_turns","is_error":true,"errors":["Reached maximum number of turns (2)"]}'; exit 1"#,
        );
        let err = run_claude(&client(&path), vec![]).await.unwrap_err();
        assert!(
            matches!(
                err,
                Error::MaxTurnsExceeded {
                    max_turns: Some(2),
                    ..
                }
            ),
            "got: {err:?}"
        );
    }

    #[cfg(feature = "async")]
    #[tokio::test]
    async fn async_auth_shaped_stderr_maps_auth() {
        let (_dir, path) =
            fake_script(r#"echo "Not authenticated. Run `claude login`." >&2; exit 1"#);
        let err = run_claude(&client(&path), vec![]).await.unwrap_err();
        assert!(matches!(err, Error::Auth { .. }), "got: {err:?}");
    }

    #[cfg(feature = "async")]
    #[tokio::test]
    async fn async_scrubs_claude_env_vars() {
        let (_dir, path) =
            fake_script(r#"echo "CC=[${CLAUDECODE:-}] EP=[${CLAUDE_CODE_ENTRYPOINT:-}]""#);
        // The child sees the vars scrubbed regardless; setting them in the
        // parent is what makes the assertion meaningful rather than
        // trivially empty. Correctness does not depend on the lock (the
        // scrub removes them either way), so it only wraps the synchronous
        // env mutations -- never held across the await, per clippy.
        set_scrub_vars();
        let out = run_claude(&client(&path), vec![]).await.expect("success");
        clear_scrub_vars();
        assert!(out.stdout.contains("CC=[]"), "got: {}", out.stdout);
        assert!(out.stdout.contains("EP=[]"), "got: {}", out.stdout);
    }

    #[cfg(feature = "async")]
    #[tokio::test]
    async fn async_applies_working_dir() {
        let (_dir, path) = fake_script(r#"pwd"#);
        let workdir = tempfile::tempdir().expect("workdir");
        let claude = Claude::builder()
            .binary(&path)
            .working_dir(workdir.path())
            .build()
            .expect("build");
        let out = run_claude(&claude, vec![]).await.expect("success");
        let got = std::fs::canonicalize(out.stdout.trim()).expect("canonicalize pwd");
        let want = std::fs::canonicalize(workdir.path()).expect("canonicalize workdir");
        assert_eq!(got, want);
    }

    #[cfg(feature = "async")]
    #[tokio::test]
    async fn async_stdin_prompt_round_trips() {
        let (_dir, path) = fake_script(r#"cat"#);
        let out = run_claude_with_stdin_prompt(&client(&path), vec![], "hello via stdin".into())
            .await
            .expect("success");
        assert!(out.stdout.contains("hello via stdin"));
    }

    // The retry loop in `spawn_retrying_txtbsy` must only absorb `ETXTBSY`;
    // every other spawn error has to surface promptly rather than be retried
    // until the budget elapses. A missing binary yields `NotFound`, which
    // must return on the first attempt.
    #[cfg(feature = "async")]
    #[tokio::test]
    async fn async_spawn_retry_passes_through_non_txtbsy_error() {
        let mut cmd = Command::new("/nonexistent/definitely-not-a-real-binary");
        let err = spawn_retrying_txtbsy(&mut cmd)
            .await
            .expect_err("spawn of missing binary should fail");
        assert_eq!(err.kind(), std::io::ErrorKind::NotFound, "got: {err:?}");
    }

    #[cfg(feature = "async")]
    #[tokio::test]
    async fn async_allow_exit_codes_permits_listed_code() {
        let (_dir, path) = fake_script(r#"echo out; exit 2"#);
        let out = run_claude_allow_exit_codes(&client(&path), vec![], &[2])
            .await
            .expect("allowed code is Ok");
        assert!(!out.success);
        assert_eq!(out.exit_code, 2);
        assert!(out.stdout.contains("out"));
    }

    #[cfg(feature = "async")]
    #[tokio::test]
    async fn async_allow_exit_codes_still_errors_on_unlisted_code() {
        let (_dir, path) = fake_script(r#"exit 2"#);
        let err = run_claude_allow_exit_codes(&client(&path), vec![], &[5])
            .await
            .unwrap_err();
        assert!(
            matches!(err, Error::CommandFailed { exit_code: 2, .. }),
            "got: {err:?}"
        );
    }

    #[cfg(feature = "async")]
    #[tokio::test]
    async fn async_timeout_fires_on_slow_child() {
        let (_dir, path) = fake_script(r#"sleep 3; echo done"#);
        let claude = Claude::builder()
            .binary(&path)
            .timeout(Duration::from_millis(300))
            .build()
            .expect("build");
        let err = run_claude(&claude, vec![]).await.unwrap_err();
        assert!(matches!(err, Error::Timeout { .. }), "got: {err:?}");
    }

    #[cfg(feature = "async")]
    #[tokio::test]
    async fn async_timeout_path_returns_output_when_fast() {
        let (_dir, path) = fake_script(r#"echo quick"#);
        let claude = Claude::builder()
            .binary(&path)
            .timeout(Duration::from_secs(30))
            .build()
            .expect("build");
        let out = run_claude(&claude, vec![]).await.expect("success");
        assert!(out.stdout.contains("quick"));
    }

    #[cfg(feature = "async")]
    #[tokio::test]
    async fn async_timeout_path_maps_command_failed() {
        let (_dir, path) = fake_script(r#"echo e >&2; exit 4"#);
        let claude = Claude::builder()
            .binary(&path)
            .timeout(Duration::from_secs(30))
            .build()
            .expect("build");
        let err = run_claude(&claude, vec![]).await.unwrap_err();
        assert!(
            matches!(err, Error::CommandFailed { exit_code: 4, .. }),
            "got: {err:?}"
        );
    }

    #[cfg(feature = "async")]
    #[tokio::test]
    async fn async_stdin_with_timeout_round_trips() {
        let (_dir, path) = fake_script(r#"cat"#);
        let claude = Claude::builder()
            .binary(&path)
            .timeout(Duration::from_secs(30))
            .build()
            .expect("build");
        let out = run_claude_with_stdin_prompt(&claude, vec![], "piped under timeout".into())
            .await
            .expect("success");
        assert!(out.stdout.contains("piped under timeout"));
    }

    #[cfg(feature = "async")]
    #[tokio::test]
    async fn async_stdin_timeout_fires_on_slow_child() {
        let (_dir, path) = fake_script(r#"sleep 3"#);
        let claude = Claude::builder()
            .binary(&path)
            .timeout(Duration::from_millis(300))
            .build()
            .expect("build");
        let err = run_claude_with_stdin_prompt(&claude, vec![], "x".into())
            .await
            .unwrap_err();
        assert!(matches!(err, Error::Timeout { .. }), "got: {err:?}");
    }

    /// Drive `fut` just long enough for the fake script to write its pid
    /// file, then drop it mid-flight (on return) and hand back the pid.
    #[cfg(feature = "async")]
    async fn drop_in_flight_and_capture_pid<F>(fut: F, pid_path: &std::path::Path) -> u32
    where
        F: std::future::Future,
        F::Output: std::fmt::Debug,
    {
        tokio::pin!(fut);
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        loop {
            if let Some(pid) = std::fs::read_to_string(pid_path)
                .ok()
                .and_then(|s| s.trim().parse().ok())
            {
                // Returning drops the pinned future here, mid-flight.
                return pid;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "child never wrote its pid file"
            );
            tokio::select! {
                out = &mut fut => panic!("future completed before drop: {out:?}"),
                _ = tokio::time::sleep(Duration::from_millis(10)) => {}
            }
        }
    }

    /// Poll until `pid` is dead or a zombie awaiting reap. The kill is
    /// delivered synchronously (killpg / kill_on_drop's start_kill), but
    /// reaping happens asynchronously, so a transient zombie counts as
    /// killed. Blocking on purpose: it runs after the kill has been
    /// issued, so nothing async needs to make progress.
    fn assert_pid_killed(pid: u32) {
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        loop {
            let out = std::process::Command::new("ps")
                .args(["-o", "stat=", "-p", &pid.to_string()])
                .output()
                .expect("run ps");
            let stat = String::from_utf8_lossy(&out.stdout).trim().to_string();
            if !out.status.success() || stat.is_empty() || stat.starts_with('Z') {
                return;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "process {pid} still alive (stat {stat}) after kill"
            );
            std::thread::sleep(Duration::from_millis(25));
        }
    }

    #[cfg(feature = "async")]
    fn assert_pid_not_live_now(pid: u32) {
        let out = std::process::Command::new("ps")
            .args(["-o", "stat=", "-p", &pid.to_string()])
            .output()
            .expect("run ps");
        let stat = String::from_utf8_lossy(&out.stdout).trim().to_string();
        assert!(
            !out.status.success() || stat.is_empty() || stat.starts_with('Z'),
            "process {pid} still live after terminal settlement (stat {stat})"
        );
    }

    /// Fake script that records its own pid, spawns a same-group
    /// grandchild that records its pid too, then sleeps far longer than
    /// any test deadline. The main shell waits for the grandchild pid
    /// to land before writing its own, so tests that poll for the pid
    /// file can rely on the grandchild pid being readable as well.
    /// Non-interactive bash does not create new process groups for
    /// background jobs, so a group kill must take down both.
    fn group_script(
        pid_path: &std::path::Path,
        gpid_path: &std::path::Path,
    ) -> (tempfile::TempDir, std::path::PathBuf) {
        // `bash -c` rather than a subshell because `$$` inside a
        // subshell still names the parent, and `$BASHPID` needs bash 4
        // (macOS ships 3.2). The path travels as `$0` so it needs no
        // extra quoting.
        fake_script(&format!(
            concat!(
                "bash -c 'echo $$ > \"$0\"; exec sleep 300' \"{g}\" &\n",
                "until [[ -s \"{g}\" ]]; do sleep 0.01; done\n",
                "echo $$ > \"{p}\"\n",
                "exec sleep 300",
            ),
            g = gpid_path.display(),
            p = pid_path.display(),
        ))
    }

    /// A `claude` stand-in that prints `bytes` bytes to `stream` and
    /// exits successfully.
    fn spew_script(stream: &str, bytes: usize) -> (tempfile::TempDir, std::path::PathBuf) {
        let redirect = if stream == "stderr" { ">&2" } else { "" };
        fake_script(&format!(
            "head -c {bytes} /dev/zero | tr '\\0' 'x' {redirect}\nexit 0"
        ))
    }

    fn limited_client(path: &std::path::Path, limit: usize) -> Claude {
        Claude::builder()
            .binary(path)
            .output_limit(limit)
            .build()
            .expect("build client")
    }

    fn assert_limit_error(error: &Error, want_stream: crate::OutputStream, want_limit: usize) {
        match error {
            Error::OutputLimitExceeded {
                stream,
                limit_bytes,
            } => {
                assert_eq!(*stream, want_stream);
                assert_eq!(*limit_bytes, want_limit);
            }
            other => panic!("expected OutputLimitExceeded, got: {other:?}"),
        }
    }

    #[cfg(feature = "async")]
    #[tokio::test]
    async fn async_stdout_over_limit_is_typed_not_truncated() {
        let (_dir, path) = spew_script("stdout", 64 * 1024);
        let error = run_claude(&limited_client(&path, 4096), vec![])
            .await
            .expect_err("over-limit stdout must fail");
        assert_limit_error(&error, crate::OutputStream::Stdout, 4096);
    }

    #[cfg(feature = "async")]
    #[tokio::test]
    async fn async_stderr_over_limit_names_stderr() {
        let (_dir, path) = spew_script("stderr", 64 * 1024);
        let error = run_claude(&limited_client(&path, 4096), vec![])
            .await
            .expect_err("over-limit stderr must fail");
        assert_limit_error(&error, crate::OutputStream::Stderr, 4096);
    }

    // The ceiling is a ceiling, not a threshold: output exactly at it
    // is a complete answer and must come back as one.
    #[cfg(feature = "async")]
    #[tokio::test]
    async fn async_output_at_limit_succeeds() {
        let (_dir, path) = fake_script(r#"printf 'abcde'"#);
        let out = run_claude(&limited_client(&path, 5), vec![])
            .await
            .expect("output at the ceiling is complete");
        assert_eq!(out.stdout, "abcde");
    }

    #[cfg(feature = "async")]
    #[tokio::test]
    async fn async_unset_limit_leaves_large_output_intact() {
        let (_dir, path) = spew_script("stdout", 256 * 1024);
        let out = run_claude(&client(&path), vec![])
            .await
            .expect("no ceiling means no failure");
        assert_eq!(out.stdout.len(), 256 * 1024);
    }

    #[cfg(feature = "async")]
    #[tokio::test]
    async fn async_stdin_prompt_path_honors_limit() {
        let (_dir, path) = spew_script("stdout", 64 * 1024);
        let error = run_claude_with_stdin_prompt_sync_free(&limited_client(&path, 4096))
            .await
            .expect_err("over-limit stdout must fail on the stdin path");
        assert_limit_error(&error, crate::OutputStream::Stdout, 4096);
    }

    #[cfg(feature = "async")]
    async fn run_claude_with_stdin_prompt_sync_free(claude: &Claude) -> Result<CommandOutput> {
        run_claude_with_stdin_prompt(claude, vec![], "prompt".to_string()).await
    }

    // A breach has to settle the child the way a timeout does, or the
    // ceiling just changes which error the caller sees while the
    // runaway keeps running.
    #[cfg(feature = "async")]
    #[tokio::test]
    async fn async_limit_breach_kills_process_group() {
        let workdir = tempfile::tempdir().expect("workdir");
        let pid_path = workdir.path().join("pid");
        let gpid_path = workdir.path().join("gpid");
        let (_dir, path) = fake_script(&format!(
            concat!(
                "bash -c 'echo $$ > \"$0\"; exec sleep 300' \"{g}\" &\n",
                "until [[ -s \"{g}\" ]]; do sleep 0.01; done\n",
                "echo $$ > \"{p}\"\n",
                "head -c 65536 /dev/zero | tr '\\0' 'x'\n",
                "exec sleep 300",
            ),
            g = gpid_path.display(),
            p = pid_path.display(),
        ));
        let error = run_claude(&limited_client(&path, 4096), vec![])
            .await
            .expect_err("over-limit stdout must fail");
        assert_limit_error(&error, crate::OutputStream::Stdout, 4096);

        let pid = try_read_pid(&pid_path).expect("child recorded its pid");
        let gpid = try_read_pid(&gpid_path).expect("grandchild recorded its pid");
        assert_pid_killed(pid);
        assert_pid_killed(gpid);
    }

    #[cfg(feature = "sync")]
    #[test]
    fn sync_stdout_over_limit_is_typed_not_truncated() {
        let (_dir, path) = spew_script("stdout", 64 * 1024);
        let error = run_claude_sync(&limited_client(&path, 4096), vec![])
            .expect_err("over-limit stdout must fail");
        assert_limit_error(&error, crate::OutputStream::Stdout, 4096);
    }

    #[cfg(feature = "sync")]
    #[test]
    fn sync_stderr_over_limit_names_stderr() {
        let (_dir, path) = spew_script("stderr", 64 * 1024);
        let error = run_claude_sync(&limited_client(&path, 4096), vec![])
            .expect_err("over-limit stderr must fail");
        assert_limit_error(&error, crate::OutputStream::Stderr, 4096);
    }

    #[cfg(feature = "sync")]
    #[test]
    fn sync_output_at_limit_succeeds() {
        let (_dir, path) = fake_script(r#"printf 'abcde'"#);
        let out = run_claude_sync(&limited_client(&path, 5), vec![])
            .expect("output at the ceiling is complete");
        assert_eq!(out.stdout, "abcde");
    }

    #[cfg(feature = "sync")]
    #[test]
    fn sync_unset_limit_leaves_large_output_intact() {
        let (_dir, path) = spew_script("stdout", 256 * 1024);
        let out = run_claude_sync(&client(&path), vec![]).expect("no ceiling means no failure");
        assert_eq!(out.stdout.len(), 256 * 1024);
    }

    #[cfg(feature = "sync")]
    #[test]
    fn sync_stdin_prompt_path_honors_limit() {
        let (_dir, path) = spew_script("stdout", 64 * 1024);
        let error =
            run_claude_with_stdin_prompt_sync(&limited_client(&path, 4096), vec![], "p".into())
                .expect_err("over-limit stdout must fail on the stdin path");
        assert_limit_error(&error, crate::OutputStream::Stdout, 4096);
    }

    // The blocking waiter polls for a breach rather than being woken,
    // so this also covers the child being unblocked from a full pipe on
    // a path that has no deadline of its own.
    #[cfg(feature = "sync")]
    #[test]
    fn sync_limit_breach_terminates_a_child_with_no_deadline() {
        let (_dir, path) = fake_script(concat!(
            "head -c 65536 /dev/zero | tr '\\0' 'x'\n",
            "exec sleep 300",
        ));
        let started = std::time::Instant::now();
        let error = run_claude_sync(&limited_client(&path, 4096), vec![])
            .expect_err("over-limit stdout must fail");
        assert_limit_error(&error, crate::OutputStream::Stdout, 4096);
        assert!(
            started.elapsed() < Duration::from_secs(30),
            "breach must not wait out the child, took {:?}",
            started.elapsed(),
        );
    }

    /// Read a pid recorded by `group_script`, if fully written yet.
    fn try_read_pid(path: &std::path::Path) -> Option<u32> {
        std::fs::read_to_string(path).ok()?.trim().parse().ok()
    }

    /// Read a pid recorded by `group_script`. Only the async tests use
    /// this unconditional variant; the sync timeout test reads through
    /// `try_read_pid`, so gate it to keep sync-only builds warning-free.
    #[cfg(feature = "async")]
    fn read_pid(path: &std::path::Path) -> u32 {
        try_read_pid(path).expect("pid file readable")
    }

    // Dropping an in-flight execute future must kill the spawned child:
    // every async spawn site sets kill_on_drop(true), so a caller racing
    // execute against cancellation (tokio::select!, timeout) cannot leak
    // a headless CLI run. `exec` keeps the recorded pid the direct child,
    // so the SIGKILL lands on the process the test watches.
    #[cfg(feature = "async")]
    #[tokio::test]
    async fn async_dropping_in_flight_future_kills_child() {
        let workdir = tempfile::tempdir().expect("workdir");
        let pid_path = workdir.path().join("pid");
        let (_dir, path) = fake_script(&format!(
            r#"echo $$ > "{}"; exec sleep 30"#,
            pid_path.display()
        ));
        let claude = client(&path);
        let pid = drop_in_flight_and_capture_pid(run_claude(&claude, vec![]), &pid_path).await;
        assert_pid_killed(pid);
    }

    // Same guarantee on the timeout path, which holds a Child from
    // spawn_retrying_txtbsy rather than going through Command::output.
    // The configured timeout is far longer than the test; the drop is
    // what kills the child.
    #[cfg(feature = "async")]
    #[tokio::test]
    async fn async_dropping_in_flight_future_kills_child_with_timeout() {
        let workdir = tempfile::tempdir().expect("workdir");
        let pid_path = workdir.path().join("pid");
        let (_dir, path) = fake_script(&format!(
            r#"echo $$ > "{}"; exec sleep 30"#,
            pid_path.display()
        ));
        let claude = Claude::builder()
            .binary(&path)
            .timeout(Duration::from_secs(120))
            .build()
            .expect("build");
        let pid = drop_in_flight_and_capture_pid(run_claude(&claude, vec![]), &pid_path).await;
        assert_pid_killed(pid);
    }

    // Dropping the future must kill the child's whole process group,
    // not just the direct child: the CLI spawns subprocesses for tool
    // use, and a cancelled run must leave none of them behind.
    #[cfg(feature = "async")]
    #[tokio::test]
    async fn async_dropping_in_flight_future_kills_process_group() {
        let workdir = tempfile::tempdir().expect("workdir");
        let pid_path = workdir.path().join("pid");
        let gpid_path = workdir.path().join("gpid");
        let (_dir, path) = group_script(&pid_path, &gpid_path);
        let claude = client(&path);
        let pid = drop_in_flight_and_capture_pid(run_claude(&claude, vec![]), &pid_path).await;
        assert_pid_killed(pid);
        assert_pid_killed(read_pid(&gpid_path));
    }

    // A fired timeout must also kill the whole group. Before the group
    // kill, the timeout path SIGKILLed only the direct child and the
    // grandchild survived. Retries on a heavily loaded host, where the
    // child can get killed before it records its pids: the kill still
    // happened, but there is nothing to observe, so run it again.
    #[cfg(feature = "async")]
    #[tokio::test]
    async fn async_timeout_kills_process_group() {
        let mut observed = false;
        for _ in 0..5 {
            let workdir = tempfile::tempdir().expect("workdir");
            let pid_path = workdir.path().join("pid");
            let gpid_path = workdir.path().join("gpid");
            let (_dir, path) = group_script(&pid_path, &gpid_path);
            let claude = Claude::builder()
                .binary(&path)
                .timeout(Duration::from_millis(1000))
                .build()
                .expect("build");
            let err = run_claude(&claude, vec![]).await.unwrap_err();
            assert!(matches!(err, Error::Timeout { .. }), "got: {err:?}");
            if let (Some(pid), Some(gpid)) = (try_read_pid(&pid_path), try_read_pid(&gpid_path)) {
                assert_pid_killed(pid);
                assert_pid_killed(gpid);
                observed = true;
                break;
            }
        }
        assert!(observed, "child never recorded pids within 5 timeout runs");
    }

    #[cfg(feature = "async")]
    #[tokio::test]
    async fn async_explicit_cancellation_settles_process_group() {
        let workdir = tempfile::tempdir().expect("workdir");
        let pid_path = workdir.path().join("pid");
        let gpid_path = workdir.path().join("gpid");
        let (_dir, path) = group_script(&pid_path, &gpid_path);
        let claude = Claude::builder()
            .binary(&path)
            .kill_grace(Duration::from_millis(10))
            .build()
            .expect("build");
        let cancel = async {
            while !pid_path.exists() || !gpid_path.exists() {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        };

        let error = run_claude_cancellable(&claude, vec![], cancel)
            .await
            .expect_err("run must be cancelled");
        assert!(matches!(error, Error::Cancelled));
        assert_pid_not_live_now(read_pid(&pid_path));
        assert_pid_not_live_now(read_pid(&gpid_path));
    }

    #[cfg(feature = "async")]
    #[tokio::test]
    async fn async_stdin_failure_settles_process_group() {
        let workdir = tempfile::tempdir().expect("workdir");
        let pid_path = workdir.path().join("pid");
        let gpid_path = workdir.path().join("gpid");
        let (_dir, path) = fake_script(&format!(
            concat!(
                "sleep 300 </dev/null &\n",
                "child=$!\n",
                "echo $$ > '{p}'\n",
                "echo $child > '{g}'\n",
                "exec 0<&-\n",
                "wait $child",
            ),
            p = pid_path.display(),
            g = gpid_path.display(),
        ));
        let claude = Claude::builder()
            .binary(&path)
            .kill_grace(Duration::from_millis(10))
            .build()
            .expect("build");

        let error = run_claude_with_stdin_prompt(&claude, vec![], "x".repeat(4 * 1024 * 1024))
            .await
            .expect_err("stdin write must fail");
        assert!(matches!(error, Error::Io { .. }));
        assert_pid_not_live_now(read_pid(&pid_path));
        assert_pid_not_live_now(read_pid(&gpid_path));
    }

    // With the process-group split opted out, dropping the future still
    // kills the direct child via kill_on_drop, but the grandchild is
    // deliberately left running: that is the pre-group contract #767
    // preserves for terminal-attached hosts, where the terminal is the
    // supervisor. The test reaps the survivor itself.
    #[cfg(feature = "async")]
    #[tokio::test]
    async fn async_process_group_opt_out_kills_only_direct_child() {
        let workdir = tempfile::tempdir().expect("workdir");
        let pid_path = workdir.path().join("pid");
        let gpid_path = workdir.path().join("gpid");
        let (_dir, path) = group_script(&pid_path, &gpid_path);
        let claude = Claude::builder()
            .binary(&path)
            .process_group(false)
            .build()
            .expect("build");
        let pid = drop_in_flight_and_capture_pid(run_claude(&claude, vec![]), &pid_path).await;
        assert_pid_killed(pid);

        // The grandchild must still be alive: no group kill happened.
        let gpid = read_pid(&gpid_path);
        let out = std::process::Command::new("ps")
            .args(["-o", "stat=", "-p", &gpid.to_string()])
            .output()
            .expect("run ps");
        let stat = String::from_utf8_lossy(&out.stdout).trim().to_string();
        assert!(
            out.status.success() && !stat.is_empty() && !stat.starts_with('Z'),
            "grandchild {gpid} should have survived the opt-out drop (stat {stat:?})"
        );

        // Reap the deliberate survivor so it does not idle for 300s.
        let _ = std::process::Command::new("kill")
            .args(["-9", &gpid.to_string()])
            .status();
    }

    /// Fake script that traps SIGTERM, records a marker, and exits
    /// cleanly. The marker can only exist if TERM arrived before the
    /// KILL: SIGKILL cannot be trapped. `sleep` runs as a background
    /// job with `wait` so bash stays alive to handle the signal
    /// (an `exec sleep` would replace bash and drop the trap).
    fn term_trap_script(marker: &std::path::Path) -> (tempfile::TempDir, std::path::PathBuf) {
        fake_script(&format!(
            concat!(
                "trap 'echo term > \"{m}\"; exit 0' TERM\n",
                "sleep 300 &\n",
                "wait $!",
            ),
            m = marker.display(),
        ))
    }

    // With a kill grace configured, a fired timeout SIGTERMs the group
    // before the SIGKILL, giving the child a chance to flush. Retries
    // on a heavily loaded host where the child is killed before it
    // installs its trap: the kill still happened, but there is nothing
    // to observe, so run it again.
    #[cfg(feature = "async")]
    #[tokio::test]
    async fn async_timeout_with_grace_delivers_sigterm_first() {
        let mut observed = false;
        for _ in 0..5 {
            let workdir = tempfile::tempdir().expect("workdir");
            let marker = workdir.path().join("term-marker");
            let (_dir, path) = term_trap_script(&marker);
            let claude = Claude::builder()
                .binary(&path)
                .timeout(Duration::from_millis(500))
                .kill_grace(Duration::from_secs(1))
                .build()
                .expect("build");
            let err = run_claude(&claude, vec![]).await.unwrap_err();
            assert!(matches!(err, Error::Timeout { .. }), "got: {err:?}");
            if marker.exists() {
                observed = true;
                break;
            }
        }
        assert!(observed, "TERM marker never appeared within 5 timeout runs");
    }

    // Blocking mirror of async_timeout_with_grace_delivers_sigterm_first.
    #[cfg(feature = "sync")]
    #[test]
    fn sync_timeout_with_grace_delivers_sigterm_first() {
        let mut observed = false;
        for _ in 0..5 {
            let workdir = tempfile::tempdir().expect("workdir");
            let marker = workdir.path().join("term-marker");
            let (_dir, path) = term_trap_script(&marker);
            let claude = Claude::builder()
                .binary(&path)
                .timeout(Duration::from_millis(500))
                .kill_grace(Duration::from_secs(1))
                .build()
                .expect("build");
            let err = run_claude_sync(&claude, vec![]).unwrap_err();
            assert!(matches!(err, Error::Timeout { .. }), "got: {err:?}");
            if marker.exists() {
                observed = true;
                break;
            }
        }
        assert!(observed, "TERM marker never appeared within 5 timeout runs");
    }

    // Same guarantee for the stdin-prompt path.
    #[cfg(feature = "async")]
    #[tokio::test]
    async fn async_dropping_in_flight_stdin_future_kills_child() {
        let workdir = tempfile::tempdir().expect("workdir");
        let pid_path = workdir.path().join("pid");
        let (_dir, path) = fake_script(&format!(
            r#"echo $$ > "{}"; exec sleep 30"#,
            pid_path.display()
        ));
        let claude = client(&path);
        let pid = drop_in_flight_and_capture_pid(
            run_claude_with_stdin_prompt(&claude, vec![], "x".into()),
            &pid_path,
        )
        .await;
        assert_pid_killed(pid);
    }

    #[cfg(feature = "async")]
    #[tokio::test]
    async fn async_spawn_failure_maps_io() {
        let claude = Claude::builder()
            .binary("/nonexistent/definitely/not/here")
            .build()
            .expect("build");
        let err = run_claude(&claude, vec![]).await.unwrap_err();
        assert!(matches!(err, Error::Io { .. }), "got: {err:?}");
    }

    // ---------- sync ----------

    #[cfg(feature = "sync")]
    #[test]
    fn sync_success_maps_output() {
        let (_dir, path) = fake_script(r#"echo "hi sync"; exit 0"#);
        let out = run_claude_sync(&client(&path), vec![]).expect("success");
        assert!(out.success);
        assert!(out.stdout.contains("hi sync"));
    }

    #[cfg(feature = "sync")]
    #[test]
    fn sync_nonzero_exit_maps_command_failed() {
        let (_dir, path) = fake_script(r#"echo "boom" >&2; exit 3"#);
        let err = run_claude_sync(&client(&path), vec![]).unwrap_err();
        match err {
            Error::CommandFailed {
                exit_code, stderr, ..
            } => {
                assert_eq!(exit_code, 3);
                assert!(stderr.contains("boom"));
            }
            other => panic!("expected CommandFailed, got {other:?}"),
        }
    }

    #[cfg(feature = "sync")]
    #[test]
    fn sync_scrubs_claude_env_vars() {
        let (_dir, path) =
            fake_script(r#"echo "CC=[${CLAUDECODE:-}] EP=[${CLAUDE_CODE_ENTRYPOINT:-}]""#);
        set_scrub_vars();
        let out = run_claude_sync(&client(&path), vec![]).expect("success");
        clear_scrub_vars();
        assert!(out.stdout.contains("CC=[]"), "got: {}", out.stdout);
        assert!(out.stdout.contains("EP=[]"), "got: {}", out.stdout);
    }

    #[cfg(feature = "sync")]
    #[test]
    fn sync_stdin_prompt_round_trips() {
        let (_dir, path) = fake_script(r#"cat"#);
        let out = run_claude_with_stdin_prompt_sync(&client(&path), vec![], "sync stdin".into())
            .expect("success");
        assert!(out.stdout.contains("sync stdin"));
    }

    // Sync mirror: only `ETXTBSY` is retried; a missing binary must surface
    // `NotFound` on the first attempt.
    #[cfg(feature = "sync")]
    #[test]
    fn sync_spawn_retry_passes_through_non_txtbsy_error() {
        let mut cmd = std::process::Command::new("/nonexistent/definitely-not-a-real-binary");
        let err =
            spawn_retrying_txtbsy_sync(&mut cmd).expect_err("spawn of missing binary should fail");
        assert_eq!(err.kind(), std::io::ErrorKind::NotFound, "got: {err:?}");
    }

    #[cfg(feature = "sync")]
    #[test]
    fn sync_allow_exit_codes_permits_listed_code() {
        let (_dir, path) = fake_script(r#"echo out; exit 2"#);
        let out = run_claude_allow_exit_codes_sync(&client(&path), vec![], &[2])
            .expect("allowed code is Ok");
        assert!(!out.success);
        assert_eq!(out.exit_code, 2);
    }

    #[cfg(feature = "sync")]
    #[test]
    fn sync_timeout_fires_on_slow_child() {
        let (_dir, path) = fake_script(r#"sleep 3; echo done"#);
        let claude = Claude::builder()
            .binary(&path)
            .timeout(Duration::from_millis(300))
            .build()
            .expect("build");
        let err = run_claude_sync(&claude, vec![]).unwrap_err();
        assert!(matches!(err, Error::Timeout { .. }), "got: {err:?}");
    }

    // Blocking mirror of async_timeout_kills_process_group: a fired
    // timeout on the sync path must kill the whole group too. Same
    // retry rationale as the async variant.
    #[cfg(feature = "sync")]
    #[test]
    fn sync_timeout_kills_process_group() {
        let mut observed = false;
        for _ in 0..5 {
            let workdir = tempfile::tempdir().expect("workdir");
            let pid_path = workdir.path().join("pid");
            let gpid_path = workdir.path().join("gpid");
            let (_dir, path) = group_script(&pid_path, &gpid_path);
            let claude = Claude::builder()
                .binary(&path)
                .timeout(Duration::from_millis(1000))
                .build()
                .expect("build");
            let err = run_claude_sync(&claude, vec![]).unwrap_err();
            assert!(matches!(err, Error::Timeout { .. }), "got: {err:?}");
            if let (Some(pid), Some(gpid)) = (try_read_pid(&pid_path), try_read_pid(&gpid_path)) {
                assert_pid_killed(pid);
                assert_pid_killed(gpid);
                observed = true;
                break;
            }
        }
        assert!(observed, "child never recorded pids within 5 timeout runs");
    }

    #[cfg(feature = "sync")]
    #[test]
    fn sync_timeout_path_returns_output_when_fast() {
        let (_dir, path) = fake_script(r#"echo quick"#);
        let claude = Claude::builder()
            .binary(&path)
            .timeout(Duration::from_secs(30))
            .build()
            .expect("build");
        let out = run_claude_sync(&claude, vec![]).expect("success");
        assert!(out.stdout.contains("quick"));
    }

    #[cfg(feature = "sync")]
    #[test]
    fn sync_stdin_with_timeout_round_trips() {
        let (_dir, path) = fake_script(r#"cat"#);
        let claude = Claude::builder()
            .binary(&path)
            .timeout(Duration::from_secs(30))
            .build()
            .expect("build");
        let out = run_claude_with_stdin_prompt_sync(&claude, vec![], "sync piped".into())
            .expect("success");
        assert!(out.stdout.contains("sync piped"));
    }
}
