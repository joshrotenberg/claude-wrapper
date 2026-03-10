use crate::Claude;
use crate::command::ClaudeCommand;
use crate::error::Result;
use crate::exec::{self, CommandOutput};

/// Check authentication status.
///
/// # Example
///
/// ```no_run
/// use claude_wrapper::{Claude, ClaudeCommand, AuthStatusCommand};
///
/// # async fn example() -> claude_wrapper::Result<()> {
/// let claude = Claude::builder().build()?;
/// let status = AuthStatusCommand::new().execute_json(&claude).await?;
/// println!("logged in: {}", status.logged_in);
/// # Ok(())
/// # }
/// ```
#[derive(Debug, Clone, Default)]
pub struct AuthStatusCommand {
    json: bool,
}

impl AuthStatusCommand {
    /// Create a new auth status command.
    #[must_use]
    pub fn new() -> Self {
        Self { json: true }
    }

    /// Request text output instead of JSON.
    #[must_use]
    pub fn text(mut self) -> Self {
        self.json = false;
        self
    }

    /// Execute and parse the JSON result into an [`AuthStatus`](crate::types::AuthStatus).
    #[cfg(feature = "json")]
    pub async fn execute_json(&self, claude: &Claude) -> Result<crate::types::AuthStatus> {
        let mut cmd = self.clone();
        cmd.json = true;

        let output = exec::run_claude(claude, cmd.args()).await?;

        serde_json::from_str(&output.stdout).map_err(|e| crate::error::Error::Json {
            message: format!("failed to parse auth status: {e}"),
            source: e,
        })
    }
}

impl ClaudeCommand for AuthStatusCommand {
    type Output = CommandOutput;

    fn args(&self) -> Vec<String> {
        let mut args = vec!["auth".to_string(), "status".to_string()];
        if self.json {
            args.push("--json".to_string());
        } else {
            args.push("--text".to_string());
        }
        args
    }

    async fn execute(&self, claude: &Claude) -> Result<CommandOutput> {
        exec::run_claude(claude, self.args()).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_auth_status_args() {
        let cmd = AuthStatusCommand::new();
        assert_eq!(cmd.args(), vec!["auth", "status", "--json"]);
    }

    #[test]
    fn test_auth_status_text() {
        let cmd = AuthStatusCommand::new().text();
        assert_eq!(cmd.args(), vec!["auth", "status", "--text"]);
    }
}
