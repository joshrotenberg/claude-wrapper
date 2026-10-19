//! Multi-turn session management for short-lived processes.
//!
//! A [`Session`] threads Claude's `session_id` across turns automatically,
//! so callers never need to scrape it out of a result event or pass
//! `--resume` by hand.
//!
//! # When to use
//!
//! [`Session`] is the right fit when each turn can stand on its own
//! and the host process is short-lived: CLIs, build scripts, batch
//! jobs, lambdas. Each turn spawns a fresh `claude` subprocess and
//! resumes the conversation via `--resume <session_id>`.
//!
//! For long-running hosts (IDE backends, daemons, agent servers,
//! chat UIs) where holding a `claude` subprocess open across many
//! turns is cheap, prefer [`DuplexSession`](crate::duplex::DuplexSession).
//! It supports mid-turn interrupts, mid-turn permission decisions, and
//! a broadcast event stream that [`Session`] cannot offer because of
//! the transient subprocess model.
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
//! # Cost accounting
//!
//! The CLI's `total_cost_usd` is a running native-session total.
//! [`Session`] records its nonnegative change in both the cumulative
//! cost and any attached [`BudgetTracker`]. Missing totals leave the
//! baseline unchanged; raw totals, including missing values, remain
//! in [`Session::history`]. A falling total starts a new baseline and
//! contributes zero because spend during a reset is unknown. A
//! changed native session id starts an independent baseline.
//! A missing id keeps the last known identity and cost baseline.
//!
//! The first observed total is counted in full, including historical
//! spend restored by [`Session::resume`]. This tracks observed native
//! lifetime spend, not spend since attaching this handle. That initial
//! historical amount also reaches the attached budget tracker.
//! Buffered failures that return an [`Error`](crate::Error) before a
//! [`QueryResult`] are not recorded. A later reported total may catch
//! up that spend; inspect the error's native cost metadata when no
//! later turn is available.
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
use crate::duplex::turn_cost_delta;
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
    last_cost_session_id: Option<String>,
    last_reported_total_usd: Option<f64>,
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
            last_cost_session_id: None,
            last_reported_total_usd: None,
            budget: None,
        }
    }

    /// Reattach to an existing session by id. The next turn immediately
    /// passes `--resume <id>`. Cost and turn counters start at zero
    /// since no history is available. The first result's reported
    /// total includes the native session's historical spend, which
    /// is counted in full. See the [cost-accounting scope](crate::session#cost-accounting).
    pub fn resume(claude: Arc<Claude>, session_id: impl Into<String>) -> Self {
        let session_id = session_id.into();
        Self {
            claude,
            session_id: Some(session_id.clone()),
            history: Vec::new(),
            cumulative_cost_usd: 0.0,
            cumulative_turns: 0,
            last_cost_session_id: Some(session_id),
            last_reported_total_usd: None,
            budget: None,
        }
    }

    /// Attach a [`BudgetTracker`] to this session. Each nonnegative
    /// change in [`QueryResult::cost_usd`] is recorded on the tracker, and
    /// [`Session::execute`]/[`Session::stream_execute`] return
    /// [`crate::error::Error::BudgetExceeded`]
    /// before dispatching a turn if the tracker's ceiling has been hit.
    ///
    /// Clone a tracker across several sessions to enforce a shared
    /// ceiling; each `Session` then sees the same running total.
    /// The first observed total includes historical spend on resumed
    /// native sessions; see the [cost-accounting scope](crate::session#cost-accounting).
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
            self.observe_session_id(&sid);
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

    /// Sum of observed nonnegative changes in native cost totals.
    ///
    /// Includes the first reported total in full, including historical
    /// spend on a resumed session. Missing totals remain unknown in
    /// [`Session::history`]; see the [cost-accounting scope](crate::session#cost-accounting).
    pub fn total_cost_usd(&self) -> f64 {
        self.cumulative_cost_usd
    }

    /// Cumulative turn count across all turns in this session.
    pub fn total_turns(&self) -> u32 {
        self.cumulative_turns
    }

    /// Cumulative token count across all recorded turns, summing every
    /// bucket the CLI reported (cache and non-cache input both count;
    /// see [`TokenUsage::total`](crate::TokenUsage::total)).
    ///
    /// Turns whose result carried no usage contribute nothing here.
    /// Check [`turns_missing_usage`](Self::turns_missing_usage) to tell
    /// a genuinely low total from an incomplete one.
    pub fn total_tokens(&self) -> u64 {
        self.history
            .iter()
            .filter_map(|r| r.usage.as_ref())
            .map(crate::TokenUsage::total)
            .sum()
    }

    /// Number of recorded turns whose result carried no usage at all
    /// (no `usage` object, or one with every bucket absent). A non-zero
    /// value means [`total_tokens`](Self::total_tokens) undercounts.
    pub fn turns_missing_usage(&self) -> usize {
        self.history
            .iter()
            .filter(|r| r.usage.as_ref().is_none_or(crate::TokenUsage::is_empty))
            .count()
    }

    /// Full per-turn result history.
    pub fn history(&self) -> &[QueryResult] {
        &self.history
    }

    /// Result of the most recent turn, if any.
    pub fn last_result(&self) -> Option<&QueryResult> {
        self.history.last()
    }

    fn observe_session_id(&mut self, session_id: &str) {
        if session_id.is_empty() {
            return;
        }
        if self
            .last_cost_session_id
            .as_deref()
            .is_some_and(|previous| previous != session_id)
        {
            self.last_reported_total_usd = None;
        }
        self.last_cost_session_id = Some(session_id.to_owned());
        self.session_id = Some(session_id.to_owned());
    }

    fn record(&mut self, result: &QueryResult) {
        self.observe_session_id(&result.session_id);
        let cost =
            turn_cost_delta(&mut self.last_reported_total_usd, result.cost_usd).unwrap_or(0.0);
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
            usage: None,
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
            usage: None,
            extra: Default::default(),
        };
        let r2 = QueryResult {
            result: "b".into(),
            session_id: "sess-1".into(),
            cost_usd: Some(0.02),
            duration_ms: None,
            num_turns: Some(1),
            is_error: false,
            usage: None,
            extra: Default::default(),
        };
        session.record(&r1);
        session.record(&r2);
        assert_eq!(session.total_turns(), 3);
        assert!((session.total_cost_usd() - 0.02).abs() < f64::EPSILON);
        assert_eq!(session.history().len(), 2);
    }

    #[test]
    fn token_accounting_sums_and_flags_missing() {
        use crate::TokenUsage;

        let mut session = Session::new(test_claude());
        let with_usage = QueryResult {
            result: "a".into(),
            session_id: "sess-1".into(),
            cost_usd: Some(0.01),
            duration_ms: None,
            num_turns: Some(1),
            is_error: false,
            usage: Some(TokenUsage {
                input_tokens: Some(100),
                output_tokens: Some(25),
                ..Default::default()
            }),
            extra: Default::default(),
        };
        let without_usage = QueryResult {
            result: "b".into(),
            session_id: "sess-1".into(),
            cost_usd: Some(0.01),
            duration_ms: None,
            num_turns: Some(1),
            is_error: false,
            usage: None,
            extra: Default::default(),
        };
        // A usage object with every bucket absent counts as missing,
        // not as a zero-token turn.
        let empty_usage = QueryResult {
            usage: Some(TokenUsage::default()),
            ..without_usage.clone()
        };

        session.record(&with_usage);
        session.record(&without_usage);
        session.record(&empty_usage);

        assert_eq!(session.total_tokens(), 125);
        assert_eq!(session.turns_missing_usage(), 2);
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
            usage: None,
            extra: Default::default(),
        };
        session.record(&r);

        assert!((budget.total_usd() - 0.07).abs() < 1e-9);
        assert!((session.total_cost_usd() - 0.07).abs() < 1e-9);
    }

    fn cost_result(session_id: &str, total: Option<f64>) -> QueryResult {
        QueryResult {
            result: "ok".into(),
            session_id: session_id.into(),
            cost_usd: total,
            duration_ms: None,
            num_turns: Some(1),
            is_error: false,
            usage: None,
            extra: Default::default(),
        }
    }

    #[test]
    fn record_cost_totals_count_once() {
        let budget = BudgetTracker::builder().max_usd(0.007).build();
        let mut session = Session::new(test_claude()).with_budget(budget.clone());
        for total in [0.001, 0.003, 0.006] {
            session.record(&cost_result("sess-1", Some(total)));
        }
        assert!((session.total_cost_usd() - 0.006).abs() < 1e-9);
        assert!((budget.total_usd() - 0.006).abs() < 1e-9);
        assert!(budget.check().is_ok());
        assert_eq!(session.history()[1].cost_usd, Some(0.003));
    }

    #[test]
    fn record_cost_missing_total_preserves_baseline() {
        let mut session = Session::new(test_claude());
        for total in [Some(0.002), None, Some(0.006)] {
            session.record(&cost_result("sess-1", total));
        }
        assert!((session.total_cost_usd() - 0.006).abs() < 1e-9);
        assert_eq!(session.history()[1].cost_usd, None);
    }

    #[test]
    fn record_cost_falling_total_starts_new_baseline() {
        let mut session = Session::new(test_claude());
        for total in [0.004, 0.001, 0.003] {
            session.record(&cost_result("sess-1", Some(total)));
        }
        assert!((session.total_cost_usd() - 0.006).abs() < 1e-9);
    }

    #[test]
    fn record_cost_changed_session_resets_baseline_even_without_total() {
        let budget = BudgetTracker::builder().build();
        let mut session = Session::new(test_claude()).with_budget(budget.clone());
        for (id, total) in [
            ("sess-1", Some(0.004)),
            ("sess-2", Some(0.001)),
            ("sess-2", None),
            ("sess-2", Some(0.003)),
            ("sess-3", None),
            ("sess-1", Some(0.001)),
        ] {
            session.record(&cost_result(id, total));
        }
        assert!((session.total_cost_usd() - 0.008).abs() < 1e-9);
        assert!((budget.total_usd() - 0.008).abs() < 1e-9);
    }

    #[test]
    fn record_cost_resumed_session_includes_initial_historical_total() {
        let budget = BudgetTracker::builder().build();
        let mut session =
            Session::resume(test_claude(), "sess-existing").with_budget(budget.clone());
        session.record(&cost_result("sess-existing", Some(0.4)));
        session.record(&cost_result("sess-existing", Some(0.6)));
        assert!((session.total_cost_usd() - 0.6).abs() < 1e-9);
        assert!((budget.total_usd() - 0.6).abs() < 1e-9);
    }

    #[test]
    fn record_cost_unchanged_error_total_counts_zero() {
        let mut session = Session::new(test_claude());
        session.record(&cost_result("sess-1", Some(0.002)));
        let interrupted = QueryResult {
            result: String::new(),
            is_error: true,
            ..cost_result("sess-1", Some(0.002))
        };
        session.record(&interrupted);
        session.record(&cost_result("sess-1", Some(0.005)));
        assert!((session.total_cost_usd() - 0.005).abs() < 1e-9);
    }

    #[test]
    fn record_cost_missing_session_id_keeps_known_identity_and_baseline() {
        let mut session = Session::new(test_claude());
        session.record(&cost_result("sess-1", Some(0.001)));
        session.record(&cost_result("", Some(0.003)));
        assert_eq!(session.id(), Some("sess-1"));
        session.record(&cost_result("sess-1", Some(0.006)));
        assert!((session.total_cost_usd() - 0.006).abs() < 1e-9);

        let mut unknown = Session::new(test_claude());
        unknown.record(&cost_result("", Some(0.001)));
        assert_eq!(unknown.id(), None);
        unknown.record(&cost_result("discovered", Some(0.003)));
        assert!((unknown.total_cost_usd() - 0.003).abs() < 1e-9);
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
