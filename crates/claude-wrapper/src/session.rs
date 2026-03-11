//! Type-safe session management for multi-turn conversations.
//!
//! The [`Session`] struct consolidates session control into a single abstraction
//! that prevents conflicting session flags at the type level. Instead of
//! independently calling `.continue_session()`, `.resume()`, `.session_id()`,
//! or `.fork_session()` on a [`QueryCommand`] (which can
//! be combined incorrectly), a `Session` encodes the session mode in its
//! construction and provides `.query()` with automatic resume behavior.
//!
//! # Example
//!
//! ```no_run
//! use claude_wrapper::{Claude, QueryCommand};
//! use claude_wrapper::session::Session;
//!
//! # async fn example() -> claude_wrapper::Result<()> {
//! let claude = Claude::builder().build()?;
//!
//! // Start a session with an initial query
//! let first = QueryCommand::new("explain quicksort")
//!     .execute_json(&claude)
//!     .await?;
//!
//! // Wrap it in a Session for automatic resume
//! let mut session = Session::from_result(&claude, &first);
//!
//! // Follow-up queries auto-resume the session
//! let second = session.query("now explain mergesort")
//!     .model("sonnet")
//!     .execute()
//!     .await?;
//!
//! println!("total cost: ${:.4}", session.total_cost_usd());
//! println!("total turns: {}", session.total_turns());
//! # Ok(())
//! # }
//! ```

use crate::Claude;
use crate::command::query::QueryCommand;
use crate::error::Result;
use crate::types::{Effort, InputFormat, OutputFormat, PermissionMode, QueryResult};

/// A type-safe session handle for multi-turn conversations.
///
/// `Session` wraps a [`Claude`] client reference and a session ID, providing
/// `.query()` that automatically resumes the session. It tracks cumulative
/// cost and turn count across all queries in the session.
///
/// Conflicting session flags are impossible because the session mode is
/// encoded in construction rather than as independent builder methods.
#[derive(Debug)]
pub struct Session<'a> {
    claude: &'a Claude,
    session_id: String,
    cumulative_cost_usd: f64,
    cumulative_turns: u32,
}

impl<'a> Session<'a> {
    /// Create a session from a completed query result.
    ///
    /// This is the most common way to start a session: run an initial
    /// [`QueryCommand::execute_json()`] and then wrap the result.
    ///
    /// # Example
    ///
    /// ```no_run
    /// use claude_wrapper::{Claude, QueryCommand};
    /// use claude_wrapper::session::Session;
    ///
    /// # async fn example() -> claude_wrapper::Result<()> {
    /// let claude = Claude::builder().build()?;
    /// let result = QueryCommand::new("hello")
    ///     .execute_json(&claude).await?;
    ///
    /// let mut session = Session::from_result(&claude, &result);
    /// # Ok(())
    /// # }
    /// ```
    pub fn from_result(claude: &'a Claude, result: &QueryResult) -> Self {
        Self {
            claude,
            session_id: result.session_id.clone(),
            cumulative_cost_usd: result.cost_usd.unwrap_or(0.0),
            cumulative_turns: result.num_turns.unwrap_or(0),
        }
    }

    /// Attach to an existing session by ID.
    ///
    /// Cost and turn counters start at zero since we have no history.
    pub fn from_id(claude: &'a Claude, session_id: impl Into<String>) -> Self {
        Self {
            claude,
            session_id: session_id.into(),
            cumulative_cost_usd: 0.0,
            cumulative_turns: 0,
        }
    }

    /// Continue the most recent session.
    ///
    /// Runs the first query with `--continue` to discover the session ID,
    /// then returns a `Session` that uses `--resume` for subsequent queries.
    pub async fn continue_recent(
        claude: &'a Claude,
        prompt: impl Into<String>,
    ) -> Result<(Self, QueryResult)> {
        let result = QueryCommand::new(prompt)
            .continue_session()
            .execute_json(claude)
            .await?;

        let session = Self {
            claude,
            session_id: result.session_id.clone(),
            cumulative_cost_usd: result.cost_usd.unwrap_or(0.0),
            cumulative_turns: result.num_turns.unwrap_or(0),
        };
        Ok((session, result))
    }

    /// Send a follow-up query in this session.
    ///
    /// Returns a [`SessionQuery`] builder with `--resume` pre-set.
    /// Configure additional options (model, effort, etc.) on the builder,
    /// then call `.execute()`.
    ///
    /// # Example
    ///
    /// ```no_run
    /// # use claude_wrapper::{Claude, QueryCommand};
    /// # use claude_wrapper::session::Session;
    /// # async fn example() -> claude_wrapper::Result<()> {
    /// # let claude = Claude::builder().build()?;
    /// # let result = QueryCommand::new("hello").execute_json(&claude).await?;
    /// let mut session = Session::from_result(&claude, &result);
    ///
    /// let follow_up = session.query("what about the edge cases?")
    ///     .model("opus")
    ///     .max_turns(5)
    ///     .execute()
    ///     .await?;
    /// # Ok(())
    /// # }
    /// ```
    pub fn query(&mut self, prompt: impl Into<String>) -> SessionQuery<'_, 'a> {
        SessionQuery::new(self, prompt)
    }

    /// Fork this session into a new one.
    ///
    /// Sends a query with `--resume` and `--fork-session`, creating a new
    /// session branched from this one. Returns the new `Session` and the
    /// query result. The original session is not modified.
    pub async fn fork(&self, prompt: impl Into<String>) -> Result<(Session<'a>, QueryResult)> {
        let result = QueryCommand::new(prompt)
            .resume(&self.session_id)
            .fork_session()
            .execute_json(self.claude)
            .await?;

        let forked = Session {
            claude: self.claude,
            session_id: result.session_id.clone(),
            cumulative_cost_usd: self.cumulative_cost_usd + result.cost_usd.unwrap_or(0.0),
            cumulative_turns: self.cumulative_turns + result.num_turns.unwrap_or(0),
        };
        Ok((forked, result))
    }

    /// Get the current session ID.
    pub fn id(&self) -> &str {
        &self.session_id
    }

    /// Get cumulative cost in USD across all queries in this session.
    pub fn total_cost_usd(&self) -> f64 {
        self.cumulative_cost_usd
    }

    /// Get cumulative turn count across all queries in this session.
    pub fn total_turns(&self) -> u32 {
        self.cumulative_turns
    }
}

/// Builder for a follow-up query within a session.
///
/// This wraps a [`QueryCommand`] with `--resume` pre-set. Session-related
/// methods (`.continue_session()`, `.session_id()`, `.fork_session()`,
/// `.resume()`) are intentionally not exposed, preventing conflicting flags
/// at the type level.
///
/// All other `QueryCommand` options are available via delegation.
#[derive(Debug)]
pub struct SessionQuery<'s, 'a> {
    session: &'s mut Session<'a>,
    command: QueryCommand,
}

impl<'s, 'a> SessionQuery<'s, 'a> {
    fn new(session: &'s mut Session<'a>, prompt: impl Into<String>) -> Self {
        let command = QueryCommand::new(prompt).resume(&session.session_id);
        Self { session, command }
    }

    /// Set the model to use.
    #[must_use]
    pub fn model(mut self, model: impl Into<String>) -> Self {
        self.command = self.command.model(model);
        self
    }

    /// Set a custom system prompt.
    #[must_use]
    pub fn system_prompt(mut self, prompt: impl Into<String>) -> Self {
        self.command = self.command.system_prompt(prompt);
        self
    }

    /// Append to the default system prompt.
    #[must_use]
    pub fn append_system_prompt(mut self, prompt: impl Into<String>) -> Self {
        self.command = self.command.append_system_prompt(prompt);
        self
    }

    /// Set the output format.
    #[must_use]
    pub fn output_format(mut self, format: OutputFormat) -> Self {
        self.command = self.command.output_format(format);
        self
    }

    /// Set the maximum budget in USD.
    #[must_use]
    pub fn max_budget_usd(mut self, budget: f64) -> Self {
        self.command = self.command.max_budget_usd(budget);
        self
    }

    /// Set the permission mode.
    #[must_use]
    pub fn permission_mode(mut self, mode: PermissionMode) -> Self {
        self.command = self.command.permission_mode(mode);
        self
    }

    /// Add allowed tools.
    #[must_use]
    pub fn allowed_tools(mut self, tools: impl IntoIterator<Item = impl Into<String>>) -> Self {
        self.command = self.command.allowed_tools(tools);
        self
    }

    /// Add a single allowed tool.
    #[must_use]
    pub fn allowed_tool(mut self, tool: impl Into<String>) -> Self {
        self.command = self.command.allowed_tool(tool);
        self
    }

    /// Add disallowed tools.
    #[must_use]
    pub fn disallowed_tools(mut self, tools: impl IntoIterator<Item = impl Into<String>>) -> Self {
        self.command = self.command.disallowed_tools(tools);
        self
    }

    /// Add an MCP config file path.
    #[must_use]
    pub fn mcp_config(mut self, path: impl Into<String>) -> Self {
        self.command = self.command.mcp_config(path);
        self
    }

    /// Add an additional directory for tool access.
    #[must_use]
    pub fn add_dir(mut self, dir: impl Into<String>) -> Self {
        self.command = self.command.add_dir(dir);
        self
    }

    /// Set the effort level.
    #[must_use]
    pub fn effort(mut self, effort: Effort) -> Self {
        self.command = self.command.effort(effort);
        self
    }

    /// Set the maximum number of turns.
    #[must_use]
    pub fn max_turns(mut self, turns: u32) -> Self {
        self.command = self.command.max_turns(turns);
        self
    }

    /// Set a JSON schema for structured output validation.
    #[must_use]
    pub fn json_schema(mut self, schema: impl Into<String>) -> Self {
        self.command = self.command.json_schema(schema);
        self
    }

    /// Set a fallback model.
    #[must_use]
    pub fn fallback_model(mut self, model: impl Into<String>) -> Self {
        self.command = self.command.fallback_model(model);
        self
    }

    /// Disable session persistence.
    #[must_use]
    pub fn no_session_persistence(mut self) -> Self {
        self.command = self.command.no_session_persistence();
        self
    }

    /// Bypass all permission checks.
    #[must_use]
    pub fn dangerously_skip_permissions(mut self) -> Self {
        self.command = self.command.dangerously_skip_permissions();
        self
    }

    /// Set the agent for the session.
    #[must_use]
    pub fn agent(mut self, agent: impl Into<String>) -> Self {
        self.command = self.command.agent(agent);
        self
    }

    /// Set custom agents as a JSON object.
    #[must_use]
    pub fn agents_json(mut self, json: impl Into<String>) -> Self {
        self.command = self.command.agents_json(json);
        self
    }

    /// Set the list of available built-in tools.
    #[must_use]
    pub fn tools(mut self, tools: impl IntoIterator<Item = impl Into<String>>) -> Self {
        self.command = self.command.tools(tools);
        self
    }

    /// Add a file resource to download at startup.
    #[must_use]
    pub fn file(mut self, spec: impl Into<String>) -> Self {
        self.command = self.command.file(spec);
        self
    }

    /// Include partial message chunks as they arrive.
    #[must_use]
    pub fn include_partial_messages(mut self) -> Self {
        self.command = self.command.include_partial_messages();
        self
    }

    /// Set the input format.
    #[must_use]
    pub fn input_format(mut self, format: InputFormat) -> Self {
        self.command = self.command.input_format(format);
        self
    }

    /// Only use MCP servers from `--mcp-config`.
    #[must_use]
    pub fn strict_mcp_config(mut self) -> Self {
        self.command = self.command.strict_mcp_config();
        self
    }

    /// Path to a settings JSON file or a JSON string.
    #[must_use]
    pub fn settings(mut self, settings: impl Into<String>) -> Self {
        self.command = self.command.settings(settings);
        self
    }

    /// Set a per-command retry policy.
    #[must_use]
    pub fn retry(mut self, policy: crate::retry::RetryPolicy) -> Self {
        self.command = self.command.retry(policy);
        self
    }

    /// Execute the query, updating the session's cumulative cost and turns.
    pub async fn execute(self) -> Result<QueryResult> {
        let result = self.command.execute_json(self.session.claude).await?;
        self.session.cumulative_cost_usd += result.cost_usd.unwrap_or(0.0);
        self.session.cumulative_turns += result.num_turns.unwrap_or(0);
        self.session.session_id.clone_from(&result.session_id);
        Ok(result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ClaudeCommand;

    fn test_claude() -> Claude {
        Claude::builder()
            .binary("/usr/local/bin/claude")
            .build()
            .unwrap()
    }

    fn test_result(session_id: &str, cost: f64, turns: u32) -> QueryResult {
        QueryResult {
            result: "test".into(),
            session_id: session_id.into(),
            cost_usd: Some(cost),
            duration_ms: None,
            num_turns: Some(turns),
            is_error: false,
            extra: Default::default(),
        }
    }

    #[test]
    fn session_from_result_captures_state() {
        let claude = test_claude();
        let result = test_result("sess-abc", 0.05, 3);
        let session = Session::from_result(&claude, &result);

        assert_eq!(session.id(), "sess-abc");
        assert!((session.total_cost_usd() - 0.05).abs() < f64::EPSILON);
        assert_eq!(session.total_turns(), 3);
    }

    #[test]
    fn session_from_id_starts_clean() {
        let claude = test_claude();
        let session = Session::from_id(&claude, "sess-xyz");

        assert_eq!(session.id(), "sess-xyz");
        assert!((session.total_cost_usd()).abs() < f64::EPSILON);
        assert_eq!(session.total_turns(), 0);
    }

    #[test]
    fn session_from_result_handles_none_cost_and_turns() {
        let claude = test_claude();
        let result = QueryResult {
            result: "ok".into(),
            session_id: "s1".into(),
            cost_usd: None,
            duration_ms: None,
            num_turns: None,
            is_error: false,
            extra: Default::default(),
        };
        let session = Session::from_result(&claude, &result);

        assert_eq!(session.total_cost_usd(), 0.0);
        assert_eq!(session.total_turns(), 0);
    }

    #[test]
    fn session_query_sets_resume_flag() {
        let claude = test_claude();
        let mut session = Session::from_id(&claude, "sess-123");
        let sq = session.query("follow up");

        let args = sq.command.args();
        assert!(args.contains(&"--resume".to_string()));
        assert!(args.contains(&"sess-123".to_string()));
    }

    #[test]
    fn session_query_model_delegation() {
        let claude = test_claude();
        let mut session = Session::from_id(&claude, "sess-123");
        let sq = session.query("follow up").model("sonnet");

        let args = sq.command.args();
        assert!(args.contains(&"--model".to_string()));
        assert!(args.contains(&"sonnet".to_string()));
    }

    #[test]
    fn session_query_effort_delegation() {
        let claude = test_claude();
        let mut session = Session::from_id(&claude, "sess-123");
        let sq = session.query("follow up").effort(Effort::High);

        let args = sq.command.args();
        assert!(args.contains(&"--effort".to_string()));
        assert!(args.contains(&"high".to_string()));
    }

    #[test]
    fn session_query_max_turns_delegation() {
        let claude = test_claude();
        let mut session = Session::from_id(&claude, "sess-123");
        let sq = session.query("follow up").max_turns(10);

        let args = sq.command.args();
        assert!(args.contains(&"--max-turns".to_string()));
        assert!(args.contains(&"10".to_string()));
    }

    #[test]
    fn session_query_prompt_is_last_arg() {
        let claude = test_claude();
        let mut session = Session::from_id(&claude, "sess-123");
        let sq = session.query("my prompt");

        let args = sq.command.args();
        assert_eq!(args.last().unwrap(), "my prompt");
    }

    #[test]
    fn session_query_does_not_have_continue_or_fork() {
        // This is a compile-time check: SessionQuery does not expose
        // .continue_session(), .session_id(), .fork_session(), or .resume().
        // If any of those methods existed on SessionQuery, they would appear
        // in the API. We verify structurally by checking that the inner
        // command only has --resume set (no --continue, --fork-session, --session-id).
        let claude = test_claude();
        let mut session = Session::from_id(&claude, "sess-123");
        let sq = session.query("test");

        let args = sq.command.args();
        assert!(!args.contains(&"--continue".to_string()));
        assert!(!args.contains(&"--fork-session".to_string()));
        assert!(!args.contains(&"--session-id".to_string()));
    }
}
