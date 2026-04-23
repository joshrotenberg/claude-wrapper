//! Cumulative USD budget tracking for Claude sessions.
//!
//! A [`BudgetTracker`] accumulates cost across turns and fires
//! caller-supplied callbacks when configurable thresholds are crossed.
//! When a `max_usd` ceiling is set, [`BudgetTracker::check`] returns
//! [`Error::BudgetExceeded`](crate::error::Error::BudgetExceeded) once
//! the total is at or above the ceiling, giving callers a hard stop
//! before the next CLI invocation.
//!
//! # Ownership and sharing
//!
//! `BudgetTracker` wraps its state in `Arc<Mutex<...>>` so clones share
//! the same running total and fire-once flags. Attach one tracker to a
//! [`Session`](crate::session::Session), or clone it across several
//! sessions to enforce a fleet-wide ceiling.
//!
//! # Example
//!
//! ```no_run
//! use std::sync::Arc;
//! use claude_wrapper::{Claude, BudgetTracker};
//! use claude_wrapper::session::Session;
//!
//! # async fn example() -> claude_wrapper::Result<()> {
//! let budget = BudgetTracker::builder()
//!     .max_usd(5.00)
//!     .warn_at_usd(4.00)
//!     .on_warning(|total| eprintln!("warning: ${total:.2} spent"))
//!     .on_exceeded(|total| eprintln!("budget hit: ${total:.2}"))
//!     .build();
//!
//! let claude = Arc::new(Claude::builder().build()?);
//! let mut session = Session::new(claude).with_budget(budget.clone());
//!
//! session.send("hello").await?;
//! println!("spent so far: ${:.4}", budget.total_usd());
//! # Ok(())
//! # }
//! ```

use std::sync::{Arc, Mutex};

use crate::error::{Error, Result};

type Callback = Arc<dyn Fn(f64) + Send + Sync>;

struct Config {
    max_usd: Option<f64>,
    warn_at_usd: Option<f64>,
    on_warning: Option<Callback>,
    on_exceeded: Option<Callback>,
}

#[derive(Default)]
struct State {
    total_usd: f64,
    warned: bool,
    exceeded: bool,
}

struct Inner {
    config: Config,
    state: Mutex<State>,
}

/// Cumulative USD budget tracker with threshold callbacks.
///
/// See the [module docs](crate::budget) for the full design.
#[derive(Clone)]
pub struct BudgetTracker {
    inner: Arc<Inner>,
}

impl BudgetTracker {
    /// Start building a tracker.
    pub fn builder() -> BudgetBuilder {
        BudgetBuilder::default()
    }

    /// Record an additional cost in USD. Fires `on_warning` the first
    /// time the running total reaches `warn_at_usd`, and `on_exceeded`
    /// the first time it reaches `max_usd`.
    pub fn record(&self, cost_usd: f64) {
        if cost_usd <= 0.0 || !cost_usd.is_finite() {
            return;
        }

        let (warn_fired, exceeded_fired, total) = {
            let mut state = self.inner.state.lock().expect("budget mutex poisoned");
            state.total_usd += cost_usd;

            let warn_fired = match self.inner.config.warn_at_usd {
                Some(threshold) if !state.warned && state.total_usd >= threshold => {
                    state.warned = true;
                    true
                }
                _ => false,
            };

            let exceeded_fired = match self.inner.config.max_usd {
                Some(threshold) if !state.exceeded && state.total_usd >= threshold => {
                    state.exceeded = true;
                    true
                }
                _ => false,
            };

            (warn_fired, exceeded_fired, state.total_usd)
        };

        if warn_fired && let Some(cb) = &self.inner.config.on_warning {
            cb(total);
        }
        if exceeded_fired && let Some(cb) = &self.inner.config.on_exceeded {
            cb(total);
        }
    }

    /// Return `Err(Error::BudgetExceeded)` if the running total is at
    /// or above the configured `max_usd`. Returns `Ok(())` when no
    /// ceiling is set.
    pub fn check(&self) -> Result<()> {
        let Some(max) = self.inner.config.max_usd else {
            return Ok(());
        };
        let total = self
            .inner
            .state
            .lock()
            .expect("budget mutex poisoned")
            .total_usd;
        if total >= max {
            Err(Error::BudgetExceeded {
                total_usd: total,
                max_usd: max,
            })
        } else {
            Ok(())
        }
    }

    /// Cumulative cost recorded so far, in USD.
    pub fn total_usd(&self) -> f64 {
        self.inner
            .state
            .lock()
            .expect("budget mutex poisoned")
            .total_usd
    }

    /// Remaining budget in USD, if a ceiling is set. Clamped at zero
    /// once the ceiling is reached.
    pub fn remaining_usd(&self) -> Option<f64> {
        let max = self.inner.config.max_usd?;
        let total = self
            .inner
            .state
            .lock()
            .expect("budget mutex poisoned")
            .total_usd;
        Some((max - total).max(0.0))
    }

    /// Configured ceiling, if any.
    pub fn max_usd(&self) -> Option<f64> {
        self.inner.config.max_usd
    }

    /// Configured warning threshold, if any.
    pub fn warn_at_usd(&self) -> Option<f64> {
        self.inner.config.warn_at_usd
    }

    /// Clear the running total and re-arm both callbacks.
    pub fn reset(&self) {
        let mut state = self.inner.state.lock().expect("budget mutex poisoned");
        state.total_usd = 0.0;
        state.warned = false;
        state.exceeded = false;
    }
}

impl std::fmt::Debug for BudgetTracker {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let state = self.inner.state.lock().expect("budget mutex poisoned");
        f.debug_struct("BudgetTracker")
            .field("max_usd", &self.inner.config.max_usd)
            .field("warn_at_usd", &self.inner.config.warn_at_usd)
            .field("total_usd", &state.total_usd)
            .field("warned", &state.warned)
            .field("exceeded", &state.exceeded)
            .finish()
    }
}

/// Builder for [`BudgetTracker`].
#[derive(Default)]
pub struct BudgetBuilder {
    max_usd: Option<f64>,
    warn_at_usd: Option<f64>,
    on_warning: Option<Callback>,
    on_exceeded: Option<Callback>,
}

impl BudgetBuilder {
    /// Hard ceiling in USD. Once the running total hits this value,
    /// [`BudgetTracker::check`] returns [`Error::BudgetExceeded`].
    pub fn max_usd(mut self, max: f64) -> Self {
        self.max_usd = Some(max);
        self
    }

    /// Warning threshold in USD. [`BudgetBuilder::on_warning`] fires
    /// the first time the running total reaches this value.
    pub fn warn_at_usd(mut self, warn: f64) -> Self {
        self.warn_at_usd = Some(warn);
        self
    }

    /// Callback fired once when `warn_at_usd` is crossed. The argument
    /// is the running total at the crossing.
    pub fn on_warning<F>(mut self, f: F) -> Self
    where
        F: Fn(f64) + Send + Sync + 'static,
    {
        self.on_warning = Some(Arc::new(f));
        self
    }

    /// Callback fired once when `max_usd` is crossed. The argument is
    /// the running total at the crossing.
    pub fn on_exceeded<F>(mut self, f: F) -> Self
    where
        F: Fn(f64) + Send + Sync + 'static,
    {
        self.on_exceeded = Some(Arc::new(f));
        self
    }

    /// Finish construction.
    pub fn build(self) -> BudgetTracker {
        BudgetTracker {
            inner: Arc::new(Inner {
                config: Config {
                    max_usd: self.max_usd,
                    warn_at_usd: self.warn_at_usd,
                    on_warning: self.on_warning,
                    on_exceeded: self.on_exceeded,
                },
                state: Mutex::new(State::default()),
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[test]
    fn record_accumulates() {
        let b = BudgetTracker::builder().build();
        b.record(0.01);
        b.record(0.02);
        b.record(0.03);
        assert!((b.total_usd() - 0.06).abs() < 1e-9);
    }

    #[test]
    fn record_ignores_non_positive_and_non_finite() {
        let b = BudgetTracker::builder().build();
        b.record(0.0);
        b.record(-0.5);
        b.record(f64::NAN);
        b.record(f64::INFINITY);
        assert_eq!(b.total_usd(), 0.0);
    }

    #[test]
    fn warn_callback_fires_once_at_threshold() {
        let count = Arc::new(AtomicUsize::new(0));
        let c = Arc::clone(&count);
        let b = BudgetTracker::builder()
            .warn_at_usd(0.10)
            .on_warning(move |_| {
                c.fetch_add(1, Ordering::SeqCst);
            })
            .build();

        b.record(0.05);
        assert_eq!(count.load(Ordering::SeqCst), 0);
        b.record(0.06); // total 0.11 -> crosses
        assert_eq!(count.load(Ordering::SeqCst), 1);
        b.record(0.20); // further spend, no re-fire
        assert_eq!(count.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn exceeded_callback_fires_once_at_threshold() {
        let count = Arc::new(AtomicUsize::new(0));
        let c = Arc::clone(&count);
        let b = BudgetTracker::builder()
            .max_usd(1.00)
            .on_exceeded(move |_| {
                c.fetch_add(1, Ordering::SeqCst);
            })
            .build();

        b.record(0.50);
        b.record(0.49);
        assert_eq!(count.load(Ordering::SeqCst), 0);
        b.record(0.02); // total 1.01 -> crosses
        assert_eq!(count.load(Ordering::SeqCst), 1);
        b.record(0.50);
        assert_eq!(count.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn check_errors_once_over_max() {
        let b = BudgetTracker::builder().max_usd(0.10).build();
        b.record(0.05);
        assert!(b.check().is_ok());
        b.record(0.05); // total 0.10 -> at threshold
        match b.check() {
            Err(Error::BudgetExceeded { total_usd, max_usd }) => {
                assert!((total_usd - 0.10).abs() < 1e-9);
                assert!((max_usd - 0.10).abs() < 1e-9);
            }
            other => panic!("expected BudgetExceeded, got {other:?}"),
        }
    }

    #[test]
    fn check_noop_without_max() {
        let b = BudgetTracker::builder().build();
        b.record(1_000.0);
        assert!(b.check().is_ok());
    }

    #[test]
    fn remaining_usd_clamps_at_zero() {
        let b = BudgetTracker::builder().max_usd(1.00).build();
        assert_eq!(b.remaining_usd(), Some(1.00));
        b.record(0.40);
        assert!((b.remaining_usd().unwrap() - 0.60).abs() < 1e-9);
        b.record(10.00);
        assert_eq!(b.remaining_usd(), Some(0.0));
    }

    #[test]
    fn remaining_usd_none_without_max() {
        let b = BudgetTracker::builder().build();
        assert!(b.remaining_usd().is_none());
    }

    #[test]
    fn reset_clears_total_and_rearms_callbacks() {
        let warn = Arc::new(AtomicUsize::new(0));
        let exc = Arc::new(AtomicUsize::new(0));
        let w = Arc::clone(&warn);
        let e = Arc::clone(&exc);
        let b = BudgetTracker::builder()
            .warn_at_usd(0.10)
            .max_usd(0.20)
            .on_warning(move |_| {
                w.fetch_add(1, Ordering::SeqCst);
            })
            .on_exceeded(move |_| {
                e.fetch_add(1, Ordering::SeqCst);
            })
            .build();

        b.record(0.25);
        assert_eq!(warn.load(Ordering::SeqCst), 1);
        assert_eq!(exc.load(Ordering::SeqCst), 1);
        assert!(b.check().is_err());

        b.reset();
        assert_eq!(b.total_usd(), 0.0);
        assert!(b.check().is_ok());

        b.record(0.25);
        assert_eq!(warn.load(Ordering::SeqCst), 2);
        assert_eq!(exc.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn clones_share_state() {
        let a = BudgetTracker::builder().max_usd(1.00).build();
        let b = a.clone();
        a.record(0.60);
        b.record(0.50);
        assert!((a.total_usd() - 1.10).abs() < 1e-9);
        assert!((b.total_usd() - 1.10).abs() < 1e-9);
        assert!(a.check().is_err());
        assert!(b.check().is_err());
    }

    #[test]
    fn concurrent_record_preserves_total() {
        use std::thread;

        let b = BudgetTracker::builder().build();
        let mut handles = Vec::new();
        for _ in 0..8 {
            let b = b.clone();
            handles.push(thread::spawn(move || {
                for _ in 0..1000 {
                    b.record(0.001);
                }
            }));
        }
        for h in handles {
            h.join().unwrap();
        }
        assert!((b.total_usd() - 8.0).abs() < 1e-6);
    }
}
