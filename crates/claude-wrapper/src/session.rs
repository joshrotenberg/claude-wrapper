//! Multi-turn session management.
//!
//! A [`Session`] threads Claude's `session_id` across turns automatically,
//! so callers never need to scrape it out of a result event or pass
//! `--resume` by hand.
//!
//! # Ownership
//!
//! [`Session`] holds an `Arc<Claude>`. Wrapping the client in an `Arc`
//! means a session can outlive the original client binding, be moved
//! between tasks, and sit inside long-lived actor state -- which is the
//! usage shape that callers like centralino-rs need. One `Arc::clone`
//! per session is a negligible cost in exchange.
//!
//! # Two entry points
//!
//! - [`Session::send`] takes a plain prompt. Use this for straightforward
//!   multi-turn chat.
//! - [`Session::execute`] takes a fully-configured [`QueryCommand`]. Use
//!   this when you want per-turn options like `model`, `max_turns`,
//!   `permission_mode`, etc. The session automatically overrides any
//!   session-related flags on the command (`--resume`, `--continue`,
//!   `--session-id`, `--fork-session`) so they can't conflict.
//!
//! Streaming follows the same split: [`Session::stream`] and
//! [`Session::stream_execute`].
//!
//! # Example
//!
//! ```no_run
//! use std::sync::Arc;
//! use claude_wrapper::{Claude, QueryCommand};
//! use claude_wrapper::session::Session;
//!
//! # async fn example() -> claude_wrapper::Result<()> {
//! let claude = Arc::new(Claude::builder().build()?);
//!
//! let mut session = Session::new(Arc::clone(&claude));
//!
//! // Simple path
//! let first = session.send("explain quicksort").await?;
//!
//! // Full control: custom model, effort, permission mode, etc.
//! let second = session
//!     .execute(QueryCommand::new("now mergesort").model("opus"))
//!     .await?;
//!
//! println!("total cost: ${:.4}", session.total_cost_usd());
//! println!("turns: {}", session.total_turns());
//! # Ok(())
//! # }
//! ```
//!
//! # Resuming an existing session
//!
//! ```no_run
//! # use std::sync::Arc;
//! # use claude_wrapper::{Claude};
//! # use claude_wrapper::session::Session;
//! # async fn example() -> claude_wrapper::Result<()> {
//! # let claude = Arc::new(Claude::builder().build()?);
//! // Reattach to a session you stored earlier
//! let mut session = Session::resume(claude, "sess-abc123");
//! let result = session.send("pick up where we left off").await?;
//! # Ok(())
//! # }
//! ```

use std::sync::Arc;

use crate::Claude;
use crate::budget::BudgetTracker;
use crate::command::query::QueryCommand;
use crate::error::Result;
use crate::types::QueryResult;

#[cfg(feature = "json")]
use crate::streaming::{StreamEvent, stream_query};

/// A multi-turn conversation handle.
///
/// Owns an `Arc<Claude>` so it can be moved between tasks and live
/// inside long-running actors. Tracks `session_id`, cumulative cost,
/// turn count, and per-turn result history.
#[derive(Debug, Clone)]
pub struct Session {
    claude: Arc<Claude>,
    session_id: Option<String>,
    history: Vec<QueryResult>,
    cumulative_cost_usd: f64,
    cumulative_turns: u32,
    budget: Option<BudgetTracker>,
}

impl Session {
    /// Start a fresh session. The first turn will discover a session id
    /// from its result; subsequent turns reuse it via `--resume`.
    pub fn new(claude: Arc<Claude>) -> Self {
        Self {
            claude,
            session_id: None,
            history: Vec::new(),
            cumulative_cost_usd: 0.0,
            cumulative_turns: 0,
            budget: None,
        }
    }

    /// Reattach to an existing session by id. The next turn immediately
    /// passes `--resume <id>`. Cost and turn counters start at zero
    /// since no history is available.
    pub fn resume(claude: Arc<Claude>, session_id: impl Into<String>) -> Self {
        Self {
            claude,
            session_id: Some(session_id.into()),
            history: Vec::new(),
            cumulative_cost_usd: 0.0,
            cumulative_turns: 0,
            budget: None,
        }
    }

    /// Attach a [`BudgetTracker`] to this session. Every turn's cost
    /// (from [`QueryResult::cost_usd`]) is recorded on the tracker, and
    /// [`Session::execute`]/[`Session::stream_execute`] return
    /// [`Error::BudgetExceeded`](crate::error::Error::BudgetExceeded)
    /// before dispatching a turn if the tracker's ceiling has been hit.
    ///
    /// Clone a tracker across several sessions to enforce a shared
    /// ceiling; each `Session` then sees the same running total.
    pub fn with_budget(mut self, budget: BudgetTracker) -> Self {
        self.budget = Some(budget);
        self
    }

    /// The attached [`BudgetTracker`], if any.
    pub fn budget(&self) -> Option<&BudgetTracker> {
        self.budget.as_ref()
    }

    /// Send a plain-prompt turn. Equivalent to
    /// `execute(QueryCommand::new(prompt))`.
    #[cfg(feature = "json")]
    pub async fn send(&mut self, prompt: impl Into<String>) -> Result<QueryResult> {
        self.execute(QueryCommand::new(prompt)).await
    }

    /// Send a turn with a fully-configured [`QueryCommand`].
    ///
    /// Any session-related flags on `cmd` (`--resume`, `--continue`,
    /// `--session-id`, `--fork-session`) are overridden with this
    /// session's current id, so they can't conflict.
    #[cfg(feature = "json")]
    pub async fn execute(&mut self, cmd: QueryCommand) -> Result<QueryResult> {
        if let Some(b) = &self.budget {
            b.check()?;
        }

        let cmd = match &self.session_id {
            Some(id) => cmd.replace_session(id),
            None => cmd,
        };

        let result = cmd.execute_json(&self.claude).await?;
        self.record(&result);
        Ok(result)
    }

    /// Stream a plain-prompt turn, dispatching each NDJSON event to
    /// `handler`. The session's id is captured from the first event
    /// that carries one, so subsequent turns can resume, and the id
    /// persists even if the stream errors partway through.
    #[cfg(feature = "json")]
    pub async fn stream<F>(&mut self, prompt: impl Into<String>, handler: F) -> Result<()>
    where
        F: FnMut(StreamEvent),
    {
        self.stream_execute(QueryCommand::new(prompt), handler)
            .await
    }

    /// Stream a turn with a fully-configured [`QueryCommand`], with the
    /// same session-id capture semantics as [`Session::stream`].
    ///
    /// The command's output format is forced to `stream-json` and any
    /// session-related flags are overridden as in [`Session::execute`].
    #[cfg(feature = "json")]
    pub async fn stream_execute<F>(&mut self, cmd: QueryCommand, mut handler: F) -> Result<()>
    where
        F: FnMut(StreamEvent),
    {
        use crate::types::OutputFormat;

        if let Some(b) = &self.budget {
            b.check()?;
        }

        let cmd = match &self.session_id {
            Some(id) => cmd.replace_session(id),
            None => cmd,
        }
        .output_format(OutputFormat::StreamJson);

        // Capture session_id and result state from events inside a
        // wrapper closure. The captures happen before the caller's
        // handler runs, and self is updated after the stream completes
        // (even on error) so id persists across partial failures.
        let mut captured_session_id: Option<String> = None;
        let mut captured_result: Option<QueryResult> = None;

        let outcome = {
            let wrap = |event: StreamEvent| {
                if captured_session_id.is_none()
                    && let Some(sid) = event.session_id()
                {
                    captured_session_id = Some(sid.to_string());
                }
                if event.is_result()
                    && captured_result.is_none()
                    && let Ok(qr) = serde_json::from_value::<QueryResult>(event.data.clone())
                {
                    captured_result = Some(qr);
                }
                handler(event);
            };
            stream_query(&self.claude, &cmd, wrap).await
        };

        if let Some(sid) = captured_session_id {
            self.session_id = Some(sid);
        }
        if let Some(qr) = captured_result {
            self.record(&qr);
        }

        outcome.map(|_| ())
    }

    /// Current session id, if one has been established.
    pub fn id(&self) -> Option<&str> {
        self.session_id.as_deref()
    }

    /// Cumulative cost in USD across all turns in this session.
    pub fn total_cost_usd(&self) -> f64 {
        self.cumulative_cost_usd
    }

    /// Cumulative turn count across all turns in this session.
    pub fn total_turns(&self) -> u32 {
        self.cumulative_turns
    }

    /// Full per-turn result history.
    pub fn history(&self) -> &[QueryResult] {
        &self.history
    }

    /// Result of the most recent turn, if any.
    pub fn last_result(&self) -> Option<&QueryResult> {
        self.history.last()
    }

    fn record(&mut self, result: &QueryResult) {
        self.session_id = Some(result.session_id.clone());
        let cost = result.cost_usd.unwrap_or(0.0);
        self.cumulative_cost_usd += cost;
        self.cumulative_turns += result.num_turns.unwrap_or(0);
        if let Some(b) = &self.budget {
            b.record(cost);
        }
        self.history.push(result.clone());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_claude() -> Arc<Claude> {
        Arc::new(
            Claude::builder()
                .binary("/usr/local/bin/claude")
                .build()
                .unwrap(),
        )
    }

    #[test]
    fn new_session_has_no_id() {
        let session = Session::new(test_claude());
        assert!(session.id().is_none());
        assert_eq!(session.total_cost_usd(), 0.0);
        assert_eq!(session.total_turns(), 0);
        assert!(session.history().is_empty());
        assert!(session.last_result().is_none());
    }

    #[test]
    fn resume_session_has_preset_id() {
        let session = Session::resume(test_claude(), "sess-abc");
        assert_eq!(session.id(), Some("sess-abc"));
        assert_eq!(session.total_cost_usd(), 0.0);
        assert_eq!(session.total_turns(), 0);
    }

    #[test]
    fn record_updates_state() {
        let mut session = Session::new(test_claude());
        let result = QueryResult {
            result: "ok".into(),
            session_id: "sess-1".into(),
            cost_usd: Some(0.05),
            duration_ms: None,
            num_turns: Some(3),
            is_error: false,
            extra: Default::default(),
        };
        session.record(&result);
        assert_eq!(session.id(), Some("sess-1"));
        assert!((session.total_cost_usd() - 0.05).abs() < f64::EPSILON);
        assert_eq!(session.total_turns(), 3);
        assert_eq!(session.history().len(), 1);
        assert_eq!(
            session.last_result().map(|r| r.session_id.as_str()),
            Some("sess-1")
        );
    }

    #[test]
    fn record_accumulates_across_turns() {
        let mut session = Session::new(test_claude());
        let r1 = QueryResult {
            result: "a".into(),
            session_id: "sess-1".into(),
            cost_usd: Some(0.01),
            duration_ms: None,
            num_turns: Some(2),
            is_error: false,
            extra: Default::default(),
        };
        let r2 = QueryResult {
            result: "b".into(),
            session_id: "sess-1".into(),
            cost_usd: Some(0.02),
            duration_ms: None,
            num_turns: Some(1),
            is_error: false,
            extra: Default::default(),
        };
        session.record(&r1);
        session.record(&r2);
        assert_eq!(session.total_turns(), 3);
        assert!((session.total_cost_usd() - 0.03).abs() < f64::EPSILON);
        assert_eq!(session.history().len(), 2);
    }

    #[test]
    fn record_forwards_cost_to_budget() {
        use crate::budget::BudgetTracker;

        let budget = BudgetTracker::builder().build();
        let mut session = Session::new(test_claude()).with_budget(budget.clone());

        let r = QueryResult {
            result: "ok".into(),
            session_id: "sess-1".into(),
            cost_usd: Some(0.07),
            duration_ms: None,
            num_turns: Some(1),
            is_error: false,
            extra: Default::default(),
        };
        session.record(&r);

        assert!((budget.total_usd() - 0.07).abs() < 1e-9);
        assert!((session.total_cost_usd() - 0.07).abs() < 1e-9);
    }

    #[test]
    fn budget_pre_check_would_block_next_turn() {
        use crate::budget::BudgetTracker;
        use crate::error::Error;

        // The execute() pre-check defers to BudgetTracker::check().
        // Exercise that directly with a pre-loaded tracker, so we don't
        // need a live Claude CLI.
        let budget = BudgetTracker::builder().max_usd(0.10).build();
        budget.record(0.15);

        let session = Session::new(test_claude()).with_budget(budget);
        match session.budget().unwrap().check() {
            Err(Error::BudgetExceeded { total_usd, max_usd }) => {
                assert!((total_usd - 0.15).abs() < 1e-9);
                assert!((max_usd - 0.10).abs() < 1e-9);
            }
            other => panic!("expected BudgetExceeded, got {other:?}"),
        }
    }

    #[test]
    fn replace_session_clears_conflicting_flags() {
        use crate::command::ClaudeCommand;

        // Verify that replace_session strips --continue/--session-id/
        // --fork-session and sets --resume to the given id.
        let cmd = QueryCommand::new("hi")
            .continue_session()
            .session_id("old")
            .fork_session()
            .replace_session("new-id");

        let args = cmd.args();
        assert!(args.contains(&"--resume".to_string()));
        assert!(args.contains(&"new-id".to_string()));
        assert!(!args.contains(&"--continue".to_string()));
        assert!(!args.contains(&"--session-id".to_string()));
        assert!(!args.contains(&"--fork-session".to_string()));
    }
}
