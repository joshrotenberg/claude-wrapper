//! Turn registry: tracks in-flight and recently-completed agent
//! turns by stable [`TurnId`].
//!
//! Async-by-default agent tools (the bare-named `chat_send` /
//! `claude_query` introduced in steps 3 and 5) fire a turn into the
//! background and immediately return a [`TurnId`]. Callers poll
//! status with `turn_get`, block on completion with `turn_wait`, or
//! cancel with `turn_cancel`. Sync variants (`*_sync`) bypass this
//! registry entirely -- they hold the request connection open.
//!
//! Internally each entry holds a [`tokio::sync::watch::Sender`]
//! whose value is the latest [`TurnSnapshot`]. Workers update by
//! sending a new snapshot; waiters subscribe to the receiver and
//! `wait_for(terminal)`. Cancellation is a separate cooperative
//! flag (an `AtomicBool`) so the worker can check between awaits.
//!
//! TTL eviction is a separate concern (step 6); for now the
//! registry grows monotonically until `cancel_turn` / process exit.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::Serialize;
use serde_json::Value;
use tokio::sync::{RwLock, watch};

use crate::state::ChatId;

/// Opaque identifier for an async turn. Format: `turn_<hex>_<counter>`.
pub type TurnId = String;

/// Lifecycle states for a turn. Three terminal: [`Done`], [`Failed`],
/// [`Cancelled`].
///
/// [`Done`]: TurnStatus::Done
/// [`Failed`]: TurnStatus::Failed
/// [`Cancelled`]: TurnStatus::Cancelled
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TurnStatus {
    Running,
    Done,
    Failed,
    Cancelled,
}

impl TurnStatus {
    /// True when no further transitions are expected.
    pub fn is_terminal(self) -> bool {
        !matches!(self, TurnStatus::Running)
    }
}

/// Point-in-time view of a turn. Cloneable so multiple waiters can
/// hold copies.
#[derive(Debug, Clone, Serialize)]
pub struct TurnSnapshot {
    pub turn_id: TurnId,
    /// The chat that owns this turn, when the turn was fired from
    /// the chat surface. None for single-shot turns (`claude_query`).
    pub chat_id: Option<ChatId>,
    pub status: TurnStatus,
    /// Unix-epoch microseconds when the turn was registered.
    pub started_at_us: u128,
    /// Unix-epoch microseconds when the turn reached a terminal
    /// status. None while running.
    pub finished_at_us: Option<u128>,
    /// On `Done`, the JSON envelope produced by the worker (matches
    /// the shape `chat_send_sync` / `claude_query_sync` returns).
    pub result: Option<Value>,
    /// On `Failed`, the worker's `Display`'d error.
    pub error: Option<String>,
}

/// Handle returned by [`TurnRegistry::register`] to the worker that
/// will run the turn. Drop semantics intentionally permissive: if
/// the worker panics or exits without calling `complete` / `fail`,
/// the turn stays in `Running` until something else cleans it up.
/// (Step 6's TTL sweeper will handle that case.)
pub struct TurnHandle {
    pub turn_id: TurnId,
    pub cancel: Arc<AtomicBool>,
    snapshot_tx: watch::Sender<TurnSnapshot>,
}

impl TurnHandle {
    /// True if a cancel was requested via [`TurnRegistry::cancel`].
    /// Workers should check this between awaits and short-circuit
    /// out via [`TurnHandle::cancelled`] if set.
    pub fn is_cancelled(&self) -> bool {
        self.cancel.load(Ordering::SeqCst)
    }

    /// Publish a Done terminal state with the worker's JSON result.
    pub fn complete(self, result: Value) {
        let mut snap = self.snapshot_tx.borrow().clone();
        snap.status = TurnStatus::Done;
        snap.finished_at_us = Some(now_us());
        snap.result = Some(result);
        let _ = self.snapshot_tx.send(snap);
    }

    /// Publish a Failed terminal state.
    pub fn fail(self, error: impl std::fmt::Display) {
        let mut snap = self.snapshot_tx.borrow().clone();
        snap.status = TurnStatus::Failed;
        snap.finished_at_us = Some(now_us());
        snap.error = Some(error.to_string());
        let _ = self.snapshot_tx.send(snap);
    }

    /// Publish a Cancelled terminal state. Workers call this when
    /// they observe `is_cancelled()` and short-circuit out.
    pub fn cancelled(self) {
        let mut snap = self.snapshot_tx.borrow().clone();
        snap.status = TurnStatus::Cancelled;
        snap.finished_at_us = Some(now_us());
        let _ = self.snapshot_tx.send(snap);
    }
}

struct TurnEntry {
    snapshot_rx: watch::Receiver<TurnSnapshot>,
    cancel: Arc<AtomicBool>,
}

/// Concurrent map from [`TurnId`] to live + terminal turns.
///
/// Designed around the small handful of operations the tools need:
/// [`Self::register`] (fire), [`Self::get`] (poll),
/// [`Self::wait`] (block), [`Self::cancel`] (cooperative),
/// [`Self::list`] (enumerate).
#[derive(Default)]
pub struct TurnRegistry {
    entries: RwLock<HashMap<TurnId, TurnEntry>>,
}

impl TurnRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a fresh turn. Returns a worker-side [`TurnHandle`]
    /// the caller must move into the spawned task -- the handle's
    /// terminal methods (complete / fail / cancelled) publish the
    /// final snapshot.
    pub async fn register(&self, chat_id: Option<ChatId>) -> TurnHandle {
        let turn_id = new_turn_id();
        let cancel = Arc::new(AtomicBool::new(false));
        let snapshot = TurnSnapshot {
            turn_id: turn_id.clone(),
            chat_id,
            status: TurnStatus::Running,
            started_at_us: now_us(),
            finished_at_us: None,
            result: None,
            error: None,
        };
        let (tx, rx) = watch::channel(snapshot);
        self.entries.write().await.insert(
            turn_id.clone(),
            TurnEntry {
                snapshot_rx: rx,
                cancel: cancel.clone(),
            },
        );
        TurnHandle {
            turn_id,
            cancel,
            snapshot_tx: tx,
        }
    }

    /// Non-blocking snapshot read. Returns None for unknown ids.
    pub async fn get(&self, id: &str) -> Option<TurnSnapshot> {
        let entries = self.entries.read().await;
        entries.get(id).map(|e| e.snapshot_rx.borrow().clone())
    }

    /// Block until the turn reaches a terminal status, or until the
    /// optional timeout elapses. Returns:
    /// - `Ok(Some(snapshot))` -- turn settled, returns the terminal snapshot
    /// - `Ok(None)`           -- timeout elapsed; turn still running
    /// - `Err(...)`           -- unknown turn id
    pub async fn wait(
        &self,
        id: &str,
        timeout: Option<std::time::Duration>,
    ) -> Result<Option<TurnSnapshot>, UnknownTurn> {
        let mut rx = {
            let entries = self.entries.read().await;
            let entry = entries.get(id).ok_or_else(|| UnknownTurn(id.to_string()))?;
            entry.snapshot_rx.clone()
        };
        // Fast path: already terminal.
        if rx.borrow().status.is_terminal() {
            return Ok(Some(rx.borrow().clone()));
        }
        let fut = async {
            // wait_for returns once the predicate returns true OR the
            // sender is dropped. We treat sender-drop as "still running",
            // which becomes a stale entry that the TTL sweeper handles.
            let _ = rx.wait_for(|s| s.status.is_terminal()).await;
            rx.borrow().clone()
        };
        match timeout {
            Some(d) => match tokio::time::timeout(d, fut).await {
                Ok(snap) => Ok(Some(snap)),
                Err(_) => Ok(None),
            },
            None => Ok(Some(fut.await)),
        }
    }

    /// Signal cancellation. The worker is responsible for checking
    /// [`TurnHandle::is_cancelled`] and short-circuiting out; this
    /// method only flips the flag.
    pub async fn cancel(&self, id: &str) -> bool {
        let entries = self.entries.read().await;
        match entries.get(id) {
            Some(e) => {
                e.cancel.store(true, Ordering::SeqCst);
                true
            }
            None => false,
        }
    }

    /// List snapshots, optionally filtered to one chat.
    pub async fn list(&self, chat_id: Option<&str>) -> Vec<TurnSnapshot> {
        let entries = self.entries.read().await;
        entries
            .values()
            .map(|e| e.snapshot_rx.borrow().clone())
            .filter(|s| match chat_id {
                Some(want) => s.chat_id.as_deref() == Some(want),
                None => true,
            })
            .collect()
    }
}

/// Error type when a turn id is not present in the registry.
#[derive(Debug, Clone)]
pub struct UnknownTurn(pub String);

impl std::fmt::Display for UnknownTurn {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "no turn with id `{}`", self.0)
    }
}

impl std::error::Error for UnknownTurn {}

fn new_turn_id() -> TurnId {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let t = now_us();
    format!("turn_{t:x}_{n:x}")
}

fn now_us() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_micros())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[tokio::test]
    async fn register_assigns_running_turn_id() {
        let r = TurnRegistry::new();
        let h = r.register(None).await;
        assert!(h.turn_id.starts_with("turn_"));
        let snap = r.get(&h.turn_id).await.expect("found");
        assert_eq!(snap.status, TurnStatus::Running);
        assert!(snap.finished_at_us.is_none());
    }

    #[tokio::test]
    async fn complete_transitions_to_done_with_result() {
        let r = TurnRegistry::new();
        let h = r.register(Some("chat_x".into())).await;
        let id = h.turn_id.clone();
        h.complete(serde_json::json!({"result": "ok"}));
        let snap = r.get(&id).await.expect("found");
        assert_eq!(snap.status, TurnStatus::Done);
        assert_eq!(snap.chat_id.as_deref(), Some("chat_x"));
        assert_eq!(snap.result, Some(serde_json::json!({"result": "ok"})));
        assert!(snap.finished_at_us.is_some());
    }

    #[tokio::test]
    async fn wait_returns_immediately_when_already_terminal() {
        let r = TurnRegistry::new();
        let h = r.register(None).await;
        let id = h.turn_id.clone();
        h.complete(serde_json::json!(null));
        let snap = r
            .wait(&id, Some(Duration::from_millis(1)))
            .await
            .expect("ok")
            .expect("terminal");
        assert_eq!(snap.status, TurnStatus::Done);
    }

    #[tokio::test]
    async fn wait_blocks_until_complete() {
        let r = Arc::new(TurnRegistry::new());
        let h = r.register(None).await;
        let id = h.turn_id.clone();
        let r2 = r.clone();
        let waiter =
            tokio::spawn(async move { r2.wait(&id, None).await.expect("ok").expect("terminal") });
        tokio::time::sleep(Duration::from_millis(20)).await;
        h.complete(serde_json::json!({"x": 1}));
        let snap = waiter.await.expect("joined");
        assert_eq!(snap.status, TurnStatus::Done);
        assert_eq!(snap.result, Some(serde_json::json!({"x": 1})));
    }

    #[tokio::test]
    async fn wait_with_timeout_returns_none_on_timeout() {
        let r = TurnRegistry::new();
        let h = r.register(None).await;
        let res = r
            .wait(&h.turn_id, Some(Duration::from_millis(10)))
            .await
            .expect("ok");
        assert!(res.is_none(), "expected timeout to yield None");
    }

    #[tokio::test]
    async fn cancel_sets_flag_visible_to_handle() {
        let r = TurnRegistry::new();
        let h = r.register(None).await;
        let id = h.turn_id.clone();
        assert!(!h.is_cancelled());
        assert!(r.cancel(&id).await);
        assert!(h.is_cancelled());
    }

    #[tokio::test]
    async fn cancel_unknown_id_returns_false() {
        let r = TurnRegistry::new();
        assert!(!r.cancel("turn_nope").await);
    }

    #[tokio::test]
    async fn list_filters_by_chat_id() {
        let r = TurnRegistry::new();
        let _h1 = r.register(Some("chat_a".into())).await;
        let _h2 = r.register(Some("chat_b".into())).await;
        let _h3 = r.register(None).await;
        let all = r.list(None).await;
        assert_eq!(all.len(), 3);
        let a = r.list(Some("chat_a")).await;
        assert_eq!(a.len(), 1);
        assert_eq!(a[0].chat_id.as_deref(), Some("chat_a"));
    }

    #[test]
    fn ids_are_unique() {
        let a = new_turn_id();
        let b = new_turn_id();
        let c = new_turn_id();
        assert_ne!(a, b);
        assert_ne!(b, c);
        assert!(a.starts_with("turn_"));
    }
}
