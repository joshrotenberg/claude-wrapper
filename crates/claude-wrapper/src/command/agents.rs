#[cfg(feature = "async")]
use crate::Claude;
use crate::command::ClaudeCommand;
#[cfg(feature = "async")]
use crate::error::Result;
#[cfg(feature = "async")]
use crate::exec;
use crate::exec::CommandOutput;

/// List configured agents.
///
/// # Example
///
/// ```no_run
/// use claude_wrapper::{Claude, ClaudeCommand, AgentsCommand};
///
/// # async fn example() -> claude_wrapper::Result<()> {
/// let claude = Claude::builder().build()?;
/// let output = AgentsCommand::new()
///     .setting_sources("user,project")
///     .execute(&claude)
///     .await?;
/// println!("{}", output.stdout);
/// # Ok(())
/// # }
/// ```
#[derive(Debug, Clone, Default)]
pub struct AgentsCommand {
    setting_sources: Option<String>,
}

impl AgentsCommand {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Set which setting sources to load (comma-separated: user, project, local).
    #[must_use]
    pub fn setting_sources(mut self, sources: impl Into<String>) -> Self {
        self.setting_sources = Some(sources.into());
        self
    }
}

impl ClaudeCommand for AgentsCommand {
    type Output = CommandOutput;

    fn args(&self) -> Vec<String> {
        let mut args = vec!["agents".to_string()];
        if let Some(ref sources) = self.setting_sources {
            args.push("--setting-sources".to_string());
            args.push(sources.clone());
        }
        args
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
    fn test_agents_default() {
        let cmd = AgentsCommand::new();
        assert_eq!(ClaudeCommand::args(&cmd), vec!["agents"]);
    }

    #[test]
    fn test_agents_with_sources() {
        let cmd = AgentsCommand::new().setting_sources("user,project");
        assert_eq!(
            ClaudeCommand::args(&cmd),
            vec!["agents", "--setting-sources", "user,project"]
        );
    }
}
