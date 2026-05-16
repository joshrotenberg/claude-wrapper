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

/// Which billing path the CLI should authenticate against.
/// Maps to `--claudeai` (subscription) or `--console` (Anthropic
/// Console / API usage billing) on `claude auth login`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LoginMode {
    /// Claude subscription account (the CLI's default if neither
    /// flag is passed; passing this is explicit-form). Maps to
    /// `--claudeai`.
    Claudeai,
    /// Anthropic Console account, billed via API usage. Maps to
    /// `--console`. Required for teams on Console billing -- the
    /// default subscription path will sign them into the wrong
    /// account.
    Console,
}

impl LoginMode {
    fn as_arg(self) -> &'static str {
        match self {
            Self::Claudeai => "--claudeai",
            Self::Console => "--console",
        }
    }
}

/// Authenticate with Claude.
///
/// # Billing mode
///
/// As of Claude Code 2.1.x the CLI supports two billing paths:
/// Claude subscription (`--claudeai`, the default) and Anthropic
/// Console / API usage (`--console`). Use [`Self::mode`] to pin
/// the path explicitly -- Console-billed teams need to, or
/// they'll land in the wrong account on first auth.
///
/// # Example
///
/// ```no_run
/// use claude_wrapper::{Claude, ClaudeCommand, AuthLoginCommand};
/// use claude_wrapper::command::auth::LoginMode;
///
/// # async fn example() -> claude_wrapper::Result<()> {
/// let claude = Claude::builder().build()?;
/// AuthLoginCommand::new()
///     .mode(LoginMode::Console)
///     .email("user@example.com")
///     .execute(&claude)
///     .await?;
/// # Ok(())
/// # }
/// ```
#[derive(Debug, Clone, Default)]
pub struct AuthLoginCommand {
    email: Option<String>,
    mode: Option<LoginMode>,
    force_sso: bool,
    #[deprecated(
        since = "0.10.0",
        note = "the `--sso` flag is a boolean since at least Claude Code 2.1.x; \
                the value passed via the deprecated `.sso(provider)` was being \
                emitted as an extra positional and silently doing the wrong thing. \
                Use `force_sso()` to set the boolean flag instead."
    )]
    legacy_sso_value: Option<String>,
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

    /// Pin the billing path (`--claudeai` or `--console`). The CLI
    /// defaults to `Claudeai` when neither flag is passed; setting
    /// this explicitly is the only way Console-billed teams reach
    /// their account.
    #[must_use]
    pub fn mode(mut self, mode: LoginMode) -> Self {
        self.mode = Some(mode);
        self
    }

    /// Force the SSO login flow (`--sso`). Boolean flag with no
    /// value -- replaces the historical [`Self::sso`] which took a
    /// provider name and emitted invalid args (the CLI's `--sso`
    /// has been boolean since at least 2.1.x).
    #[must_use]
    pub fn force_sso(mut self) -> Self {
        self.force_sso = true;
        self
    }

    /// **Deprecated.** Set the SSO provider for authentication.
    ///
    /// The CLI's `--sso` is a boolean flag with no value (since at
    /// least Claude Code 2.1.x). Passing a `provider` string caused
    /// the wrapper to emit `--sso <provider>`, which the CLI parsed
    /// as `--sso` plus an extra positional that was silently
    /// ignored or mishandled. Use [`Self::force_sso`] for the
    /// correct boolean form.
    ///
    /// Kept as a compile-error-and-deprecation-warning bridge so
    /// callers see the change. The value is intentionally ignored
    /// at args() emit time -- only the boolean intent is preserved.
    #[deprecated(
        since = "0.10.0",
        note = "the `--sso` flag is a boolean since at least Claude Code 2.1.x. \
                Use `force_sso()` instead. The value passed here is ignored at \
                emit time; the boolean intent is preserved."
    )]
    #[must_use]
    pub fn sso(mut self, provider: impl Into<String>) -> Self {
        // Honor the boolean intent (caller clearly wanted SSO);
        // record the legacy value purely so the deprecation
        // warning's "this used to break stuff" claim is reproducible
        // by anyone reading the field.
        self.force_sso = true;
        #[allow(deprecated)]
        {
            self.legacy_sso_value = Some(provider.into());
        }
        self
    }
}

impl ClaudeCommand for AuthLoginCommand {
    type Output = CommandOutput;

    fn args(&self) -> Vec<String> {
        let mut args = vec!["auth".to_string(), "login".to_string()];
        if let Some(mode) = self.mode {
            args.push(mode.as_arg().to_string());
        }
        if let Some(ref email) = self.email {
            args.push("--email".to_string());
            args.push(email.clone());
        }
        if self.force_sso {
            args.push("--sso".to_string());
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
    fn test_auth_login_with_force_sso() {
        let cmd = AuthLoginCommand::new().force_sso();
        assert_eq!(cmd.args(), vec!["auth", "login", "--sso"]);
    }

    #[test]
    #[allow(deprecated)]
    fn test_auth_login_deprecated_sso_emits_boolean_only() {
        // Bug fix: the historical `.sso(provider)` would emit
        // `--sso <provider>` but the CLI's `--sso` is boolean. The
        // deprecated method now honors the boolean intent (calls
        // `force_sso` internally) and drops the value at emit time.
        let cmd = AuthLoginCommand::new().sso("okta");
        assert_eq!(cmd.args(), vec!["auth", "login", "--sso"]);
    }

    #[test]
    fn test_auth_login_with_mode_claudeai() {
        let cmd = AuthLoginCommand::new().mode(LoginMode::Claudeai);
        assert_eq!(cmd.args(), vec!["auth", "login", "--claudeai"]);
    }

    #[test]
    fn test_auth_login_with_mode_console() {
        let cmd = AuthLoginCommand::new().mode(LoginMode::Console);
        assert_eq!(cmd.args(), vec!["auth", "login", "--console"]);
    }

    #[test]
    fn test_auth_login_console_with_email() {
        let cmd = AuthLoginCommand::new()
            .mode(LoginMode::Console)
            .email("ops@example.com");
        assert_eq!(
            cmd.args(),
            vec!["auth", "login", "--console", "--email", "ops@example.com"]
        );
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
