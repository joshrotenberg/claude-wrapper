//! Marketplace subcommand builders.
//!
//! Builders for the `claude plugin marketplace` surface: list, add,
//! remove, and update the marketplaces that [`crate::command::plugin`]
//! installs plugins from.

#[cfg(feature = "async")]
use crate::Claude;
use crate::command::ClaudeCommand;
#[cfg(feature = "async")]
use crate::error::Result;
#[cfg(feature = "async")]
use crate::exec;
use crate::exec::CommandOutput;
use crate::types::Scope;

/// List configured plugin marketplaces.
///
/// # Example
///
/// ```no_run
/// use claude_wrapper::{Claude, ClaudeCommand, MarketplaceListCommand};
///
/// # async fn example() -> claude_wrapper::Result<()> {
/// let claude = Claude::builder().build()?;
/// let output = MarketplaceListCommand::new().json().execute(&claude).await?;
/// println!("{}", output.stdout);
/// # Ok(())
/// # }
/// ```
#[derive(Debug, Clone, Default)]
pub struct MarketplaceListCommand {
    json: bool,
}

impl MarketplaceListCommand {
    /// Creates a new marketplace list command.
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
}

impl ClaudeCommand for MarketplaceListCommand {
    type Output = CommandOutput;

    fn args(&self) -> Vec<String> {
        let mut args = vec![
            "plugin".to_string(),
            "marketplace".to_string(),
            "list".to_string(),
        ];
        if self.json {
            args.push("--json".to_string());
        }
        args
    }

    #[cfg(feature = "async")]
    async fn execute(&self, claude: &Claude) -> Result<CommandOutput> {
        exec::run_claude(claude, self.args()).await
    }
}

/// Add a plugin marketplace from a URL, path, or GitHub repo.
///
/// # Example
///
/// ```no_run
/// use claude_wrapper::{Claude, ClaudeCommand, MarketplaceAddCommand, Scope};
///
/// # async fn example() -> claude_wrapper::Result<()> {
/// let claude = Claude::builder().build()?;
/// MarketplaceAddCommand::new("https://github.com/org/marketplace")
///     .scope(Scope::User)
///     .execute(&claude)
///     .await?;
/// # Ok(())
/// # }
/// ```
#[derive(Debug, Clone)]
pub struct MarketplaceAddCommand {
    source: String,
    scope: Option<Scope>,
    sparse: Vec<String>,
}

impl MarketplaceAddCommand {
    /// Creates a command to add a marketplace by URL, path, or GitHub repo.
    #[must_use]
    pub fn new(source: impl Into<String>) -> Self {
        Self {
            source: source.into(),
            scope: None,
            sparse: Vec::new(),
        }
    }

    /// Set the scope.
    #[must_use]
    pub fn scope(mut self, scope: Scope) -> Self {
        self.scope = Some(scope);
        self
    }

    /// Limit checkout to specific directories via git sparse-checkout (for monorepos).
    #[must_use]
    pub fn sparse(mut self, paths: impl IntoIterator<Item = impl Into<String>>) -> Self {
        self.sparse.extend(paths.into_iter().map(Into::into));
        self
    }
}

impl ClaudeCommand for MarketplaceAddCommand {
    type Output = CommandOutput;

    fn args(&self) -> Vec<String> {
        let mut args = vec![
            "plugin".to_string(),
            "marketplace".to_string(),
            "add".to_string(),
        ];
        if let Some(ref scope) = self.scope {
            args.push("--scope".to_string());
            args.push(scope.as_arg().to_string());
        }
        if !self.sparse.is_empty() {
            args.push("--sparse".to_string());
            args.extend(self.sparse.clone());
        }
        args.push(self.source.clone());
        args
    }

    #[cfg(feature = "async")]
    async fn execute(&self, claude: &Claude) -> Result<CommandOutput> {
        exec::run_claude(claude, self.args()).await
    }
}

/// Remove a configured marketplace.
#[derive(Debug, Clone)]
pub struct MarketplaceRemoveCommand {
    name: String,
}

impl MarketplaceRemoveCommand {
    /// Creates a command to remove a marketplace by name.
    #[must_use]
    pub fn new(name: impl Into<String>) -> Self {
        Self { name: name.into() }
    }
}

impl ClaudeCommand for MarketplaceRemoveCommand {
    type Output = CommandOutput;

    fn args(&self) -> Vec<String> {
        vec![
            "plugin".to_string(),
            "marketplace".to_string(),
            "remove".to_string(),
            self.name.clone(),
        ]
    }

    #[cfg(feature = "async")]
    async fn execute(&self, claude: &Claude) -> Result<CommandOutput> {
        exec::run_claude(claude, self.args()).await
    }
}

/// Update marketplace(s) from their source.
#[derive(Debug, Clone, Default)]
pub struct MarketplaceUpdateCommand {
    name: Option<String>,
}

impl MarketplaceUpdateCommand {
    /// Update all marketplaces.
    #[must_use]
    pub fn all() -> Self {
        Self { name: None }
    }

    /// Creates a command to update a specific marketplace catalog.
    #[must_use]
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: Some(name.into()),
        }
    }
}

impl ClaudeCommand for MarketplaceUpdateCommand {
    type Output = CommandOutput;

    fn args(&self) -> Vec<String> {
        let mut args = vec![
            "plugin".to_string(),
            "marketplace".to_string(),
            "update".to_string(),
        ];
        if let Some(ref name) = self.name {
            args.push(name.clone());
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
    fn test_marketplace_list() {
        let cmd = MarketplaceListCommand::new().json();
        assert_eq!(
            ClaudeCommand::args(&cmd),
            vec!["plugin", "marketplace", "list", "--json"]
        );
    }

    #[test]
    fn test_marketplace_add() {
        let cmd = MarketplaceAddCommand::new("https://github.com/org/mp").scope(Scope::User);
        assert_eq!(
            ClaudeCommand::args(&cmd),
            vec![
                "plugin",
                "marketplace",
                "add",
                "--scope",
                "user",
                "https://github.com/org/mp"
            ]
        );
    }

    #[test]
    fn test_marketplace_add_sparse() {
        let cmd = MarketplaceAddCommand::new("https://github.com/org/monorepo")
            .sparse([".claude-plugin", "plugins"]);
        let args = ClaudeCommand::args(&cmd);
        assert!(args.contains(&"--sparse".to_string()));
        assert!(args.contains(&".claude-plugin".to_string()));
        assert!(args.contains(&"plugins".to_string()));
    }

    #[test]
    fn test_marketplace_remove() {
        let cmd = MarketplaceRemoveCommand::new("old-mp");
        assert_eq!(
            ClaudeCommand::args(&cmd),
            vec!["plugin", "marketplace", "remove", "old-mp"]
        );
    }

    #[test]
    fn test_marketplace_update_all() {
        let cmd = MarketplaceUpdateCommand::all();
        assert_eq!(
            ClaudeCommand::args(&cmd),
            vec!["plugin", "marketplace", "update"]
        );
    }

    #[test]
    fn test_marketplace_update_specific() {
        let cmd = MarketplaceUpdateCommand::new("my-mp");
        assert_eq!(
            ClaudeCommand::args(&cmd),
            vec!["plugin", "marketplace", "update", "my-mp"]
        );
    }
}
