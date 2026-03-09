//! A type-safe Claude Code CLI wrapper for Rust.
//!
//! `claude-wrapper` provides a builder-pattern interface for invoking the
//! `claude` CLI programmatically. It follows the same design philosophy as
//! [`docker-wrapper`](https://crates.io/crates/docker-wrapper) and
//! [`terraform-wrapper`](https://crates.io/crates/terraform-wrapper):
//! each CLI subcommand is a builder struct that produces typed output.
//!
//! # Quick Start
//!
//! ```no_run
//! use claude_wrapper::{Claude, ClaudeCommand, QueryCommand, OutputFormat};
//!
//! # async fn example() -> claude_wrapper::Result<()> {
//! let claude = Claude::builder().build()?;
//!
//! // Simple oneshot query
//! let output = QueryCommand::new("explain this error: file not found")
//!     .model("sonnet")
//!     .output_format(OutputFormat::Json)
//!     .execute(&claude)
//!     .await?;
//!
//! println!("{}", output.stdout);
//! # Ok(())
//! # }
//! ```
//!
//! # MCP Config Generation
//!
//! ```no_run
//! use claude_wrapper::McpConfigBuilder;
//!
//! # fn example() -> claude_wrapper::Result<()> {
//! let config = McpConfigBuilder::new()
//!     .http_server("my-hub", "http://127.0.0.1:9090")
//!     .write_to("/tmp/my-project/.mcp.json")?;
//! # Ok(())
//! # }
//! ```

pub mod command;
pub mod error;
pub mod exec;
pub mod mcp_config;
pub mod streaming;
pub mod types;

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

pub use command::ClaudeCommand;
pub use command::agents::AgentsCommand;
pub use command::auth::AuthStatusCommand;
pub use command::doctor::DoctorCommand;
pub use command::marketplace::{
    MarketplaceAddCommand, MarketplaceListCommand, MarketplaceRemoveCommand,
    MarketplaceUpdateCommand,
};
pub use command::mcp::{
    McpAddCommand, McpAddFromDesktopCommand, McpAddJsonCommand, McpGetCommand, McpListCommand,
    McpRemoveCommand, McpResetProjectChoicesCommand,
};
pub use command::plugin::{
    PluginDisableCommand, PluginEnableCommand, PluginInstallCommand, PluginListCommand,
    PluginUninstallCommand, PluginUpdateCommand, PluginValidateCommand,
};
pub use command::query::QueryCommand;
pub use command::raw::RawCommand;
pub use command::version::VersionCommand;
pub use error::{Error, Result};
pub use exec::CommandOutput;
pub use mcp_config::{McpConfigBuilder, McpServerConfig};
pub use types::*;

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
}
