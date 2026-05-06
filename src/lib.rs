//! A type-safe Claude Code CLI wrapper for Rust.
//!
//! `claude-wrapper` provides a builder-pattern interface for invoking the
//! `claude` CLI programmatically. Each subcommand is a typed builder that
//! produces typed output. The design follows the same shape as
//! [`docker-wrapper`](https://crates.io/crates/docker-wrapper) and
//! [`terraform-wrapper`](https://crates.io/crates/terraform-wrapper).
//!
//! # Feature flags
//!
//! | Feature | Default | Purpose |
//! |---|---|---|
//! | `async` | yes | tokio-backed async API. Disabling drops tokio from the runtime dep tree. |
//! | `json` | yes | JSON output parsing ([`QueryCommand::execute_json`], [`streaming::StreamEvent`], [`session::Session`], [`streaming::stream_query`]). |
//! | `tempfile` | yes | [`TempMcpConfig`] for one-shot MCP config files. |
//! | `sync` | no | Blocking API: `*_sync` methods on [`exec`], [`retry`], every command builder, and [`Claude`]. |
//!
//! Sync-only (tokio-free) build:
//!
//! ```toml
//! claude-wrapper = { version = "0.6", default-features = false, features = ["json", "sync"] }
//! ```
//!
//! # Quick start (async)
//!
//! ```no_run
//! # #[cfg(feature = "async")] {
//! use claude_wrapper::{Claude, ClaudeCommand, QueryCommand};
//!
//! # async fn example() -> claude_wrapper::Result<()> {
//! let claude = Claude::builder().build()?;
//! let output = QueryCommand::new("explain this error: file not found")
//!     .model("sonnet")
//!     .execute(&claude)
//!     .await?;
//! println!("{}", output.stdout);
//! # Ok(()) }
//! # }
//! ```
//!
//! # Quick start (sync)
//!
//! Enable the `sync` feature and bring [`ClaudeCommandSyncExt`] into scope:
//!
//! ```no_run
//! # #[cfg(feature = "sync")] {
//! use claude_wrapper::{Claude, ClaudeCommandSyncExt, QueryCommand};
//!
//! # fn example() -> claude_wrapper::Result<()> {
//! let claude = Claude::builder().build()?;
//! let output = QueryCommand::new("explain this error")
//!     .execute_sync(&claude)?;
//! println!("{}", output.stdout);
//! # Ok(()) }
//! # }
//! ```
//!
//! # Two-layer builder
//!
//! The [`Claude`] client holds shared config (binary path, env, timeout,
//! default retry policy). Command builders hold per-invocation options
//! and call `execute(&claude)` (or `execute_sync`).
//!
//! ```no_run
//! # #[cfg(feature = "async")] {
//! use claude_wrapper::{Claude, ClaudeCommand, Effort, PermissionMode, QueryCommand};
//!
//! # async fn example() -> claude_wrapper::Result<()> {
//! let claude = Claude::builder()
//!     .env("AWS_REGION", "us-west-2")
//!     .timeout_secs(300)
//!     .build()?;
//!
//! let output = QueryCommand::new("review src/main.rs")
//!     .model("opus")
//!     .system_prompt("You are a senior Rust developer")
//!     .permission_mode(PermissionMode::Plan)
//!     .effort(Effort::High)
//!     .max_turns(5)
//!     .no_session_persistence()
//!     .execute(&claude)
//!     .await?;
//! # Ok(()) }
//! # }
//! ```
//!
//! # JSON output
//!
//! ```no_run
//! # #[cfg(all(feature = "async", feature = "json"))] {
//! use claude_wrapper::{Claude, QueryCommand};
//!
//! # async fn example() -> claude_wrapper::Result<()> {
//! let claude = Claude::builder().build()?;
//! let result = QueryCommand::new("what is 2+2?")
//!     .execute_json(&claude)
//!     .await?;
//! println!("answer: {}", result.result);
//! println!("cost: ${:.4}", result.cost_usd.unwrap_or(0.0));
//! # Ok(()) }
//! # }
//! ```
//!
//! # Session management
//!
//! For multi-turn conversations use [`session::Session`]. It threads the
//! CLI `session_id` across turns, tracks cumulative cost + history, and
//! supports streaming. See the [session module docs](session) for the
//! full API.
//!
//! ```no_run
//! # #[cfg(all(feature = "async", feature = "json"))] {
//! use std::sync::Arc;
//! use claude_wrapper::Claude;
//! use claude_wrapper::session::Session;
//!
//! # async fn example() -> claude_wrapper::Result<()> {
//! let claude = Arc::new(Claude::builder().build()?);
//! let mut session = Session::new(claude);
//! let _first = session.send("what's 2 + 2?").await?;
//! let _second = session.send("and squared?").await?;
//! println!("cost: ${:.4}", session.total_cost_usd());
//! # Ok(()) }
//! # }
//! ```
//!
//! # Budget tracking
//!
//! Attach a [`BudgetTracker`] to a session (or share one across several
//! sessions) to enforce a cumulative USD ceiling. Callbacks fire
//! exactly once when thresholds are crossed; pre-turn checks
//! short-circuit with [`Error::BudgetExceeded`]
//! once the ceiling is hit.
//!
//! ```no_run
//! # #[cfg(all(feature = "async", feature = "json"))] {
//! use std::sync::Arc;
//! use claude_wrapper::{BudgetTracker, Claude};
//! use claude_wrapper::session::Session;
//!
//! # async fn example() -> claude_wrapper::Result<()> {
//! let budget = BudgetTracker::builder()
//!     .max_usd(5.00)
//!     .warn_at_usd(4.00)
//!     .on_warning(|t| eprintln!("warning: ${t:.2}"))
//!     .on_exceeded(|t| eprintln!("budget hit: ${t:.2}"))
//!     .build();
//!
//! let claude = Arc::new(Claude::builder().build()?);
//! let mut session = Session::new(claude).with_budget(budget.clone());
//! session.send("hello").await?;
//! println!("spent: ${:.4}", budget.total_usd());
//! # Ok(()) }
//! # }
//! ```
//!
//! # Tool permissions
//!
//! Use [`ToolPattern`] for typed `--allowed-tools` / `--disallowed-tools`
//! entries. Typed constructors always produce valid patterns; loose
//! `From<&str>` keeps bare strings working for back-compat.
//!
//! ```
//! use claude_wrapper::{QueryCommand, ToolPattern};
//!
//! let cmd = QueryCommand::new("review")
//!     .allowed_tool(ToolPattern::tool("Read"))
//!     .allowed_tool(ToolPattern::tool_with_args("Bash", "git log:*"))
//!     .allowed_tool(ToolPattern::all("Write"))
//!     .allowed_tool(ToolPattern::mcp("my-server", "*"))
//!     .disallowed_tool(ToolPattern::tool_with_args("Bash", "rm*"));
//! ```
//!
//! # Streaming
//!
//! Process NDJSON events in real time with [`streaming::stream_query`]
//! (async) or [`streaming::stream_query_sync`] (blocking; non-`Send`
//! handler supported).
//!
//! ```no_run
//! # #[cfg(all(feature = "async", feature = "json"))] {
//! use claude_wrapper::{Claude, OutputFormat, QueryCommand};
//! use claude_wrapper::streaming::{StreamEvent, stream_query};
//!
//! # async fn example() -> claude_wrapper::Result<()> {
//! let claude = Claude::builder().build()?;
//! let cmd = QueryCommand::new("explain quicksort")
//!     .output_format(OutputFormat::StreamJson);
//!
//! stream_query(&claude, &cmd, |event: StreamEvent| {
//!     if event.is_result() {
//!         println!("result: {}", event.result_text().unwrap_or(""));
//!     }
//! }).await?;
//! # Ok(()) }
//! # }
//! ```
//!
//! # MCP config generation
//!
//! Generate `.mcp.json` files for `--mcp-config`:
//!
//! ```no_run
//! # #[cfg(feature = "async")] {
//! use claude_wrapper::{Claude, ClaudeCommand, McpConfigBuilder, QueryCommand};
//!
//! # async fn example() -> claude_wrapper::Result<()> {
//! McpConfigBuilder::new()
//!     .http_server("hub", "http://127.0.0.1:9090")
//!     .stdio_server("tool", "npx", ["my-server"])
//!     .write_to("/tmp/my-project/.mcp.json")?;
//!
//! let claude = Claude::builder().build()?;
//! QueryCommand::new("list tools")
//!     .mcp_config("/tmp/my-project/.mcp.json")
//!     .execute(&claude)
//!     .await?;
//! # Ok(()) }
//! # }
//! ```
//!
//! # Dangerous: bypass mode
//!
//! `--permission-mode bypassPermissions` is isolated behind
//! [`dangerous::DangerousClient`], which requires an env-var
//! acknowledgement ([`dangerous::ALLOW_ENV`] = `"1"`) at process start.
//! See the [dangerous module docs](dangerous) for details.
//!
//! # Escape hatch
//!
//! For subcommands not yet wrapped, use [`RawCommand`]:
//!
//! ```no_run
//! # #[cfg(feature = "async")] {
//! use claude_wrapper::{Claude, ClaudeCommand, RawCommand};
//!
//! # async fn example() -> claude_wrapper::Result<()> {
//! let claude = Claude::builder().build()?;
//! let output = RawCommand::new("some-future-command")
//!     .arg("--new-flag")
//!     .arg("value")
//!     .execute(&claude)
//!     .await?;
//! # Ok(()) }
//! # }
//! ```

pub mod budget;
pub mod command;
pub mod dangerous;
#[cfg(all(feature = "json", feature = "async"))]
pub mod duplex;
pub mod error;
pub mod exec;
pub mod mcp_config;
pub mod retry;
#[cfg(all(feature = "json", feature = "async"))]
pub mod session;
pub mod streaming;
pub mod tool_pattern;
pub mod types;
pub mod version;

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

pub use budget::{BudgetBuilder, BudgetTracker};
pub use command::ClaudeCommand;
#[cfg(feature = "sync")]
pub use command::ClaudeCommandSyncExt;
pub use command::agents::AgentsCommand;
pub use command::auth::{
    AuthLoginCommand, AuthLogoutCommand, AuthStatusCommand, SetupTokenCommand,
};
pub use command::auto_mode::{
    AutoModeConfigCommand, AutoModeCritiqueCommand, AutoModeDefaultsCommand,
};
pub use command::doctor::DoctorCommand;
pub use command::install::InstallCommand;
pub use command::marketplace::{
    MarketplaceAddCommand, MarketplaceListCommand, MarketplaceRemoveCommand,
    MarketplaceUpdateCommand,
};
pub use command::mcp::{
    McpAddCommand, McpAddFromDesktopCommand, McpAddJsonCommand, McpGetCommand, McpListCommand,
    McpRemoveCommand, McpResetProjectChoicesCommand, McpServeCommand,
};
pub use command::plugin::{
    PluginDisableCommand, PluginEnableCommand, PluginInstallCommand, PluginListCommand,
    PluginTagCommand, PluginUninstallCommand, PluginUpdateCommand, PluginValidateCommand,
};
pub use command::query::QueryCommand;
pub use command::raw::RawCommand;
pub use command::update::UpdateCommand;
pub use command::version::VersionCommand;
#[cfg(all(feature = "json", feature = "async"))]
pub use duplex::{DuplexOptions, DuplexSession, TurnResult};
pub use error::{Error, Result};
pub use exec::CommandOutput;
#[cfg(feature = "tempfile")]
pub use mcp_config::TempMcpConfig;
pub use mcp_config::{McpConfigBuilder, McpServerConfig};
pub use retry::{BackoffStrategy, RetryPolicy};
#[cfg(all(feature = "json", feature = "async"))]
pub use session::Session;
pub use tool_pattern::{PatternError, ToolPattern};
pub use types::*;
pub use version::{CliVersion, VersionParseError};

/// The Claude CLI client. Holds shared configuration applied to all commands.
///
/// Create one via [`Claude::builder()`] and reuse it across commands.
#[derive(Debug, Clone)]
pub struct Claude {
    pub(crate) binary: PathBuf,
    pub(crate) working_dir: Option<PathBuf>,
    pub(crate) env: HashMap<String, String>,
    pub(crate) global_args: Vec<String>,
    pub(crate) timeout: Option<Duration>,
    pub(crate) retry_policy: Option<RetryPolicy>,
}

impl Claude {
    /// Create a new builder for configuring the Claude client.
    #[must_use]
    pub fn builder() -> ClaudeBuilder {
        ClaudeBuilder::default()
    }

    /// Get the path to the claude binary.
    #[must_use]
    pub fn binary(&self) -> &Path {
        &self.binary
    }

    /// Get the working directory, if set.
    #[must_use]
    pub fn working_dir(&self) -> Option<&Path> {
        self.working_dir.as_deref()
    }

    /// Create a clone of this client with a different working directory.
    #[must_use]
    pub fn with_working_dir(&self, dir: impl Into<PathBuf>) -> Self {
        let mut clone = self.clone();
        clone.working_dir = Some(dir.into());
        clone
    }

    /// Query the installed CLI version.
    ///
    /// Runs `claude --version` and parses the output into a [`CliVersion`].
    ///
    /// # Example
    ///
    /// ```no_run
    /// # async fn example() -> claude_wrapper::Result<()> {
    /// let claude = claude_wrapper::Claude::builder().build()?;
    /// let version = claude.cli_version().await?;
    /// println!("Claude CLI {version}");
    /// # Ok(())
    /// # }
    /// ```
    #[cfg(feature = "async")]
    pub async fn cli_version(&self) -> Result<CliVersion> {
        let output = VersionCommand::new().execute(self).await?;
        CliVersion::parse_version_output(&output.stdout).map_err(|e| Error::Io {
            message: format!("failed to parse CLI version: {e}"),
            source: std::io::Error::new(std::io::ErrorKind::InvalidData, e.to_string()),
            working_dir: None,
        })
    }

    /// Check that the installed CLI version meets a minimum requirement.
    ///
    /// Returns the detected version on success, or an error if the version
    /// is below the minimum.
    ///
    /// # Example
    ///
    /// ```no_run
    /// use claude_wrapper::CliVersion;
    ///
    /// # async fn example() -> claude_wrapper::Result<()> {
    /// let claude = claude_wrapper::Claude::builder().build()?;
    /// let version = claude.check_version(&CliVersion::new(2, 1, 0)).await?;
    /// println!("CLI version {version} meets minimum requirement");
    /// # Ok(())
    /// # }
    /// ```
    #[cfg(feature = "async")]
    pub async fn check_version(&self, minimum: &CliVersion) -> Result<CliVersion> {
        let version = self.cli_version().await?;
        if version.satisfies_minimum(minimum) {
            Ok(version)
        } else {
            Err(Error::VersionMismatch {
                found: version,
                minimum: *minimum,
            })
        }
    }

    /// Blocking mirror of [`Claude::cli_version`]. Requires the
    /// `sync` feature.
    #[cfg(feature = "sync")]
    pub fn cli_version_sync(&self) -> Result<CliVersion> {
        let output = VersionCommand::new().execute_sync(self)?;
        CliVersion::parse_version_output(&output.stdout).map_err(|e| Error::Io {
            message: format!("failed to parse CLI version: {e}"),
            source: std::io::Error::new(std::io::ErrorKind::InvalidData, e.to_string()),
            working_dir: None,
        })
    }

    /// Blocking mirror of [`Claude::check_version`]. Requires the
    /// `sync` feature.
    #[cfg(feature = "sync")]
    pub fn check_version_sync(&self, minimum: &CliVersion) -> Result<CliVersion> {
        let version = self.cli_version_sync()?;
        if version.satisfies_minimum(minimum) {
            Ok(version)
        } else {
            Err(Error::VersionMismatch {
                found: version,
                minimum: *minimum,
            })
        }
    }
}

/// Builder for creating a [`Claude`] client.
///
/// # Example
///
/// ```no_run
/// use claude_wrapper::Claude;
///
/// # fn example() -> claude_wrapper::Result<()> {
/// let claude = Claude::builder()
///     .env("AWS_REGION", "us-west-2")
///     .timeout_secs(120)
///     .build()?;
/// # Ok(())
/// # }
/// ```
#[derive(Debug, Default)]
pub struct ClaudeBuilder {
    binary: Option<PathBuf>,
    working_dir: Option<PathBuf>,
    env: HashMap<String, String>,
    global_args: Vec<String>,
    timeout: Option<Duration>,
    retry_policy: Option<RetryPolicy>,
}

impl ClaudeBuilder {
    /// Set the path to the claude binary.
    ///
    /// If not set, the binary is resolved from PATH using `which`.
    #[must_use]
    pub fn binary(mut self, path: impl Into<PathBuf>) -> Self {
        self.binary = Some(path.into());
        self
    }

    /// Set the working directory for all commands.
    ///
    /// The spawned process will use this as its current directory.
    #[must_use]
    pub fn working_dir(mut self, path: impl Into<PathBuf>) -> Self {
        self.working_dir = Some(path.into());
        self
    }

    /// Add an environment variable to pass to all commands.
    #[must_use]
    pub fn env(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.env.insert(key.into(), value.into());
        self
    }

    /// Add multiple environment variables.
    #[must_use]
    pub fn envs(
        mut self,
        vars: impl IntoIterator<Item = (impl Into<String>, impl Into<String>)>,
    ) -> Self {
        for (k, v) in vars {
            self.env.insert(k.into(), v.into());
        }
        self
    }

    /// Set a default timeout for all commands (in seconds).
    #[must_use]
    pub fn timeout_secs(mut self, seconds: u64) -> Self {
        self.timeout = Some(Duration::from_secs(seconds));
        self
    }

    /// Set a default timeout for all commands.
    #[must_use]
    pub fn timeout(mut self, duration: Duration) -> Self {
        self.timeout = Some(duration);
        self
    }

    /// Add a global argument applied to all commands.
    ///
    /// This is an escape hatch for flags not yet covered by the API.
    #[must_use]
    pub fn arg(mut self, arg: impl Into<String>) -> Self {
        self.global_args.push(arg.into());
        self
    }

    /// Enable verbose output for all commands (`--verbose`).
    #[must_use]
    pub fn verbose(mut self) -> Self {
        self.global_args.push("--verbose".into());
        self
    }

    /// Enable debug output for all commands (`--debug`).
    #[must_use]
    pub fn debug(mut self) -> Self {
        self.global_args.push("--debug".into());
        self
    }

    /// Set a default retry policy for all commands.
    ///
    /// Individual commands can override this via their own retry settings.
    ///
    /// # Example
    ///
    /// ```no_run
    /// use claude_wrapper::{Claude, RetryPolicy};
    /// use std::time::Duration;
    ///
    /// # fn example() -> claude_wrapper::Result<()> {
    /// let claude = Claude::builder()
    ///     .retry(RetryPolicy::new()
    ///         .max_attempts(3)
    ///         .initial_backoff(Duration::from_secs(2))
    ///         .exponential()
    ///         .retry_on_timeout(true))
    ///     .build()?;
    /// # Ok(())
    /// # }
    /// ```
    #[must_use]
    pub fn retry(mut self, policy: RetryPolicy) -> Self {
        self.retry_policy = Some(policy);
        self
    }

    /// Build the Claude client, resolving the binary path.
    pub fn build(self) -> Result<Claude> {
        let binary = match self.binary {
            Some(path) => path,
            None => which::which("claude").map_err(|_| Error::NotFound)?,
        };

        Ok(Claude {
            binary,
            working_dir: self.working_dir,
            env: self.env,
            global_args: self.global_args,
            timeout: self.timeout,
            retry_policy: self.retry_policy,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_builder_with_binary() {
        let claude = Claude::builder()
            .binary("/usr/local/bin/claude")
            .env("FOO", "bar")
            .timeout_secs(60)
            .build()
            .unwrap();

        assert_eq!(claude.binary, PathBuf::from("/usr/local/bin/claude"));
        assert_eq!(claude.env.get("FOO").unwrap(), "bar");
        assert_eq!(claude.timeout, Some(Duration::from_secs(60)));
    }

    #[test]
    fn test_builder_global_args() {
        let claude = Claude::builder()
            .binary("/usr/local/bin/claude")
            .arg("--verbose")
            .build()
            .unwrap();

        assert_eq!(claude.global_args, vec!["--verbose"]);
    }

    #[test]
    fn test_builder_verbose() {
        let claude = Claude::builder()
            .binary("/usr/local/bin/claude")
            .verbose()
            .build()
            .unwrap();
        assert!(claude.global_args.contains(&"--verbose".to_string()));
    }

    #[test]
    fn test_builder_debug() {
        let claude = Claude::builder()
            .binary("/usr/local/bin/claude")
            .debug()
            .build()
            .unwrap();
        assert!(claude.global_args.contains(&"--debug".to_string()));
    }
}
