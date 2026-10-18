//! Host-side bookkeeping wrapper around [`DuplexSession`].
//!
//! [`Conversation`] keeps a rolling history of [`TurnResult`]s,
//! cumulative cost, and an optional [`BudgetTracker`] hard stop on top
//! of an underlying [`DuplexSession`]. The duplex session itself
//! remains the transport; this wrapper only adds accounting that
//! `DuplexSession::send` does not provide on its own.
//!
//! For the equivalent shape on top of transient `--resume` subprocess
//! turns, see [`Session`](crate::session::Session). [`Conversation`]
//! is the duplex-flavoured peer.
//!
//! # When to use
//!
//! Reach for [`Conversation`] when you already want a
//! [`DuplexSession`] (long-running host, mid-turn interrupts, broadcast
//! subscribers) AND want to answer questions like:
//!
//! - How much have I spent on this conversation so far?
//! - What's the full history of turns and their costs?
//! - Stop accepting new turns once I hit $5.
//!
//! If you do not need bookkeeping, use [`DuplexSession`] directly. If
//! you want accounting on short-lived per-turn subprocess calls, use
//! [`Session`](crate::session::Session) instead.
//!
//! # Example
//!
//! ```no_run
//! use claude_wrapper::Claude;
//! use claude_wrapper::conversation::Conversation;
//! use claude_wrapper::duplex::{DuplexOptions, DuplexSession};
//!
//! # async fn example() -> claude_wrapper::Result<()> {
//! let claude = Claude::builder().build()?;
//! let session = DuplexSession::spawn(
//!     &claude,
//!     DuplexOptions::default().model("haiku"),
//! ).await?;
//!
//! let mut conv = Conversation::new(session);
//! let _first = conv.send("hello").await?;
//! let _second = conv.send("and again").await?;
//!
//! println!("turns: {}", conv.total_turns());
//! println!("cost:  ${:.4}", conv.total_cost_usd());
//!
//! conv.close().await?;
//! # Ok(())
//! # }
//! ```
//!
//! # Cost accounting
//!
//! The CLI reports `total_cost_usd` on each turn's `result` message
//! as the running total for the session, not the cost of that turn
//! (see [`TurnResult::total_cost_usd`]). [`Conversation`] records the
//! change from the previous turn's total as that turn's cost, clamped
//! at zero, and adds it to [`Conversation::total_cost_usd`] and any
//! attached [`BudgetTracker`]. A turn that reports no total, or the
//! same total as the turn before, costs zero. Spend from a turn sent
//! straight through [`Conversation::session`] lands in the next
//! recorded turn's cost.
//!
//! If the [`DuplexSession`] was spawned with
//! [`DuplexOptions::resume`](crate::duplex::DuplexOptions::resume) or
//! [`DuplexOptions::continue_session`](crate::duplex::DuplexOptions::continue_session),
//! the CLI restores the resumed session's total, so the first
//! recorded turn also counts what that session spent before this
//! process started.
//! A falling total contributes zero and becomes the new baseline;
//! spend during an unobserved reset cannot be recovered from that
//! total alone.
//!
//! # Budget tracking
//!
//! Attach a [`BudgetTracker`] (the same type [`Session`](crate::session::Session)
//! uses) to enforce a cumulative USD ceiling. The pre-turn check runs
//! before delegating to [`DuplexSession::send`]; once the ceiling is
//! hit, [`Conversation::send`] returns
//! [`Error::BudgetExceeded`](crate::error::Error::BudgetExceeded)
//! without touching the underlying session.
//!
//! ```no_run
//! use claude_wrapper::{BudgetTracker, Claude};
//! use claude_wrapper::conversation::Conversation;
//! use claude_wrapper::duplex::{DuplexOptions, DuplexSession};
//!
//! # async fn example() -> claude_wrapper::Result<()> {
//! let budget = BudgetTracker::builder().max_usd(5.00).build();
//! let claude = Claude::builder().build()?;
//! let session = DuplexSession::spawn(&claude, DuplexOptions::default()).await?;
//!
//! let mut conv = Conversation::new(session).with_budget(budget.clone());
//! let _ = conv.send("hello").await?;
//! println!("spent: ${:.4}", budget.total_usd());
//! # Ok(())
//! # }
//! ```
//!
//! # Beyond bookkeeping
//!
//! [`Conversation::send`] is the only entry point that updates
//! history. For [`DuplexSession::subscribe`],
//! [`DuplexSession::interrupt`], and
//! [`DuplexSession::respond_to_permission`], use [`Conversation::session`]
//! to reach the inner handle. Those calls bypass the wrapper's
//! accounting on purpose: an interrupt still produces a `TurnResult`
//! that the in-flight [`Conversation::send`] records cleanly when the
//! truncated turn lands.

use crate::budget::BudgetTracker;
use crate::duplex::{DuplexSession, TurnResult, turn_cost_delta};
use crate::error::Result;

/// Host-side bookkeeping over a [`DuplexSession`].
///
/// See the [module docs](crate::conversation) for the full design.
#[derive(Debug)]
pub struct Conversation {
    inner: DuplexSession,
    state: ConversationState,
}

/// Pure accounting state extracted so unit tests can exercise the
/// bookkeeping logic without spawning a duplex child.
#[derive(Debug, Default)]
struct ConversationState {
    history: Vec<TurnResult>,
    cumulative_cost_usd: f64,
    cumulative_turns: u32,
    /// The last running total the CLI reported: the baseline for the
    /// next turn's cost.
    last_reported_total_usd: Option<f64>,
    budget: Option<BudgetTracker>,
}

impl ConversationState {
    fn record(&mut self, turn: TurnResult) {
        let cost = turn_cost_delta(&mut self.last_reported_total_usd, turn.total_cost_usd())
            .unwrap_or(0.0);
        self.cumulative_cost_usd += cost;
        self.cumulative_turns = self.cumulative_turns.saturating_add(1);
        if let Some(b) = &self.budget {
            b.record(cost);
        }
        self.history.push(turn);
    }
}

impl Conversation {
    /// Wrap a [`DuplexSession`] in a fresh [`Conversation`].
    ///
    /// The conversation starts with an empty history and zeroed
    /// counters; the underlying session is not touched until the
    /// first [`Conversation::send`].
    #[must_use]
    pub fn new(session: DuplexSession) -> Self {
        Self {
            inner: session,
            state: ConversationState::default(),
        }
    }

    /// Attach a [`BudgetTracker`] for cumulative-cost ceilings.
    ///
    /// Every turn's cost (the change in [`TurnResult::total_cost_usd`]
    /// from the previous turn, see the
    /// [module docs](crate::conversation#cost-accounting)) is
    /// recorded on the tracker, and [`Conversation::send`] returns
    /// [`Error::BudgetExceeded`](crate::error::Error::BudgetExceeded)
    /// before dispatching a turn if the ceiling has been hit. Clone a
    /// tracker across several conversations to enforce a shared
    /// ceiling.
    /// A resumed session's first recorded total includes historical
    /// spend, which is also charged to this tracker. See the
    /// [cost-accounting scope](crate::conversation#cost-accounting).
    #[must_use]
    pub fn with_budget(mut self, budget: BudgetTracker) -> Self {
        self.state.budget = Some(budget);
        self
    }

    /// The attached [`BudgetTracker`], if any.
    #[must_use]
    pub fn budget(&self) -> Option<&BudgetTracker> {
        self.state.budget.as_ref()
    }

    /// Send one user message and record the resulting [`TurnResult`].
    ///
    /// Pre-turn: if a [`BudgetTracker`] is attached and its ceiling is
    /// hit, returns
    /// [`Error::BudgetExceeded`](crate::error::Error::BudgetExceeded)
    /// without touching the underlying session.
    ///
    /// On success the returned reference points at the just-recorded
    /// last entry of [`Conversation::history`]. Errors from the
    /// underlying [`DuplexSession::send`]
    /// (e.g. [`Error::DuplexTurnInFlight`](crate::error::Error::DuplexTurnInFlight),
    /// [`Error::DuplexClosed`](crate::error::Error::DuplexClosed))
    /// propagate unchanged and do not update the history or cost
    /// counters.
    pub async fn send(&mut self, prompt: impl Into<String>) -> Result<&TurnResult> {
        if let Some(b) = &self.state.budget {
            b.check()?;
        }

        let turn = self.inner.send(prompt).await?;
        self.state.record(turn);
        Ok(self
            .state
            .history
            .last()
            .expect("just-pushed entry must be present"))
    }

    /// Per-turn result history, in arrival order.
    #[must_use]
    pub fn history(&self) -> &[TurnResult] {
        &self.state.history
    }

    /// Result of the most recent turn, if any.
    #[must_use]
    pub fn last(&self) -> Option<&TurnResult> {
        self.state.history.last()
    }

    /// Cumulative cost in USD across every recorded turn: the sum of
    /// each turn's change in the CLI's running total. See the
    /// [module docs](crate::conversation#cost-accounting).
    #[must_use]
    pub fn total_cost_usd(&self) -> f64 {
        self.state.cumulative_cost_usd
    }

    /// Number of turns recorded through [`Conversation::send`].
    #[must_use]
    pub fn total_turns(&self) -> u32 {
        self.state.cumulative_turns
    }

    /// Session id from the most recent turn's `result` payload, if
    /// any. Returns `None` until the first turn lands; later turns on
    /// a single duplex child reuse the same id.
    #[must_use]
    pub fn session_id(&self) -> Option<&str> {
        self.state.history.last().and_then(TurnResult::session_id)
    }

    /// Borrow the underlying [`DuplexSession`].
    ///
    /// Use this for [`DuplexSession::subscribe`],
    /// [`DuplexSession::interrupt`], and
    /// [`DuplexSession::respond_to_permission`]. Those calls bypass
    /// [`Conversation`]'s bookkeeping on purpose -- an interrupt still
    /// produces a [`TurnResult`] that the in-flight
    /// [`Conversation::send`] records cleanly when the truncated turn
    /// lands.
    #[must_use]
    pub fn session(&self) -> &DuplexSession {
        &self.inner
    }

    /// Close the underlying [`DuplexSession`] and wait for its task to
    /// exit. Consumes the [`Conversation`].
    pub async fn close(self) -> Result<()> {
        self.inner.close().await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// A completed turn whose `result` reports `total` as the
    /// session's running total.
    fn turn(session_id: &str, total: f64) -> TurnResult {
        TurnResult {
            result: json!({
                "type": "result",
                "subtype": "success",
                "is_error": false,
                "result": "ok",
                "session_id": session_id,
                "total_cost_usd": total,
            }),
            events: vec![],
        }
    }

    /// An interrupted turn, shaped like the CLI's: an error subtype,
    /// `is_error: true`, no `result` text.
    fn interrupted(session_id: &str, total: f64) -> TurnResult {
        TurnResult {
            result: json!({
                "type": "result",
                "subtype": "error_during_execution",
                "is_error": true,
                "session_id": session_id,
                "total_cost_usd": total,
            }),
            events: vec![],
        }
    }

    #[test]
    fn record_pushes_turn_and_updates_counters() {
        let mut state = ConversationState::default();
        state.record(turn("sess-1", 0.05));

        assert_eq!(state.history.len(), 1);
        assert_eq!(state.cumulative_turns, 1);
        assert!((state.cumulative_cost_usd - 0.05).abs() < 1e-9);
        assert_eq!(state.history[0].session_id(), Some("sess-1"));
    }

    #[test]
    fn record_takes_deltas_of_running_total() {
        // The CLI reports a running total per result, so three turns
        // costing 0.001, 0.002 and 0.003 report 0.001, 0.003, 0.006.
        let budget = BudgetTracker::builder().build();
        let mut state = ConversationState {
            budget: Some(budget.clone()),
            ..Default::default()
        };
        state.record(turn("sess-1", 0.001));
        state.record(turn("sess-1", 0.003));
        state.record(turn("sess-1", 0.006));

        assert_eq!(state.history.len(), 3);
        assert_eq!(state.cumulative_turns, 3);
        assert!((state.cumulative_cost_usd - 0.006).abs() < 1e-9);
        assert!((budget.total_usd() - 0.006).abs() < 1e-9);
    }

    #[test]
    fn record_interrupted_turn_with_unchanged_total_costs_zero() {
        let mut state = ConversationState::default();
        state.record(turn("sess-1", 0.002));
        state.record(interrupted("sess-1", 0.002));
        assert_eq!(state.cumulative_turns, 2);
        assert!((state.cumulative_cost_usd - 0.002).abs() < 1e-9);

        state.record(turn("sess-1", 0.005));
        assert!((state.cumulative_cost_usd - 0.005).abs() < 1e-9);
    }

    #[test]
    fn record_turn_without_total_keeps_baseline() {
        let mut state = ConversationState::default();
        state.record(turn("sess-1", 0.002));
        state.record(TurnResult {
            result: json!({ "type": "result", "session_id": "sess-1" }),
            events: vec![],
        });
        state.record(turn("sess-1", 0.004));

        assert_eq!(state.cumulative_turns, 3);
        assert!((state.cumulative_cost_usd - 0.004).abs() < 1e-9);
    }

    #[test]
    fn record_clamps_falling_total_at_zero() {
        // A total below the previous one costs zero and becomes the
        // new baseline.
        let mut state = ConversationState::default();
        state.record(turn("sess-1", 0.004));
        state.record(turn("sess-1", 0.001));
        assert!((state.cumulative_cost_usd - 0.004).abs() < 1e-9);

        state.record(turn("sess-1", 0.003));
        assert!((state.cumulative_cost_usd - 0.006).abs() < 1e-9);
    }

    #[test]
    fn record_treats_missing_cost_as_zero() {
        let mut state = ConversationState::default();
        let bare = TurnResult {
            result: json!({ "type": "result", "session_id": "sess-1" }),
            events: vec![],
        };
        state.record(bare);

        assert_eq!(state.cumulative_turns, 1);
        assert_eq!(state.cumulative_cost_usd, 0.0);
    }

    #[test]
    fn record_forwards_cost_to_budget() {
        let budget = BudgetTracker::builder().build();
        let mut state = ConversationState {
            budget: Some(budget.clone()),
            ..Default::default()
        };

        state.record(turn("sess-1", 0.07));

        assert!((budget.total_usd() - 0.07).abs() < 1e-9);
        assert!((state.cumulative_cost_usd - 0.07).abs() < 1e-9);
    }

    #[test]
    fn budget_check_blocks_before_send() {
        use crate::error::Error;

        // Conversation::send delegates pre-turn budget enforcement to
        // BudgetTracker::check. Exercise that directly with a
        // pre-loaded tracker so we don't need a live duplex session.
        let budget = BudgetTracker::builder().max_usd(0.10).build();
        budget.record(0.15);

        match budget.check() {
            Err(Error::BudgetExceeded { total_usd, max_usd }) => {
                assert!((total_usd - 0.15).abs() < 1e-9);
                assert!((max_usd - 0.10).abs() < 1e-9);
            }
            other => panic!("expected BudgetExceeded, got {other:?}"),
        }
    }

    #[test]
    fn last_returns_most_recent() {
        let mut state = ConversationState::default();
        assert!(state.history.last().is_none());

        state.record(turn("sess-1", 0.01));
        state.record(turn("sess-1", 0.02));
        let last = state.history.last().expect("last entry present");
        assert!((last.total_cost_usd().unwrap() - 0.02).abs() < 1e-9);
    }

    #[test]
    fn session_id_pulled_from_last_turn() {
        let mut state = ConversationState::default();
        // Empty history: no session id yet.
        assert!(
            state
                .history
                .last()
                .and_then(TurnResult::session_id)
                .is_none()
        );

        state.record(turn("sess-A", 0.01));
        state.record(turn("sess-B", 0.02));
        assert_eq!(
            state.history.last().and_then(TurnResult::session_id),
            Some("sess-B")
        );
    }
}
