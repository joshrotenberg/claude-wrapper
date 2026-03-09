use crate::Claude;
use crate::command::ClaudeCommand;
use crate::error::Result;
use crate::exec::{self, CommandOutput};
use crate::types::Scope;

/// List configured MCP servers.
///
/// # Example
///
/// ```no_run
/// use claude_wrapper::{Claude, ClaudeCommand, McpListCommand};
///
/// # async fn example() -> claude_wrapper::Result<()> {
/// let claude = Claude::builder().build()?;
/// let output = McpListCommand::new().execute(&claude).await?;
/// println!("{}", output.stdout);
/// # Ok(())
/// # }
/// ```
#[derive(Debug, Clone, Default)]
pub struct McpListCommand;

impl McpListCommand {
    #[must_use]
    pub fn new() -> Self {
        Self
    }
}

impl ClaudeCommand for McpListCommand {
    type Output = CommandOutput;

    fn args(&self) -> Vec<String> {
        vec!["mcp".to_string(), "list".to_string()]
    }

    async fn execute(&self, claude: &Claude) -> Result<CommandOutput> {
        exec::run_claude(claude, self.args()).await
    }
}

/// Get details about a specific MCP server.
#[derive(Debug, Clone)]
pub struct McpGetCommand {
    name: String,
}

impl McpGetCommand {
    #[must_use]
    pub fn new(name: impl Into<String>) -> Self {
        Self { name: name.into() }
    }
}

impl ClaudeCommand for McpGetCommand {
    type Output = CommandOutput;

    fn args(&self) -> Vec<String> {
        vec!["mcp".to_string(), "get".to_string(), self.name.clone()]
    }

    async fn execute(&self, claude: &Claude) -> Result<CommandOutput> {
        exec::run_claude(claude, self.args()).await
    }
}

/// Add an MCP server.
///
/// # Example
///
/// ```no_run
/// use claude_wrapper::{Claude, ClaudeCommand, McpAddCommand, Scope};
///
/// # async fn example() -> claude_wrapper::Result<()> {
/// let claude = Claude::builder().build()?;
///
/// // Add an HTTP MCP server
/// McpAddCommand::new("sentry", "https://mcp.sentry.dev/mcp")
///     .transport("http")
///     .scope(Scope::User)
///     .execute(&claude)
///     .await?;
///
/// // Add a stdio MCP server with env vars
/// McpAddCommand::new("my-server", "npx")
///     .server_args(["my-mcp-server"])
///     .env("API_KEY", "xxx")
///     .scope(Scope::Local)
///     .execute(&claude)
///     .await?;
/// # Ok(())
/// # }
/// ```
#[derive(Debug, Clone)]
pub struct McpAddCommand {
    name: String,
    command_or_url: String,
    server_args: Vec<String>,
    transport: Option<String>,
    scope: Option<Scope>,
    env: Vec<(String, String)>,
    headers: Vec<String>,
}

impl McpAddCommand {
    /// Create a new MCP add command.
    #[must_use]
    pub fn new(name: impl Into<String>, command_or_url: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            command_or_url: command_or_url.into(),
            server_args: Vec::new(),
            transport: None,
            scope: None,
            env: Vec::new(),
            headers: Vec::new(),
        }
    }

    /// Set the transport type (stdio, sse, http).
    #[must_use]
    pub fn transport(mut self, transport: impl Into<String>) -> Self {
        self.transport = Some(transport.into());
        self
    }

    /// Set the configuration scope.
    #[must_use]
    pub fn scope(mut self, scope: Scope) -> Self {
        self.scope = Some(scope);
        self
    }

    /// Add an environment variable.
    #[must_use]
    pub fn env(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.env.push((key.into(), value.into()));
        self
    }

    /// Add a header (for HTTP/SSE transport).
    #[must_use]
    pub fn header(mut self, header: impl Into<String>) -> Self {
        self.headers.push(header.into());
        self
    }

    /// Set additional arguments for the server command.
    #[must_use]
    pub fn server_args(mut self, args: impl IntoIterator<Item = impl Into<String>>) -> Self {
        self.server_args.extend(args.into_iter().map(Into::into));
        self
    }
}

impl ClaudeCommand for McpAddCommand {
    type Output = CommandOutput;

    fn args(&self) -> Vec<String> {
        let mut args = vec!["mcp".to_string(), "add".to_string()];

        if let Some(ref transport) = self.transport {
            args.push("--transport".to_string());
            args.push(transport.clone());
        }

        if let Some(ref scope) = self.scope {
            args.push("--scope".to_string());
            args.push(scope.as_arg().to_string());
        }

        for (key, value) in &self.env {
            args.push("-e".to_string());
            args.push(format!("{key}={value}"));
        }

        for header in &self.headers {
            args.push("-H".to_string());
            args.push(header.clone());
        }

        args.push(self.name.clone());
        args.push(self.command_or_url.clone());

        if !self.server_args.is_empty() {
            args.push("--".to_string());
            args.extend(self.server_args.clone());
        }

        args
    }

    async fn execute(&self, claude: &Claude) -> Result<CommandOutput> {
        exec::run_claude(claude, self.args()).await
    }
}

/// Add an MCP server using raw JSON configuration.
#[derive(Debug, Clone)]
pub struct McpAddJsonCommand {
    name: String,
    json: String,
    scope: Option<Scope>,
}

impl McpAddJsonCommand {
    /// Create a new MCP add-json command.
    #[must_use]
    pub fn new(name: impl Into<String>, json: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            json: json.into(),
            scope: None,
        }
    }

    /// Set the configuration scope.
    #[must_use]
    pub fn scope(mut self, scope: Scope) -> Self {
        self.scope = Some(scope);
        self
    }
}

impl ClaudeCommand for McpAddJsonCommand {
    type Output = CommandOutput;

    fn args(&self) -> Vec<String> {
        let mut args = vec!["mcp".to_string(), "add-json".to_string()];

        if let Some(ref scope) = self.scope {
            args.push("--scope".to_string());
            args.push(scope.as_arg().to_string());
        }

        args.push(self.name.clone());
        args.push(self.json.clone());

        args
    }

    async fn execute(&self, claude: &Claude) -> Result<CommandOutput> {
        exec::run_claude(claude, self.args()).await
    }
}

/// Remove an MCP server.
#[derive(Debug, Clone)]
pub struct McpRemoveCommand {
    name: String,
    scope: Option<Scope>,
}

impl McpRemoveCommand {
    #[must_use]
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            scope: None,
        }
    }

    /// Set the configuration scope.
    #[must_use]
    pub fn scope(mut self, scope: Scope) -> Self {
        self.scope = Some(scope);
        self
    }
}

impl ClaudeCommand for McpRemoveCommand {
    type Output = CommandOutput;

    fn args(&self) -> Vec<String> {
        let mut args = vec!["mcp".to_string(), "remove".to_string()];

        if let Some(ref scope) = self.scope {
            args.push("--scope".to_string());
            args.push(scope.as_arg().to_string());
        }

        args.push(self.name.clone());

        args
    }

    async fn execute(&self, claude: &Claude) -> Result<CommandOutput> {
        exec::run_claude(claude, self.args()).await
    }
}

/// Import MCP servers from Claude Desktop (Mac and WSL only).
#[derive(Debug, Clone, Default)]
pub struct McpAddFromDesktopCommand {
    scope: Option<Scope>,
}

impl McpAddFromDesktopCommand {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Set the configuration scope.
    #[must_use]
    pub fn scope(mut self, scope: Scope) -> Self {
        self.scope = Some(scope);
        self
    }
}

impl ClaudeCommand for McpAddFromDesktopCommand {
    type Output = CommandOutput;

    fn args(&self) -> Vec<String> {
        let mut args = vec!["mcp".to_string(), "add-from-claude-desktop".to_string()];
        if let Some(ref scope) = self.scope {
            args.push("--scope".to_string());
            args.push(scope.as_arg().to_string());
        }
        args
    }

    async fn execute(&self, claude: &Claude) -> Result<CommandOutput> {
        exec::run_claude(claude, self.args()).await
    }
}

/// Reset all approved and rejected project-scoped MCP servers.
#[derive(Debug, Clone, Default)]
pub struct McpResetProjectChoicesCommand;

impl McpResetProjectChoicesCommand {
    #[must_use]
    pub fn new() -> Self {
        Self
    }
}

impl ClaudeCommand for McpResetProjectChoicesCommand {
    type Output = CommandOutput;

    fn args(&self) -> Vec<String> {
        vec!["mcp".to_string(), "reset-project-choices".to_string()]
    }

    async fn execute(&self, claude: &Claude) -> Result<CommandOutput> {
        exec::run_claude(claude, self.args()).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_mcp_list_args() {
        let cmd = McpListCommand::new();
        assert_eq!(cmd.args(), vec!["mcp", "list"]);
    }

    #[test]
    fn test_mcp_get_args() {
        let cmd = McpGetCommand::new("my-server");
        assert_eq!(cmd.args(), vec!["mcp", "get", "my-server"]);
    }

    #[test]
    fn test_mcp_add_http() {
        let cmd = McpAddCommand::new("sentry", "https://mcp.sentry.dev/mcp")
            .transport("http")
            .scope(Scope::User);

        let args = cmd.args();
        assert_eq!(
            args,
            vec![
                "mcp",
                "add",
                "--transport",
                "http",
                "--scope",
                "user",
                "sentry",
                "https://mcp.sentry.dev/mcp"
            ]
        );
    }

    #[test]
    fn test_mcp_add_stdio_with_env() {
        let cmd = McpAddCommand::new("my-server", "npx")
            .env("API_KEY", "xxx")
            .server_args(["my-mcp-server"]);

        let args = cmd.args();
        assert_eq!(
            args,
            vec![
                "mcp",
                "add",
                "-e",
                "API_KEY=xxx",
                "my-server",
                "npx",
                "--",
                "my-mcp-server"
            ]
        );
    }

    #[test]
    fn test_mcp_remove_args() {
        let cmd = McpRemoveCommand::new("old-server").scope(Scope::Project);
        assert_eq!(
            cmd.args(),
            vec!["mcp", "remove", "--scope", "project", "old-server"]
        );
    }

    #[test]
    fn test_mcp_add_from_desktop() {
        let cmd = McpAddFromDesktopCommand::new().scope(Scope::User);
        assert_eq!(
            cmd.args(),
            vec!["mcp", "add-from-claude-desktop", "--scope", "user"]
        );
    }

    #[test]
    fn test_mcp_reset_project_choices() {
        let cmd = McpResetProjectChoicesCommand::new();
        assert_eq!(cmd.args(), vec!["mcp", "reset-project-choices"]);
    }
}
