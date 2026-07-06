//! The deprecated `agents` subcommand builder.
//!
//! [`AgentsCommand`] wraps `claude agents`. It is retained for
//! back-compat; prefer the read-side [`crate::artifacts`] introspection
//! for agent definitions and `--agent` / `--agents` on
//! [`QueryCommand`](crate::QueryCommand) for selecting one.

#[cfg(feature = "async")]
use crate::Claude;
use crate::command::ClaudeCommand;
#[cfg(feature = "async")]
use crate::error::Result;
#[cfg(feature = "async")]
use crate::exec;
use crate::exec::CommandOutput;

/// **Deprecated.** Wraps `claude agents`, which as of Claude Code
/// 2.1.143 is an interactive TUI for managing background agent
/// sessions -- not a way to list user-defined subagent definitions.
/// Calling `.execute()` non-interactively just emits
/// `'claude agents' is not available in this environment.` to
/// stderr and returns empty stdout.
///
/// Use [`crate::artifacts::AgentsRoot`] to enumerate / read /
/// write user-level subagent definitions in `~/.claude/agents/`.
/// There is no current non-interactive CLI surface for the
/// background-session manager.
///
/// Kept for back-compat; will be removed in a future major release.
#[deprecated(
    since = "0.10.0",
    note = "`claude agents` is now an interactive TUI in Claude Code 2.1.143+ (background-session manager). \
            For listing/reading/writing user subagents in `~/.claude/agents/`, use \
            `claude_wrapper::artifacts::AgentsRoot`."
)]
#[derive(Debug, Clone, Default)]
pub struct AgentsCommand {
    setting_sources: Option<String>,
}

#[allow(deprecated)]
impl AgentsCommand {
    /// Create a new [`AgentsCommand`].
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

#[allow(deprecated)]
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
#[allow(deprecated)]
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
