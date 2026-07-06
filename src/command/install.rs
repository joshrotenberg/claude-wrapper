//! `claude install` -- install a Claude Code native build.

#[cfg(feature = "async")]
use crate::Claude;
use crate::command::ClaudeCommand;
#[cfg(feature = "async")]
use crate::error::Result;
#[cfg(feature = "async")]
use crate::exec;
use crate::exec::CommandOutput;

/// Run `claude install [target] [--force]`.
///
/// Installs a Claude Code native build. `target` accepts `"stable"`,
/// `"latest"`, or a specific version; omit it to use the CLI's default.
///
/// # Example
///
/// ```no_run
/// # #[cfg(feature = "async")] {
/// use claude_wrapper::{Claude, ClaudeCommand, InstallCommand};
///
/// # async fn example() -> claude_wrapper::Result<()> {
/// let claude = Claude::builder().build()?;
/// let out = InstallCommand::new()
///     .target("stable")
///     .force()
///     .execute(&claude)
///     .await?;
/// println!("{}", out.stdout);
/// # Ok(()) }
/// # }
/// ```
#[derive(Debug, Clone, Default)]
pub struct InstallCommand {
    target: Option<String>,
    force: bool,
}

impl InstallCommand {
    /// Create a new [`InstallCommand`].
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Target to install: `"stable"`, `"latest"`, or a specific version.
    #[must_use]
    pub fn target(mut self, target: impl Into<String>) -> Self {
        self.target = Some(target.into());
        self
    }

    /// Force installation even if already installed.
    #[must_use]
    pub fn force(mut self) -> Self {
        self.force = true;
        self
    }
}

impl ClaudeCommand for InstallCommand {
    type Output = CommandOutput;

    fn args(&self) -> Vec<String> {
        let mut args = vec!["install".to_string()];
        if self.force {
            args.push("--force".to_string());
        }
        if let Some(ref target) = self.target {
            args.push(target.clone());
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

    #[test]
    fn install_command_defaults() {
        assert_eq!(ClaudeCommand::args(&InstallCommand::new()), vec!["install"]);
    }

    #[test]
    fn install_command_with_target_and_force() {
        let cmd = InstallCommand::new().target("latest").force();
        assert_eq!(
            ClaudeCommand::args(&cmd),
            vec!["install", "--force", "latest"]
        );
    }
}
