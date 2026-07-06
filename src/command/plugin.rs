//! Plugin subcommand builders.
//!
//! Builders for the `claude plugin` surface: list, install, uninstall,
//! enable, disable, update, validate, details, prune, and tag. See
//! [`crate::command::marketplace`] for managing the marketplaces
//! plugins are installed from.

#[cfg(feature = "async")]
use crate::Claude;
use crate::command::ClaudeCommand;
#[cfg(feature = "async")]
use crate::error::Result;
#[cfg(feature = "async")]
use crate::exec;
use crate::exec::CommandOutput;
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

    #[cfg(feature = "async")]
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

    #[cfg(feature = "async")]
    async fn execute(&self, claude: &Claude) -> Result<CommandOutput> {
        exec::run_claude(claude, self.args()).await
    }
}

/// Uninstall a plugin.
///
/// **Headless callers should pass [`Self::yes`]** -- the underlying
/// CLI requires `-y` whenever stdin/stdout isn't a TTY and will
/// otherwise wait on a prompt that no one is around to answer.
#[derive(Debug, Clone)]
pub struct PluginUninstallCommand {
    plugin: String,
    scope: Option<Scope>,
    keep_data: bool,
    prune: bool,
    yes: bool,
}

impl PluginUninstallCommand {
    /// Creates a command to uninstall a plugin by name.
    #[must_use]
    pub fn new(plugin: impl Into<String>) -> Self {
        Self {
            plugin: plugin.into(),
            scope: None,
            keep_data: false,
            prune: false,
            yes: false,
        }
    }

    /// Set the scope.
    #[must_use]
    pub fn scope(mut self, scope: Scope) -> Self {
        self.scope = Some(scope);
        self
    }

    /// Preserve the plugin's persistent data directory
    /// (`~/.claude/plugins/data/{id}/`) on uninstall (`--keep-data`).
    /// Default: data is removed alongside the plugin.
    #[must_use]
    pub fn keep_data(mut self) -> Self {
        self.keep_data = true;
        self
    }

    /// Also remove auto-installed dependencies that are no longer
    /// needed (`--prune`). Requires [`Self::yes`] in non-interactive
    /// contexts (which the wrapper always is).
    #[must_use]
    pub fn prune(mut self) -> Self {
        self.prune = true;
        self
    }

    /// Skip the `--prune` confirmation prompt (`-y`). **Required for
    /// non-TTY callers** -- without it, the CLI will hang waiting on
    /// stdin. Every wrapper consumer running under `execute()` is
    /// non-TTY by definition, so you almost always want this on.
    #[must_use]
    pub fn yes(mut self) -> Self {
        self.yes = true;
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
        if self.keep_data {
            args.push("--keep-data".to_string());
        }
        if self.prune {
            args.push("--prune".to_string());
        }
        if self.yes {
            args.push("--yes".to_string());
        }
        args.push(self.plugin.clone());
        args
    }

    #[cfg(feature = "async")]
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

    #[cfg(feature = "async")]
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

    #[cfg(feature = "async")]
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

    #[cfg(feature = "async")]
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

    #[cfg(feature = "async")]
    async fn execute(&self, claude: &Claude) -> Result<CommandOutput> {
        exec::run_claude(claude, self.args()).await
    }
}

/// Create a `{name}--v{version}` git tag for a plugin release.
///
/// Runs `claude plugin tag [path]`, validating that the plugin's
/// `plugin.json` and any enclosing marketplace entry agree on the
/// version before tagging.
///
/// # Example
///
/// ```no_run
/// # #[cfg(feature = "async")] {
/// use claude_wrapper::{Claude, ClaudeCommand, PluginTagCommand};
///
/// # async fn example() -> claude_wrapper::Result<()> {
/// let claude = Claude::builder().build()?;
/// let out = PluginTagCommand::new()
///     .path("./my-plugin")
///     .message("release %s")
///     .push()
///     .execute(&claude)
///     .await?;
/// println!("{}", out.stdout);
/// # Ok(()) }
/// # }
/// ```
#[derive(Debug, Clone, Default)]
pub struct PluginTagCommand {
    path: Option<String>,
    dry_run: bool,
    force: bool,
    message: Option<String>,
    push: bool,
    remote: Option<String>,
}

impl PluginTagCommand {
    /// Create a new tag command. Without [`path`](Self::path), the CLI
    /// uses the current directory.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Path to the plugin directory.
    #[must_use]
    pub fn path(mut self, path: impl Into<String>) -> Self {
        self.path = Some(path.into());
        self
    }

    /// Print what would be tagged without creating anything.
    #[must_use]
    pub fn dry_run(mut self) -> Self {
        self.dry_run = true;
        self
    }

    /// Skip dirty-working-tree and tag-already-exists checks.
    #[must_use]
    pub fn force(mut self) -> Self {
        self.force = true;
        self
    }

    /// Tag annotation message; `%s` is substituted with the version.
    #[must_use]
    pub fn message(mut self, msg: impl Into<String>) -> Self {
        self.message = Some(msg.into());
        self
    }

    /// Push the tag after creating it.
    #[must_use]
    pub fn push(mut self) -> Self {
        self.push = true;
        self
    }

    /// Override the remote pushed to with [`push`](Self::push) (default `origin`).
    #[must_use]
    pub fn remote(mut self, remote: impl Into<String>) -> Self {
        self.remote = Some(remote.into());
        self
    }
}

impl ClaudeCommand for PluginTagCommand {
    type Output = CommandOutput;

    fn args(&self) -> Vec<String> {
        let mut args = vec!["plugin".to_string(), "tag".to_string()];
        if self.dry_run {
            args.push("--dry-run".to_string());
        }
        if self.force {
            args.push("--force".to_string());
        }
        if let Some(ref msg) = self.message {
            args.push("--message".to_string());
            args.push(msg.clone());
        }
        if self.push {
            args.push("--push".to_string());
        }
        if let Some(ref remote) = self.remote {
            args.push("--remote".to_string());
            args.push(remote.clone());
        }
        if let Some(ref path) = self.path {
            args.push(path.clone());
        }
        args
    }

    #[cfg(feature = "async")]
    async fn execute(&self, claude: &Claude) -> Result<CommandOutput> {
        exec::run_claude(claude, self.args()).await
    }
}

/// Show a plugin's component inventory and projected token cost
/// (`claude plugin details <name>`).
#[derive(Debug, Clone)]
pub struct PluginDetailsCommand {
    plugin: String,
}

impl PluginDetailsCommand {
    /// Create a details command for the given plugin name.
    #[must_use]
    pub fn new(plugin: impl Into<String>) -> Self {
        Self {
            plugin: plugin.into(),
        }
    }
}

impl ClaudeCommand for PluginDetailsCommand {
    type Output = CommandOutput;

    fn args(&self) -> Vec<String> {
        vec![
            "plugin".to_string(),
            "details".to_string(),
            self.plugin.clone(),
        ]
    }

    #[cfg(feature = "async")]
    async fn execute(&self, claude: &Claude) -> Result<CommandOutput> {
        exec::run_claude(claude, self.args()).await
    }
}

/// Remove auto-installed dependencies that are no longer needed
/// (`claude plugin prune` -- alias `autoremove`).
///
/// Non-TTY callers should pass [`Self::yes`] -- the underlying CLI
/// requires `-y` whenever stdin/stdout isn't a TTY and will
/// otherwise wait on a confirmation prompt.
#[derive(Debug, Clone, Default)]
pub struct PluginPruneCommand {
    dry_run: bool,
    scope: Option<Scope>,
    yes: bool,
}

impl PluginPruneCommand {
    /// Create a new prune command.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Print what would be removed without removing anything
    /// (`--dry-run`).
    #[must_use]
    pub fn dry_run(mut self) -> Self {
        self.dry_run = true;
        self
    }

    /// Set the scope (`-s/--scope`). Default: `user`.
    #[must_use]
    pub fn scope(mut self, scope: Scope) -> Self {
        self.scope = Some(scope);
        self
    }

    /// Skip the confirmation prompt (`-y`). **Required for non-TTY
    /// callers** -- without it the CLI will hang waiting on stdin.
    #[must_use]
    pub fn yes(mut self) -> Self {
        self.yes = true;
        self
    }
}

impl ClaudeCommand for PluginPruneCommand {
    type Output = CommandOutput;

    fn args(&self) -> Vec<String> {
        let mut args = vec!["plugin".to_string(), "prune".to_string()];
        if self.dry_run {
            args.push("--dry-run".to_string());
        }
        if let Some(ref scope) = self.scope {
            args.push("--scope".to_string());
            args.push(scope.as_arg().to_string());
        }
        if self.yes {
            args.push("--yes".to_string());
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
    fn test_plugin_uninstall_with_all_flags() {
        let cmd = PluginUninstallCommand::new("old-plugin")
            .scope(Scope::User)
            .keep_data()
            .prune()
            .yes();
        assert_eq!(
            ClaudeCommand::args(&cmd),
            vec![
                "plugin",
                "uninstall",
                "--scope",
                "user",
                "--keep-data",
                "--prune",
                "--yes",
                "old-plugin"
            ]
        );
    }

    #[test]
    fn test_plugin_uninstall_yes_alone() {
        // Most common headless case: just need to skip the prompt.
        let cmd = PluginUninstallCommand::new("p").yes();
        assert_eq!(
            ClaudeCommand::args(&cmd),
            vec!["plugin", "uninstall", "--yes", "p"]
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

    #[test]
    fn plugin_tag_defaults_to_just_subcommand() {
        let cmd = PluginTagCommand::new();
        assert_eq!(ClaudeCommand::args(&cmd), vec!["plugin", "tag"]);
    }

    #[test]
    fn plugin_tag_with_all_options() {
        let cmd = PluginTagCommand::new()
            .path("./plugin")
            .dry_run()
            .force()
            .message("release %s")
            .push()
            .remote("upstream");
        assert_eq!(
            ClaudeCommand::args(&cmd),
            vec![
                "plugin",
                "tag",
                "--dry-run",
                "--force",
                "--message",
                "release %s",
                "--push",
                "--remote",
                "upstream",
                "./plugin",
            ]
        );
    }

    #[test]
    fn test_plugin_details() {
        let cmd = PluginDetailsCommand::new("some-plugin");
        assert_eq!(
            ClaudeCommand::args(&cmd),
            vec!["plugin", "details", "some-plugin"]
        );
    }

    #[test]
    fn test_plugin_prune_default() {
        let cmd = PluginPruneCommand::new();
        assert_eq!(ClaudeCommand::args(&cmd), vec!["plugin", "prune"]);
    }

    #[test]
    fn test_plugin_prune_all_flags() {
        let cmd = PluginPruneCommand::new().dry_run().scope(Scope::User).yes();
        assert_eq!(
            ClaudeCommand::args(&cmd),
            vec!["plugin", "prune", "--dry-run", "--scope", "user", "--yes"]
        );
    }

    #[test]
    fn test_scope_managed_renders_as_arg() {
        // `claude plugin update --scope managed` added in 2.1.143.
        let cmd = PluginUpdateCommand::new("p").scope(Scope::Managed);
        assert_eq!(
            ClaudeCommand::args(&cmd),
            vec!["plugin", "update", "--scope", "managed", "p"]
        );
    }
}
