//! The raw-subcommand escape hatch.
//!
//! [`RawCommand`] runs an arbitrary `claude` subcommand with
//! caller-supplied args, for surfaces this crate does not yet wrap as a
//! typed builder. Output comes back as [`CommandOutput`]; there is no
//! typed parsing.

#[cfg(feature = "async")]
use crate::Claude;
use crate::command::ClaudeCommand;
#[cfg(feature = "async")]
use crate::error::Result;
#[cfg(feature = "async")]
use crate::exec;
use crate::exec::CommandOutput;

/// Escape hatch for running arbitrary claude subcommands not yet
/// covered by dedicated command builders.
///
/// # Example
///
/// ```no_run
/// use claude_wrapper::{Claude, ClaudeCommand, RawCommand};
///
/// # async fn example() -> claude_wrapper::Result<()> {
/// let claude = Claude::builder().build()?;
///
/// let output = RawCommand::new("agents")
///     .arg("--setting-sources")
///     .arg("user,project")
///     .execute(&claude)
///     .await?;
///
/// println!("{}", output.stdout);
/// # Ok(())
/// # }
/// ```
#[derive(Debug, Clone)]
pub struct RawCommand {
    command_args: Vec<String>,
}

impl RawCommand {
    /// Create a new raw command with the given subcommand.
    #[must_use]
    pub fn new(subcommand: impl Into<String>) -> Self {
        Self {
            command_args: vec![subcommand.into()],
        }
    }

    /// Add a single argument.
    #[must_use]
    pub fn arg(mut self, arg: impl Into<String>) -> Self {
        self.command_args.push(arg.into());
        self
    }

    /// Add multiple arguments.
    #[must_use]
    pub fn args(mut self, args: impl IntoIterator<Item = impl Into<String>>) -> Self {
        self.command_args.extend(args.into_iter().map(Into::into));
        self
    }
}

impl ClaudeCommand for RawCommand {
    type Output = CommandOutput;

    fn args(&self) -> Vec<String> {
        self.command_args.clone()
    }

    #[cfg(feature = "async")]
    async fn execute(&self, claude: &Claude) -> Result<CommandOutput> {
        exec::run_claude(claude, self.args()).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::command::ClaudeCommand;

    #[test]
    fn test_raw_command_args() {
        let cmd = RawCommand::new("agents")
            .arg("--setting-sources")
            .arg("user,project");

        assert_eq!(
            ClaudeCommand::args(&cmd),
            vec!["agents", "--setting-sources", "user,project"]
        );
    }

    #[test]
    fn test_raw_command_with_args() {
        let cmd = RawCommand::new("plugin").args(["list", "--scope", "user"]);

        assert_eq!(
            ClaudeCommand::args(&cmd),
            vec!["plugin", "list", "--scope", "user"]
        );
    }
}
