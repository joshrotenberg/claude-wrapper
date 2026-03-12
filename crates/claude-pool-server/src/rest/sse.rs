//! SSE streaming helpers for the REST API.
//!
//! Provides polling-based SSE streams that emit events as task/chain state changes.
//! Polls the pool store at a configurable interval and emits state transitions.

use std::convert::Infallible;
use std::sync::Arc;
use std::time::Duration;

use axum::response::sse::{Event, KeepAlive};
use claude_pool::{PoolStore, TaskId, TaskState};
use tokio_stream::Stream;

use crate::State;

/// Default polling interval for SSE streams.
const POLL_INTERVAL: Duration = Duration::from_millis(500);

/// Create an SSE stream that emits task state changes and a final completion event.
///
/// Individual tasks don't currently store partial output, so this stream
/// emits `state` events on transitions and a final `complete`/`error` event.
/// Chain streams (below) have richer partial output support.
pub fn task_stream<S: PoolStore + 'static>(
    state: Arc<State<S>>,
    task_id: TaskId,
) -> impl Stream<Item = Result<Event, Infallible>> {
    let mut last_state = String::new();

    async_stream::stream! {
        loop {
            let task = state.pool.store().get_task(&task_id).await;

            let record = match task {
                Ok(Some(record)) => record,
                Ok(None) => {
                    yield Ok(Event::default()
                        .event("error")
                        .data(serde_json::json!({"error": format!("task {} not found", task_id.0)}).to_string()));
                    break;
                }
                Err(e) => {
                    yield Ok(Event::default()
                        .event("error")
                        .data(serde_json::json!({"error": e.to_string()}).to_string()));
                    break;
                }
            };

            let state_str = format!("{:?}", record.state).to_lowercase();

            // Emit state change events.
            if state_str != last_state {
                last_state = state_str.clone();
                yield Ok(Event::default()
                    .event("state")
                    .data(serde_json::json!({
                        "task_id": record.id.0,
                        "state": state_str,
                    }).to_string()));
            }

            // Check terminal states.
            match record.state {
                TaskState::Completed | TaskState::PendingReview => {
                    let result_data = match record.result {
                        Some(ref r) => serde_json::json!({
                            "task_id": record.id.0,
                            "state": format!("{:?}", record.state).to_lowercase(),
                            "output": r.output,
                            "success": r.success,
                            "cost_microdollars": r.cost_microdollars,
                            "turns_used": r.turns_used,
                        }),
                        None => serde_json::json!({
                            "task_id": record.id.0,
                            "state": format!("{:?}", record.state).to_lowercase(),
                        }),
                    };
                    yield Ok(Event::default()
                        .event("complete")
                        .data(result_data.to_string()));
                    break;
                }
                TaskState::Failed => {
                    let result_data = match record.result {
                        Some(ref r) => serde_json::json!({
                            "task_id": record.id.0,
                            "state": "failed",
                            "output": r.output,
                            "success": false,
                            "cost_microdollars": r.cost_microdollars,
                        }),
                        None => serde_json::json!({
                            "task_id": record.id.0,
                            "state": "failed",
                        }),
                    };
                    yield Ok(Event::default()
                        .event("error")
                        .data(result_data.to_string()));
                    break;
                }
                TaskState::Cancelled => {
                    yield Ok(Event::default()
                        .event("cancelled")
                        .data(serde_json::json!({"task_id": record.id.0}).to_string()));
                    break;
                }
                _ => {}
            }

            tokio::time::sleep(POLL_INTERVAL).await;
        }
    }
}

/// Create an SSE stream for chain progress with step-level events.
///
/// Emits `step_start`, `step_output`, `step_complete`, and `chain_complete` events.
/// Chains have full partial output support via `chain_progress()`.
pub fn chain_stream<S: PoolStore + 'static>(
    state: Arc<State<S>>,
    task_id: TaskId,
) -> impl Stream<Item = Result<Event, Infallible>> {
    let mut last_completed_count = 0usize;
    let mut last_partial_len = 0usize;
    let mut last_step: Option<usize> = None;

    async_stream::stream! {
        loop {
            let progress = state.pool.chain_progress(&task_id);

            let progress = match progress {
                Some(p) => p,
                None => {
                    yield Ok(Event::default()
                        .event("error")
                        .data(serde_json::json!({"error": format!("chain {} not found", task_id.0)}).to_string()));
                    break;
                }
            };

            // Emit events for newly completed steps.
            for step in &progress.completed_steps[last_completed_count..] {
                yield Ok(Event::default()
                    .event("step_complete")
                    .data(serde_json::json!({
                        "step": last_completed_count,
                        "name": step.name,
                        "success": step.success,
                        "cost_microdollars": step.cost_microdollars,
                    }).to_string()));
                last_completed_count += 1;
                last_partial_len = 0;
            }

            // Emit step_start when a new step begins.
            if let Some(current) = progress.current_step {
                if last_step != Some(current) {
                    last_step = Some(current);
                    last_partial_len = 0;
                    yield Ok(Event::default()
                        .event("step_start")
                        .data(serde_json::json!({
                            "step": current,
                            "name": progress.current_step_name,
                        }).to_string()));
                }

                // Emit partial output for current step.
                if let Some(ref partial) = progress.current_step_partial_output
                    && partial.len() > last_partial_len
                {
                    let new_chunk = &partial[last_partial_len..];
                    yield Ok(Event::default()
                        .event("step_output")
                        .data(serde_json::json!({
                            "step": current,
                            "chunk": new_chunk,
                        }).to_string()));
                    last_partial_len = partial.len();
                }
            }

            // Check terminal states.
            use claude_pool::ChainStatus;
            match progress.status {
                ChainStatus::Completed => {
                    let total_cost: u64 = progress
                        .completed_steps
                        .iter()
                        .map(|s| s.cost_microdollars)
                        .sum();
                    yield Ok(Event::default()
                        .event("chain_complete")
                        .data(serde_json::json!({
                            "chain_id": task_id.0,
                            "total_steps": progress.total_steps,
                            "total_cost_microdollars": total_cost,
                            "success": true,
                        }).to_string()));
                    break;
                }
                ChainStatus::Failed => {
                    yield Ok(Event::default()
                        .event("chain_failed")
                        .data(serde_json::json!({
                            "chain_id": task_id.0,
                            "failed_at_step": last_completed_count,
                        }).to_string()));
                    break;
                }
                ChainStatus::Cancelled => {
                    yield Ok(Event::default()
                        .event("chain_cancelled")
                        .data(serde_json::json!({"chain_id": task_id.0}).to_string()));
                    break;
                }
                _ => {}
            }

            tokio::time::sleep(POLL_INTERVAL).await;
        }
    }
}

/// SSE keep-alive configuration for all streams.
pub fn keep_alive() -> KeepAlive {
    KeepAlive::new()
        .interval(Duration::from_secs(15))
        .text("ping")
}
