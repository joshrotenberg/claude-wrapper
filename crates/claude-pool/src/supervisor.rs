//! Background supervisor loop for slot health monitoring.
//!
//! The supervisor periodically checks slot health and automatically restarts
//! errored slots that haven't exceeded their restart limit. Enable it via
//! [`crate::PoolConfig::supervisor_enabled`] and configure the interval with
//! [`crate::PoolConfig::supervisor_interval_secs`].

use tokio::sync::watch;
use tokio::task::JoinHandle;

use crate::pool::Pool;
use crate::store::PoolStore;
use crate::types::{SlotRecord, SlotState};

/// Handle to a running supervisor loop.
///
/// Returned by [`Pool::start_supervisor`]. Dropping the handle does **not**
/// stop the loop — call [`SupervisorHandle::stop`]
/// explicitly.
pub struct SupervisorHandle {
    stop_tx: watch::Sender<bool>,
    handle: JoinHandle<()>,
}

impl SupervisorHandle {
    /// Signal the supervisor to stop and wait for it to finish.
    pub async fn stop(self) {
        let _ = self.stop_tx.send(true);
        let _ = self.handle.await;
    }
}

/// Run one health-check pass over all slots.
///
/// Errored slots below the restart limit are reset to idle.
/// Returns the number of slots restarted.
pub(crate) async fn check_and_restart_slots<S: PoolStore + 'static>(pool: &Pool<S>) -> usize {
    let max_restarts = pool.config().max_restarts;
    let slots = match pool.store().list_slots().await {
        Ok(s) => s,
        Err(e) => {
            tracing::warn!(error = %e, "supervisor: failed to list slots");
            return 0;
        }
    };

    let mut restarted = 0;
    for slot in slots {
        if slot.state == SlotState::Errored && slot.restart_count < max_restarts {
            if let Err(e) = restart_slot(pool, &slot).await {
                tracing::warn!(
                    slot_id = %slot.id.0,
                    error = %e,
                    "supervisor: failed to restart slot"
                );
            } else {
                restarted += 1;
                tracing::info!(
                    slot_id = %slot.id.0,
                    restart_count = slot.restart_count + 1,
                    "supervisor: restarted errored slot"
                );
            }
        }
    }
    restarted
}

/// Reset an errored slot back to idle and increment its restart counter.
async fn restart_slot<S: PoolStore + 'static>(
    pool: &Pool<S>,
    slot: &SlotRecord,
) -> crate::Result<()> {
    let mut updated = slot.clone();
    updated.state = SlotState::Idle;
    updated.current_task = None;
    updated.session_id = None;
    updated.restart_count += 1;
    pool.store().put_slot(updated).await?;
    Ok(())
}

/// Spawn the background supervisor loop.
///
/// The loop runs every `interval_secs` seconds until signalled to stop.
pub(crate) fn spawn_supervisor<S: PoolStore + 'static>(
    pool: Pool<S>,
    interval_secs: u64,
) -> SupervisorHandle {
    let (stop_tx, mut stop_rx) = watch::channel(false);
    let interval = std::time::Duration::from_secs(interval_secs);

    let handle = tokio::spawn(async move {
        loop {
            tokio::select! {
                _ = tokio::time::sleep(interval) => {
                    check_and_restart_slots(&pool).await;
                }
                _ = stop_rx.changed() => {
                    break;
                }
            }
        }
    });

    SupervisorHandle { stop_tx, handle }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{PoolConfig, SlotId};
    use claude_wrapper::Claude;

    fn mock_claude() -> Claude {
        Claude::builder().binary("/usr/bin/false").build().unwrap()
    }

    #[tokio::test]
    async fn check_restarts_errored_slots() {
        let pool = Pool::builder(mock_claude()).slots(2).build().await.unwrap();

        // Mark slot-0 as errored.
        let mut slot = pool
            .store()
            .get_slot(&SlotId("slot-0".into()))
            .await
            .unwrap()
            .unwrap();
        slot.state = SlotState::Errored;
        pool.store().put_slot(slot).await.unwrap();

        let restarted = check_and_restart_slots(&pool).await;
        assert_eq!(restarted, 1);

        // Slot should now be idle with restart_count = 1.
        let slot = pool
            .store()
            .get_slot(&SlotId("slot-0".into()))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(slot.state, SlotState::Idle);
        assert_eq!(slot.restart_count, 1);
    }

    #[tokio::test]
    async fn check_skips_slots_at_restart_limit() {
        let config = PoolConfig {
            max_restarts: 2,
            ..Default::default()
        };
        let pool = Pool::builder(mock_claude())
            .slots(1)
            .config(config)
            .build()
            .await
            .unwrap();

        // Mark slot-0 as errored with restart_count at the limit.
        let mut slot = pool
            .store()
            .get_slot(&SlotId("slot-0".into()))
            .await
            .unwrap()
            .unwrap();
        slot.state = SlotState::Errored;
        slot.restart_count = 2;
        pool.store().put_slot(slot).await.unwrap();

        let restarted = check_and_restart_slots(&pool).await;
        assert_eq!(restarted, 0);

        // Slot should still be errored.
        let slot = pool
            .store()
            .get_slot(&SlotId("slot-0".into()))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(slot.state, SlotState::Errored);
    }

    #[tokio::test]
    async fn check_ignores_idle_and_busy_slots() {
        let pool = Pool::builder(mock_claude()).slots(2).build().await.unwrap();

        // Mark slot-1 as busy.
        let mut slot = pool
            .store()
            .get_slot(&SlotId("slot-1".into()))
            .await
            .unwrap()
            .unwrap();
        slot.state = SlotState::Busy;
        pool.store().put_slot(slot).await.unwrap();

        let restarted = check_and_restart_slots(&pool).await;
        assert_eq!(restarted, 0);
    }

    #[tokio::test]
    async fn start_supervisor_returns_none_when_disabled() {
        let pool = Pool::builder(mock_claude()).slots(1).build().await.unwrap();
        assert!(pool.start_supervisor().is_none());
    }

    #[tokio::test]
    async fn start_supervisor_returns_handle_when_enabled() {
        let config = PoolConfig {
            supervisor_enabled: true,
            supervisor_interval_secs: 1,
            ..Default::default()
        };
        let pool = Pool::builder(mock_claude())
            .slots(1)
            .config(config)
            .build()
            .await
            .unwrap();

        let handle = pool.start_supervisor().expect("should return handle");
        handle.stop().await;
    }
}
