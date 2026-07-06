//! `claude update` -- check for CLI updates and install if available.

#[cfg(feature = "async")]
use crate::Claude;
use crate::command::ClaudeCommand;
#[cfg(feature = "async")]
use crate::error::Result;
#[cfg(feature = "async")]
use crate::exec;
use crate::exec::CommandOutput;

/// Run `claude update` (alias `upgrade`).
///
/// Checks for updates and installs if available. Takes no options.
///
/// # Example
///
/// ```no_run
/// # #[cfg(feature = "async")] {
/// use claude_wrapper::{Claude, ClaudeCommand, UpdateCommand};
///
/// # async fn example() -> claude_wrapper::Result<()> {
/// let claude = Claude::builder().build()?;
/// let out = UpdateCommand::new().execute(&claude).await?;
/// println!("{}", out.stdout);
/// # Ok(()) }
/// # }
/// ```
#[derive(Debug, Clone, Default)]
pub struct UpdateCommand;

impl UpdateCommand {
    /// Create a new [`UpdateCommand`].
    #[must_use]
    pub fn new() -> Self {
        Self
    }
}

impl ClaudeCommand for UpdateCommand {
    type Output = CommandOutput;

    fn args(&self) -> Vec<String> {
        vec!["update".to_string()]
    }

    #[cfg(feature = "async")]
    async fn execute(&self, claude: &Claude) -> Result<CommandOutput> {
        exec::run_claude(claude, self.args()).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn update_command_args() {
        assert_eq!(ClaudeCommand::args(&UpdateCommand::new()), vec!["update"]);
    }
}
