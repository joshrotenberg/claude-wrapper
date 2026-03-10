use crate::Claude;
use crate::command::ClaudeCommand;
use crate::error::Result;
use crate::exec::{self, CommandOutput};

/// Run `claude --version` to get the CLI version.
///
/// # Example
///
/// ```no_run
/// use claude_wrapper::{Claude, ClaudeCommand, VersionCommand};
///
/// # async fn example() -> claude_wrapper::Result<()> {
/// let claude = Claude::builder().build()?;
/// let output = VersionCommand::new().execute(&claude).await?;
/// println!("claude version: {}", output.stdout.trim());
/// # Ok(())
/// # }
/// ```
#[derive(Debug, Clone, Default)]
pub struct VersionCommand;

impl VersionCommand {
    #[must_use]
    pub fn new() -> Self {
        Self
    }
}

impl ClaudeCommand for VersionCommand {
    type Output = CommandOutput;

    fn args(&self) -> Vec<String> {
        vec!["--version".to_string()]
    }

    async fn execute(&self, claude: &Claude) -> Result<CommandOutput> {
        exec::run_claude(claude, self.args()).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_version_args() {
        let cmd = VersionCommand::new();
        assert_eq!(cmd.args(), vec!["--version"]);
    }
}
