#[cfg(feature = "async")]
use crate::Claude;
use crate::command::ClaudeCommand;
#[cfg(feature = "async")]
use crate::error::Result;
#[cfg(feature = "async")]
use crate::exec;
use crate::exec::CommandOutput;

/// Run `claude doctor` to check CLI health.
///
/// # Example
///
/// ```no_run
/// use claude_wrapper::{Claude, ClaudeCommand, DoctorCommand};
///
/// # async fn example() -> claude_wrapper::Result<()> {
/// let claude = Claude::builder().build()?;
/// let output = DoctorCommand::new().execute(&claude).await?;
/// println!("{}", output.stdout);
/// # Ok(())
/// # }
/// ```
#[derive(Debug, Clone, Default)]
pub struct DoctorCommand {}

impl DoctorCommand {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }
}

impl ClaudeCommand for DoctorCommand {
    type Output = CommandOutput;

    fn args(&self) -> Vec<String> {
        vec!["doctor".to_string()]
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
    fn test_doctor_args() {
        let cmd = DoctorCommand::new();
        assert_eq!(cmd.args(), vec!["doctor"]);
    }
}
