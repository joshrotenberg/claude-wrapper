use crate::Claude;
use crate::command::ClaudeCommand;
use crate::error::Result;
use crate::exec::{self, CommandOutput};
use crate::types::Scope;

/// List installed plugins.
///
/// # Example
///
/// ```no_run
/// use claude_wrapper::{Claude, ClaudeCommand, PluginListCommand};
///
/// # async fn example() -> claude_wrapper::Result<()> {
/// let claude = Claude::builder().build()?;
/// let output = PluginListCommand::new().json().execute(&claude).await?;
/// println!("{}", output.stdout);
/// # Ok(())
/// # }
/// ```
#[derive(Debug, Clone, Default)]
pub struct PluginListCommand {
    json: bool,
    available: bool,
}

impl PluginListCommand {
    /// Creates a new plugin list command.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Output as JSON.
    #[must_use]
    pub fn json(mut self) -> Self {
        self.json = true;
        self
    }

    /// Include available plugins from marketplaces (requires `json()`).
    #[must_use]
    pub fn available(mut self) -> Self {
        self.available = true;
        self
    }
}

impl ClaudeCommand for PluginListCommand {
    type Output = CommandOutput;

    fn args(&self) -> Vec<String> {
        let mut args = vec!["plugin".to_string(), "list".to_string()];
        if self.json {
            args.push("--json".to_string());
        }
        if self.available {
            args.push("--available".to_string());
        }
        args
    }

    async fn execute(&self, claude: &Claude) -> Result<CommandOutput> {
        exec::run_claude(claude, self.args()).await
    }
}

/// Install a plugin.
///
/// # Example
///
/// ```no_run
/// use claude_wrapper::{Claude, ClaudeCommand, PluginInstallCommand, Scope};
///
/// # async fn example() -> claude_wrapper::Result<()> {
/// let claude = Claude::builder().build()?;
/// PluginInstallCommand::new("my-plugin")
///     .scope(Scope::User)
///     .execute(&claude)
///     .await?;
/// # Ok(())
/// # }
/// ```
#[derive(Debug, Clone)]
pub struct PluginInstallCommand {
    plugin: String,
    scope: Option<Scope>,
}

impl PluginInstallCommand {
    /// Creates a command to install a plugin by name.
    #[must_use]
    pub fn new(plugin: impl Into<String>) -> Self {
        Self {
            plugin: plugin.into(),
            scope: None,
        }
    }

    /// Set the installation scope.
    #[must_use]
    pub fn scope(mut self, scope: Scope) -> Self {
        self.scope = Some(scope);
        self
    }
}

impl ClaudeCommand for PluginInstallCommand {
    type Output = CommandOutput;

    fn args(&self) -> Vec<String> {
        let mut args = vec!["plugin".to_string(), "install".to_string()];
        if let Some(ref scope) = self.scope {
            args.push("--scope".to_string());
            args.push(scope.as_arg().to_string());
        }
        args.push(self.plugin.clone());
        args
    }

    async fn execute(&self, claude: &Claude) -> Result<CommandOutput> {
        exec::run_claude(claude, self.args()).await
    }
}

/// Uninstall a plugin.
#[derive(Debug, Clone)]
pub struct PluginUninstallCommand {
    plugin: String,
    scope: Option<Scope>,
}

impl PluginUninstallCommand {
    /// Creates a command to uninstall a plugin by name.
    #[must_use]
    pub fn new(plugin: impl Into<String>) -> Self {
        Self {
            plugin: plugin.into(),
            scope: None,
        }
    }

    /// Set the scope.
    #[must_use]
    pub fn scope(mut self, scope: Scope) -> Self {
        self.scope = Some(scope);
        self
    }
}

impl ClaudeCommand for PluginUninstallCommand {
    type Output = CommandOutput;

    fn args(&self) -> Vec<String> {
        let mut args = vec!["plugin".to_string(), "uninstall".to_string()];
        if let Some(ref scope) = self.scope {
            args.push("--scope".to_string());
            args.push(scope.as_arg().to_string());
        }
        args.push(self.plugin.clone());
        args
    }

    async fn execute(&self, claude: &Claude) -> Result<CommandOutput> {
        exec::run_claude(claude, self.args()).await
    }
}

/// Enable a disabled plugin.
#[derive(Debug, Clone)]
pub struct PluginEnableCommand {
    plugin: String,
    scope: Option<Scope>,
}

impl PluginEnableCommand {
    /// Creates a command to enable a plugin by name.
    #[must_use]
    pub fn new(plugin: impl Into<String>) -> Self {
        Self {
            plugin: plugin.into(),
            scope: None,
        }
    }

    /// Set the scope.
    #[must_use]
    pub fn scope(mut self, scope: Scope) -> Self {
        self.scope = Some(scope);
        self
    }
}

impl ClaudeCommand for PluginEnableCommand {
    type Output = CommandOutput;

    fn args(&self) -> Vec<String> {
        let mut args = vec!["plugin".to_string(), "enable".to_string()];
        if let Some(ref scope) = self.scope {
            args.push("--scope".to_string());
            args.push(scope.as_arg().to_string());
        }
        args.push(self.plugin.clone());
        args
    }

    async fn execute(&self, claude: &Claude) -> Result<CommandOutput> {
        exec::run_claude(claude, self.args()).await
    }
}

/// Disable an enabled plugin.
#[derive(Debug, Clone)]
pub struct PluginDisableCommand {
    plugin: Option<String>,
    scope: Option<Scope>,
    all: bool,
}

impl PluginDisableCommand {
    /// Creates a command to disable a plugin by name. To disable all plugins, use [`PluginDisableCommand::all`].
    #[must_use]
    pub fn new(plugin: impl Into<String>) -> Self {
        Self {
            plugin: Some(plugin.into()),
            scope: None,
            all: false,
        }
    }

    /// Disable all enabled plugins.
    #[must_use]
    pub fn all() -> Self {
        Self {
            plugin: None,
            scope: None,
            all: true,
        }
    }

    /// Set the scope.
    #[must_use]
    pub fn scope(mut self, scope: Scope) -> Self {
        self.scope = Some(scope);
        self
    }
}

impl ClaudeCommand for PluginDisableCommand {
    type Output = CommandOutput;

    fn args(&self) -> Vec<String> {
        let mut args = vec!["plugin".to_string(), "disable".to_string()];
        if self.all {
            args.push("--all".to_string());
        }
        if let Some(ref scope) = self.scope {
            args.push("--scope".to_string());
            args.push(scope.as_arg().to_string());
        }
        if let Some(ref plugin) = self.plugin {
            args.push(plugin.clone());
        }
        args
    }

    async fn execute(&self, claude: &Claude) -> Result<CommandOutput> {
        exec::run_claude(claude, self.args()).await
    }
}

/// Update a plugin to the latest version.
#[derive(Debug, Clone)]
pub struct PluginUpdateCommand {
    plugin: String,
    scope: Option<Scope>,
}

impl PluginUpdateCommand {
    /// Creates a command to update a plugin to the latest version.
    #[must_use]
    pub fn new(plugin: impl Into<String>) -> Self {
        Self {
            plugin: plugin.into(),
            scope: None,
        }
    }

    /// Set the scope.
    #[must_use]
    pub fn scope(mut self, scope: Scope) -> Self {
        self.scope = Some(scope);
        self
    }
}

impl ClaudeCommand for PluginUpdateCommand {
    type Output = CommandOutput;

    fn args(&self) -> Vec<String> {
        let mut args = vec!["plugin".to_string(), "update".to_string()];
        if let Some(ref scope) = self.scope {
            args.push("--scope".to_string());
            args.push(scope.as_arg().to_string());
        }
        args.push(self.plugin.clone());
        args
    }

    async fn execute(&self, claude: &Claude) -> Result<CommandOutput> {
        exec::run_claude(claude, self.args()).await
    }
}

/// Validate a plugin or marketplace manifest.
#[derive(Debug, Clone)]
pub struct PluginValidateCommand {
    path: String,
}

impl PluginValidateCommand {
    /// Creates a command to validate a plugin manifest at the given path.
    #[must_use]
    pub fn new(path: impl Into<String>) -> Self {
        Self { path: path.into() }
    }
}

impl ClaudeCommand for PluginValidateCommand {
    type Output = CommandOutput;

    fn args(&self) -> Vec<String> {
        vec![
            "plugin".to_string(),
            "validate".to_string(),
            self.path.clone(),
        ]
    }

    async fn execute(&self, claude: &Claude) -> Result<CommandOutput> {
        exec::run_claude(claude, self.args()).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::command::ClaudeCommand;

    #[test]
    fn test_plugin_list() {
        let cmd = PluginListCommand::new().json().available();
        assert_eq!(
            ClaudeCommand::args(&cmd),
            vec!["plugin", "list", "--json", "--available"]
        );
    }

    #[test]
    fn test_plugin_install() {
        let cmd = PluginInstallCommand::new("my-plugin").scope(Scope::User);
        assert_eq!(
            ClaudeCommand::args(&cmd),
            vec!["plugin", "install", "--scope", "user", "my-plugin"]
        );
    }

    #[test]
    fn test_plugin_uninstall() {
        let cmd = PluginUninstallCommand::new("old-plugin");
        assert_eq!(
            ClaudeCommand::args(&cmd),
            vec!["plugin", "uninstall", "old-plugin"]
        );
    }

    #[test]
    fn test_plugin_enable() {
        let cmd = PluginEnableCommand::new("my-plugin").scope(Scope::Project);
        assert_eq!(
            ClaudeCommand::args(&cmd),
            vec!["plugin", "enable", "--scope", "project", "my-plugin"]
        );
    }

    #[test]
    fn test_plugin_disable_specific() {
        let cmd = PluginDisableCommand::new("my-plugin");
        assert_eq!(
            ClaudeCommand::args(&cmd),
            vec!["plugin", "disable", "my-plugin"]
        );
    }

    #[test]
    fn test_plugin_disable_all() {
        let cmd = PluginDisableCommand::all();
        assert_eq!(
            ClaudeCommand::args(&cmd),
            vec!["plugin", "disable", "--all"]
        );
    }

    #[test]
    fn test_plugin_update() {
        let cmd = PluginUpdateCommand::new("my-plugin").scope(Scope::Local);
        assert_eq!(
            ClaudeCommand::args(&cmd),
            vec!["plugin", "update", "--scope", "local", "my-plugin"]
        );
    }

    #[test]
    fn test_plugin_validate() {
        let cmd = PluginValidateCommand::new("/path/to/manifest");
        assert_eq!(
            ClaudeCommand::args(&cmd),
            vec!["plugin", "validate", "/path/to/manifest"]
        );
    }
}
