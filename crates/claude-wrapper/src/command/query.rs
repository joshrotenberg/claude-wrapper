use crate::Claude;
use crate::command::ClaudeCommand;
use crate::error::Result;
use crate::exec::{self, CommandOutput};
use crate::types::{Effort, InputFormat, OutputFormat, PermissionMode};

/// Builder for `claude -p <prompt>` (oneshot print-mode queries).
///
/// This is the primary command for programmatic use. It runs a single
/// prompt through Claude and returns the result.
///
/// # Example
///
/// ```no_run
/// use claude_wrapper::{Claude, ClaudeCommand, QueryCommand, OutputFormat};
///
/// # async fn example() -> claude_wrapper::Result<()> {
/// let claude = Claude::builder().build()?;
///
/// let output = QueryCommand::new("explain this error: file not found")
///     .model("sonnet")
///     .output_format(OutputFormat::Json)
///     .max_turns(1)
///     .execute(&claude)
///     .await?;
/// # Ok(())
/// # }
/// ```
#[derive(Debug, Clone)]
pub struct QueryCommand {
    prompt: String,
    model: Option<String>,
    system_prompt: Option<String>,
    append_system_prompt: Option<String>,
    output_format: Option<OutputFormat>,
    max_budget_usd: Option<f64>,
    permission_mode: Option<PermissionMode>,
    allowed_tools: Vec<String>,
    disallowed_tools: Vec<String>,
    mcp_config: Vec<String>,
    add_dir: Vec<String>,
    effort: Option<Effort>,
    max_turns: Option<u32>,
    json_schema: Option<String>,
    continue_session: bool,
    resume: Option<String>,
    session_id: Option<String>,
    fallback_model: Option<String>,
    no_session_persistence: bool,
    dangerously_skip_permissions: bool,
    agent: Option<String>,
    agents_json: Option<String>,
    tools: Vec<String>,
    file: Vec<String>,
    include_partial_messages: bool,
    input_format: Option<InputFormat>,
    strict_mcp_config: bool,
    settings: Option<String>,
    fork_session: bool,
    retry_policy: Option<crate::retry::RetryPolicy>,
}

impl QueryCommand {
    /// Create a new query command with the given prompt.
    #[must_use]
    pub fn new(prompt: impl Into<String>) -> Self {
        Self {
            prompt: prompt.into(),
            model: None,
            system_prompt: None,
            append_system_prompt: None,
            output_format: None,
            max_budget_usd: None,
            permission_mode: None,
            allowed_tools: Vec::new(),
            disallowed_tools: Vec::new(),
            mcp_config: Vec::new(),
            add_dir: Vec::new(),
            effort: None,
            max_turns: None,
            json_schema: None,
            continue_session: false,
            resume: None,
            session_id: None,
            fallback_model: None,
            no_session_persistence: false,
            dangerously_skip_permissions: false,
            agent: None,
            agents_json: None,
            tools: Vec::new(),
            file: Vec::new(),
            include_partial_messages: false,
            input_format: None,
            strict_mcp_config: false,
            settings: None,
            fork_session: false,
            retry_policy: None,
        }
    }

    /// Set the model to use (e.g. "sonnet", "opus", or a full model ID).
    #[must_use]
    pub fn model(mut self, model: impl Into<String>) -> Self {
        self.model = Some(model.into());
        self
    }

    /// Set a custom system prompt (replaces the default).
    #[must_use]
    pub fn system_prompt(mut self, prompt: impl Into<String>) -> Self {
        self.system_prompt = Some(prompt.into());
        self
    }

    /// Append to the default system prompt.
    #[must_use]
    pub fn append_system_prompt(mut self, prompt: impl Into<String>) -> Self {
        self.append_system_prompt = Some(prompt.into());
        self
    }

    /// Set the output format.
    #[must_use]
    pub fn output_format(mut self, format: OutputFormat) -> Self {
        self.output_format = Some(format);
        self
    }

    /// Set the maximum budget in USD.
    #[must_use]
    pub fn max_budget_usd(mut self, budget: f64) -> Self {
        self.max_budget_usd = Some(budget);
        self
    }

    /// Set the permission mode.
    #[must_use]
    pub fn permission_mode(mut self, mode: PermissionMode) -> Self {
        self.permission_mode = Some(mode);
        self
    }

    /// Add allowed tools (e.g. "Bash", "Read", "mcp__my-server__*").
    #[must_use]
    pub fn allowed_tools(mut self, tools: impl IntoIterator<Item = impl Into<String>>) -> Self {
        self.allowed_tools.extend(tools.into_iter().map(Into::into));
        self
    }

    /// Add a single allowed tool.
    #[must_use]
    pub fn allowed_tool(mut self, tool: impl Into<String>) -> Self {
        self.allowed_tools.push(tool.into());
        self
    }

    /// Add disallowed tools.
    #[must_use]
    pub fn disallowed_tools(mut self, tools: impl IntoIterator<Item = impl Into<String>>) -> Self {
        self.disallowed_tools
            .extend(tools.into_iter().map(Into::into));
        self
    }

    /// Add an MCP config file path.
    #[must_use]
    pub fn mcp_config(mut self, path: impl Into<String>) -> Self {
        self.mcp_config.push(path.into());
        self
    }

    /// Add an additional directory for tool access.
    #[must_use]
    pub fn add_dir(mut self, dir: impl Into<String>) -> Self {
        self.add_dir.push(dir.into());
        self
    }

    /// Set the effort level.
    #[must_use]
    pub fn effort(mut self, effort: Effort) -> Self {
        self.effort = Some(effort);
        self
    }

    /// Set the maximum number of turns.
    #[must_use]
    pub fn max_turns(mut self, turns: u32) -> Self {
        self.max_turns = Some(turns);
        self
    }

    /// Set a JSON schema for structured output validation.
    #[must_use]
    pub fn json_schema(mut self, schema: impl Into<String>) -> Self {
        self.json_schema = Some(schema.into());
        self
    }

    /// Continue the most recent conversation.
    #[must_use]
    pub fn continue_session(mut self) -> Self {
        self.continue_session = true;
        self
    }

    /// Resume a specific session by ID.
    #[must_use]
    pub fn resume(mut self, session_id: impl Into<String>) -> Self {
        self.resume = Some(session_id.into());
        self
    }

    /// Use a specific session ID.
    #[must_use]
    pub fn session_id(mut self, id: impl Into<String>) -> Self {
        self.session_id = Some(id.into());
        self
    }

    /// Set a fallback model for when the primary model is overloaded.
    #[must_use]
    pub fn fallback_model(mut self, model: impl Into<String>) -> Self {
        self.fallback_model = Some(model.into());
        self
    }

    /// Disable session persistence (sessions won't be saved to disk).
    #[must_use]
    pub fn no_session_persistence(mut self) -> Self {
        self.no_session_persistence = true;
        self
    }

    /// Bypass all permission checks. Only use in sandboxed environments.
    #[must_use]
    pub fn dangerously_skip_permissions(mut self) -> Self {
        self.dangerously_skip_permissions = true;
        self
    }

    /// Set the agent for the session.
    #[must_use]
    pub fn agent(mut self, agent: impl Into<String>) -> Self {
        self.agent = Some(agent.into());
        self
    }

    /// Set custom agents as a JSON object.
    ///
    /// Example: `{"reviewer": {"description": "Reviews code", "prompt": "You are a code reviewer"}}`
    #[must_use]
    pub fn agents_json(mut self, json: impl Into<String>) -> Self {
        self.agents_json = Some(json.into());
        self
    }

    /// Set the list of available built-in tools.
    ///
    /// Use `""` to disable all tools, `"default"` for all tools, or
    /// specific tool names like `["Bash", "Edit", "Read"]`.
    /// This is different from `allowed_tools` which controls MCP tool permissions.
    #[must_use]
    pub fn tools(mut self, tools: impl IntoIterator<Item = impl Into<String>>) -> Self {
        self.tools.extend(tools.into_iter().map(Into::into));
        self
    }

    /// Add a file resource to download at startup.
    ///
    /// Format: `file_id:relative_path` (e.g. `file_abc:doc.txt`).
    #[must_use]
    pub fn file(mut self, spec: impl Into<String>) -> Self {
        self.file.push(spec.into());
        self
    }

    /// Include partial message chunks as they arrive.
    ///
    /// Only works with `--output-format stream-json`.
    #[must_use]
    pub fn include_partial_messages(mut self) -> Self {
        self.include_partial_messages = true;
        self
    }

    /// Set the input format.
    #[must_use]
    pub fn input_format(mut self, format: InputFormat) -> Self {
        self.input_format = Some(format);
        self
    }

    /// Only use MCP servers from `--mcp-config`, ignoring all other MCP configurations.
    #[must_use]
    pub fn strict_mcp_config(mut self) -> Self {
        self.strict_mcp_config = true;
        self
    }

    /// Path to a settings JSON file or a JSON string.
    #[must_use]
    pub fn settings(mut self, settings: impl Into<String>) -> Self {
        self.settings = Some(settings.into());
        self
    }

    /// When resuming, create a new session ID instead of reusing the original.
    #[must_use]
    pub fn fork_session(mut self) -> Self {
        self.fork_session = true;
        self
    }

    /// Set a per-command retry policy, overriding the client default.
    ///
    /// # Example
    ///
    /// ```no_run
    /// use claude_wrapper::{Claude, ClaudeCommand, QueryCommand, RetryPolicy};
    /// use std::time::Duration;
    ///
    /// # async fn example() -> claude_wrapper::Result<()> {
    /// let claude = Claude::builder().build()?;
    ///
    /// let output = QueryCommand::new("explain quicksort")
    ///     .retry(RetryPolicy::new()
    ///         .max_attempts(5)
    ///         .initial_backoff(Duration::from_secs(2))
    ///         .exponential()
    ///         .retry_on_timeout(true))
    ///     .execute(&claude)
    ///     .await?;
    /// # Ok(())
    /// # }
    /// ```
    #[must_use]
    pub fn retry(mut self, policy: crate::retry::RetryPolicy) -> Self {
        self.retry_policy = Some(policy);
        self
    }

    /// Return the full command as a string that could be run in a shell.
    ///
    /// Constructs a command string using the binary path from the Claude instance
    /// and the arguments from this query. Arguments containing spaces or special
    /// shell characters are shell-quoted to be safe for shell execution.
    ///
    /// # Example
    ///
    /// ```no_run
    /// use claude_wrapper::{Claude, QueryCommand};
    ///
    /// # async fn example() -> claude_wrapper::Result<()> {
    /// let claude = Claude::builder().build()?;
    ///
    /// let cmd = QueryCommand::new("explain quicksort")
    ///     .model("sonnet");
    ///
    /// let command_str = cmd.to_command_string(&claude);
    /// println!("Would run: {}", command_str);
    /// # Ok(())
    /// # }
    /// ```
    pub fn to_command_string(&self, claude: &Claude) -> String {
        let args = self.build_args();
        let quoted_args = args.iter().map(|arg| shell_quote(arg)).collect::<Vec<_>>();
        format!("{} {}", claude.binary().display(), quoted_args.join(" "))
    }

    /// Execute the query and parse the JSON result.
    ///
    /// This is a convenience method that sets `OutputFormat::Json` and
    /// deserializes the response into a [`QueryResult`](crate::types::QueryResult).
    #[cfg(feature = "json")]
    pub async fn execute_json(&self, claude: &Claude) -> Result<crate::types::QueryResult> {
        // Build args with JSON output format forced
        let mut args = self.build_args();

        // Override output format to json if not already set
        if self.output_format.is_none() {
            args.push("--output-format".to_string());
            args.push("json".to_string());
        }

        let output = exec::run_claude_with_retry(claude, args, self.retry_policy.as_ref()).await?;

        serde_json::from_str(&output.stdout).map_err(|e| crate::error::Error::Json {
            message: format!("failed to parse query result: {e}"),
            source: e,
        })
    }

    fn build_args(&self) -> Vec<String> {
        let mut args = vec!["--print".to_string()];

        if let Some(ref model) = self.model {
            args.push("--model".to_string());
            args.push(model.clone());
        }

        if let Some(ref prompt) = self.system_prompt {
            args.push("--system-prompt".to_string());
            args.push(prompt.clone());
        }

        if let Some(ref prompt) = self.append_system_prompt {
            args.push("--append-system-prompt".to_string());
            args.push(prompt.clone());
        }

        if let Some(ref format) = self.output_format {
            args.push("--output-format".to_string());
            args.push(format.as_arg().to_string());
        }

        if let Some(budget) = self.max_budget_usd {
            args.push("--max-budget-usd".to_string());
            args.push(budget.to_string());
        }

        if let Some(ref mode) = self.permission_mode {
            args.push("--permission-mode".to_string());
            args.push(mode.as_arg().to_string());
        }

        if !self.allowed_tools.is_empty() {
            args.push("--allowed-tools".to_string());
            args.push(self.allowed_tools.join(","));
        }

        if !self.disallowed_tools.is_empty() {
            args.push("--disallowed-tools".to_string());
            args.push(self.disallowed_tools.join(","));
        }

        for config in &self.mcp_config {
            args.push("--mcp-config".to_string());
            args.push(config.clone());
        }

        for dir in &self.add_dir {
            args.push("--add-dir".to_string());
            args.push(dir.clone());
        }

        if let Some(ref effort) = self.effort {
            args.push("--effort".to_string());
            args.push(effort.as_arg().to_string());
        }

        if let Some(turns) = self.max_turns {
            args.push("--max-turns".to_string());
            args.push(turns.to_string());
        }

        if let Some(ref schema) = self.json_schema {
            args.push("--json-schema".to_string());
            args.push(schema.clone());
        }

        if self.continue_session {
            args.push("--continue".to_string());
        }

        if let Some(ref session_id) = self.resume {
            args.push("--resume".to_string());
            args.push(session_id.clone());
        }

        if let Some(ref id) = self.session_id {
            args.push("--session-id".to_string());
            args.push(id.clone());
        }

        if let Some(ref model) = self.fallback_model {
            args.push("--fallback-model".to_string());
            args.push(model.clone());
        }

        if self.no_session_persistence {
            args.push("--no-session-persistence".to_string());
        }

        if self.dangerously_skip_permissions {
            args.push("--dangerously-skip-permissions".to_string());
        }

        if let Some(ref agent) = self.agent {
            args.push("--agent".to_string());
            args.push(agent.clone());
        }

        if let Some(ref agents) = self.agents_json {
            args.push("--agents".to_string());
            args.push(agents.clone());
        }

        if !self.tools.is_empty() {
            args.push("--tools".to_string());
            args.push(self.tools.join(","));
        }

        for spec in &self.file {
            args.push("--file".to_string());
            args.push(spec.clone());
        }

        if self.include_partial_messages {
            args.push("--include-partial-messages".to_string());
        }

        if let Some(ref format) = self.input_format {
            args.push("--input-format".to_string());
            args.push(format.as_arg().to_string());
        }

        if self.strict_mcp_config {
            args.push("--strict-mcp-config".to_string());
        }

        if let Some(ref settings) = self.settings {
            args.push("--settings".to_string());
            args.push(settings.clone());
        }

        if self.fork_session {
            args.push("--fork-session".to_string());
        }

        // Prompt is the positional argument at the end
        args.push(self.prompt.clone());

        args
    }
}

impl ClaudeCommand for QueryCommand {
    type Output = CommandOutput;

    fn args(&self) -> Vec<String> {
        self.build_args()
    }

    async fn execute(&self, claude: &Claude) -> Result<CommandOutput> {
        exec::run_claude_with_retry(claude, self.args(), self.retry_policy.as_ref()).await
    }
}

/// Shell-quote an argument if it contains spaces or special characters.
fn shell_quote(arg: &str) -> String {
    // Check if the argument needs quoting (contains whitespace or shell metacharacters)
    if arg.contains(|c: char| c.is_whitespace() || "\"'$\\`|;<>&()[]{}".contains(c)) {
        // Use single quotes and escape any existing single quotes
        format!("'{}'", arg.replace("'", "'\\''"))
    } else {
        arg.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_basic_query_args() {
        let cmd = QueryCommand::new("hello world");
        let args = cmd.args();
        assert_eq!(args, vec!["--print", "hello world"]);
    }

    #[test]
    fn test_full_query_args() {
        let cmd = QueryCommand::new("explain this")
            .model("sonnet")
            .system_prompt("be concise")
            .output_format(OutputFormat::Json)
            .max_budget_usd(0.50)
            .permission_mode(PermissionMode::BypassPermissions)
            .allowed_tools(["Bash", "Read"])
            .mcp_config("/tmp/mcp.json")
            .effort(Effort::High)
            .max_turns(3)
            .no_session_persistence();

        let args = cmd.args();
        assert!(args.contains(&"--print".to_string()));
        assert!(args.contains(&"--model".to_string()));
        assert!(args.contains(&"sonnet".to_string()));
        assert!(args.contains(&"--system-prompt".to_string()));
        assert!(args.contains(&"--output-format".to_string()));
        assert!(args.contains(&"json".to_string()));
        assert!(args.contains(&"--max-budget-usd".to_string()));
        assert!(args.contains(&"--permission-mode".to_string()));
        assert!(args.contains(&"bypassPermissions".to_string()));
        assert!(args.contains(&"--allowed-tools".to_string()));
        assert!(args.contains(&"Bash,Read".to_string()));
        assert!(args.contains(&"--effort".to_string()));
        assert!(args.contains(&"high".to_string()));
        assert!(args.contains(&"--max-turns".to_string()));
        assert!(args.contains(&"--no-session-persistence".to_string()));
        // Prompt is last
        assert_eq!(args.last().unwrap(), "explain this");
    }

    #[test]
    fn test_to_command_string_simple() {
        let claude = Claude::builder()
            .binary("/usr/local/bin/claude")
            .build()
            .unwrap();

        let cmd = QueryCommand::new("hello");
        let command_str = cmd.to_command_string(&claude);

        assert!(command_str.starts_with("/usr/local/bin/claude"));
        assert!(command_str.contains("--print"));
        assert!(command_str.contains("hello"));
    }

    #[test]
    fn test_to_command_string_with_spaces() {
        let claude = Claude::builder()
            .binary("/usr/local/bin/claude")
            .build()
            .unwrap();

        let cmd = QueryCommand::new("hello world").model("sonnet");
        let command_str = cmd.to_command_string(&claude);

        assert!(command_str.starts_with("/usr/local/bin/claude"));
        assert!(command_str.contains("--print"));
        // Prompt with spaces should be quoted
        assert!(command_str.contains("'hello world'"));
        assert!(command_str.contains("--model"));
        assert!(command_str.contains("sonnet"));
    }

    #[test]
    fn test_to_command_string_with_special_chars() {
        let claude = Claude::builder()
            .binary("/usr/local/bin/claude")
            .build()
            .unwrap();

        let cmd = QueryCommand::new("test $VAR and `cmd`");
        let command_str = cmd.to_command_string(&claude);

        // Arguments with special shell characters should be quoted
        assert!(command_str.contains("'test $VAR and `cmd`'"));
    }

    #[test]
    fn test_to_command_string_with_single_quotes() {
        let claude = Claude::builder()
            .binary("/usr/local/bin/claude")
            .build()
            .unwrap();

        let cmd = QueryCommand::new("it's");
        let command_str = cmd.to_command_string(&claude);

        // Single quotes should be escaped in shell
        assert!(command_str.contains("'it'\\''s'"));
    }
}
