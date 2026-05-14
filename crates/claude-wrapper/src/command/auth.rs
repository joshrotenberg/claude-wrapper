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
    #[cfg(all(feature = "json", feature = "async"))]
    pub async fn execute_json(&self, claude: &Claude) -> Result<crate::types::AuthStatus> {
        let mut cmd = self.clone();
        cmd.json = true;

        let output = exec::run_claude(claude, cmd.args()).await?;

        serde_json::from_str(&output.stdout).map_err(|e| crate::error::Error::Json {
            message: format!("failed to parse auth status: {e}"),
            source: e,
        })
    }

    /// Blocking mirror of [`AuthStatusCommand::execute_json`].
    #[cfg(all(feature = "sync", feature = "json"))]
    pub fn execute_json_sync(&self, claude: &Claude) -> Result<crate::types::AuthStatus> {
        let mut cmd = self.clone();
        cmd.json = true;

        let output = exec::run_claude_sync(claude, cmd.args())?;

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

    #[cfg(feature = "async")]
    async fn execute(&self, claude: &Claude) -> Result<CommandOutput> {
        exec::run_claude(claude, self.args()).await
    }
}

/// Authenticate with Claude.
///
/// # Example
///
/// ```no_run
/// use claude_wrapper::{Claude, ClaudeCommand, AuthLoginCommand};
///
/// # async fn example() -> claude_wrapper::Result<()> {
/// let claude = Claude::builder().build()?;
/// AuthLoginCommand::new()
///     .email("user@example.com")
///     .execute(&claude)
///     .await?;
/// # Ok(())
/// # }
/// ```
#[derive(Debug, Clone, Default)]
pub struct AuthLoginCommand {
    email: Option<String>,
    sso: Option<String>,
}

impl AuthLoginCommand {
    /// Create a new auth login command.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Set the email address for authentication.
    #[must_use]
    pub fn email(mut self, email: impl Into<String>) -> Self {
        self.email = Some(email.into());
        self
    }

    /// Set the SSO provider for authentication.
    #[must_use]
    pub fn sso(mut self, provider: impl Into<String>) -> Self {
        self.sso = Some(provider.into());
        self
    }
}

impl ClaudeCommand for AuthLoginCommand {
    type Output = CommandOutput;

    fn args(&self) -> Vec<String> {
        let mut args = vec!["auth".to_string(), "login".to_string()];
        if let Some(ref email) = self.email {
            args.push("--email".to_string());
            args.push(email.clone());
        }
        if let Some(ref sso) = self.sso {
            args.push("--sso".to_string());
            args.push(sso.clone());
        }
        args
    }

    #[cfg(feature = "async")]
    async fn execute(&self, claude: &Claude) -> Result<CommandOutput> {
        exec::run_claude(claude, self.args()).await
    }
}

/// Deauthenticate from Claude.
///
/// # Example
///
/// ```no_run
/// use claude_wrapper::{Claude, ClaudeCommand, AuthLogoutCommand};
///
/// # async fn example() -> claude_wrapper::Result<()> {
/// let claude = Claude::builder().build()?;
/// AuthLogoutCommand::new().execute(&claude).await?;
/// # Ok(())
/// # }
/// ```
#[derive(Debug, Clone, Default)]
pub struct AuthLogoutCommand;

impl AuthLogoutCommand {
    /// Create a new auth logout command.
    #[must_use]
    pub fn new() -> Self {
        Self
    }
}

impl ClaudeCommand for AuthLogoutCommand {
    type Output = CommandOutput;

    fn args(&self) -> Vec<String> {
        vec!["auth".to_string(), "logout".to_string()]
    }

    #[cfg(feature = "async")]
    async fn execute(&self, claude: &Claude) -> Result<CommandOutput> {
        exec::run_claude(claude, self.args()).await
    }
}

/// Set up a long-lived authentication token.
///
/// # Example
///
/// ```no_run
/// use claude_wrapper::{Claude, ClaudeCommand, SetupTokenCommand};
///
/// # async fn example() -> claude_wrapper::Result<()> {
/// let claude = Claude::builder().build()?;
/// SetupTokenCommand::new().execute(&claude).await?;
/// # Ok(())
/// # }
/// ```
#[derive(Debug, Clone, Default)]
pub struct SetupTokenCommand;

impl SetupTokenCommand {
    /// Create a new setup-token command.
    #[must_use]
    pub fn new() -> Self {
        Self
    }
}

impl ClaudeCommand for SetupTokenCommand {
    type Output = CommandOutput;

    fn args(&self) -> Vec<String> {
        vec!["setup-token".to_string()]
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
    fn test_auth_status_args() {
        let cmd = AuthStatusCommand::new();
        assert_eq!(cmd.args(), vec!["auth", "status", "--json"]);
    }

    #[test]
    fn test_auth_status_text() {
        let cmd = AuthStatusCommand::new().text();
        assert_eq!(cmd.args(), vec!["auth", "status", "--text"]);
    }

    #[test]
    fn test_auth_login_default() {
        let cmd = AuthLoginCommand::new();
        assert_eq!(cmd.args(), vec!["auth", "login"]);
    }

    #[test]
    fn test_auth_login_with_email() {
        let cmd = AuthLoginCommand::new().email("user@example.com");
        assert_eq!(
            cmd.args(),
            vec!["auth", "login", "--email", "user@example.com"]
        );
    }

    #[test]
    fn test_auth_login_with_sso() {
        let cmd = AuthLoginCommand::new().sso("okta");
        assert_eq!(cmd.args(), vec!["auth", "login", "--sso", "okta"]);
    }

    #[test]
    fn test_auth_logout() {
        let cmd = AuthLogoutCommand::new();
        assert_eq!(cmd.args(), vec!["auth", "logout"]);
    }

    #[test]
    fn test_setup_token() {
        let cmd = SetupTokenCommand::new();
        assert_eq!(cmd.args(), vec!["setup-token"]);
    }
}
