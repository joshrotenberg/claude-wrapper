//! Pool management REST endpoints.
//!
//! - `GET /v1/pool/status` — pool health
//! - `POST /v1/pool/drain` — graceful shutdown
//! - `POST /v1/pool/scale` — scale slots up/down

use std::sync::Arc;

use axum::Json;
use axum::extract::State;
use claude_pool::PoolStore;
use serde::{Deserialize, Serialize};

use crate::rest::AppState;
use crate::rest::error::ProblemDetails;

/// Response body for `GET /v1/pool/status`.
#[derive(Debug, Serialize)]
pub struct PoolStatusResponse {
    pub total_slots: usize,
    pub idle_slots: usize,
    pub busy_slots: usize,
    pub pending_tasks: usize,
    pub running_tasks: usize,
    pub completed_tasks: usize,
    pub failed_tasks: usize,
    pub cancelled_tasks: usize,
    pub total_spend_microdollars: u64,
    pub budget_microdollars: Option<u64>,
    pub shutdown: bool,
    pub server_version: String,
    pub server_model: Option<String>,
}

/// Request body for `POST /v1/pool/scale`.
#[derive(Debug, Deserialize)]
pub struct ScaleRequest {
    /// Absolute target slot count (mutually exclusive with delta).
    pub target: Option<usize>,
    /// Relative change (+2 or -1) (mutually exclusive with target).
    pub delta: Option<i32>,
}

/// Response body for scale operations.
#[derive(Debug, Serialize)]
pub struct ScaleResponse {
    pub previous_slots: usize,
    pub current_slots: usize,
}

/// `GET /v1/pool/status` — get pool health.
pub async fn get_status<S: PoolStore + 'static>(
    State(state): State<Arc<AppState<S>>>,
) -> Result<Json<PoolStatusResponse>, ProblemDetails> {
    let status = state
        .state
        .pool
        .status()
        .await
        .map_err(ProblemDetails::from)?;

    Ok(Json(PoolStatusResponse {
        total_slots: status.total_slots,
        idle_slots: status.idle_slots,
        busy_slots: status.busy_slots,
        pending_tasks: status.pending_tasks,
        running_tasks: status.running_tasks,
        completed_tasks: status.completed_tasks,
        failed_tasks: status.failed_tasks,
        cancelled_tasks: status.cancelled_tasks,
        total_spend_microdollars: status.total_spend_microdollars,
        budget_microdollars: status.budget_microdollars,
        shutdown: status.shutdown,
        server_version: state.state.server_info.version.clone(),
        server_model: state.state.server_info.model.clone(),
    }))
}

/// `POST /v1/pool/drain` — graceful shutdown.
pub async fn drain<S: PoolStore + 'static>(
    State(state): State<Arc<AppState<S>>>,
) -> Result<Json<serde_json::Value>, ProblemDetails> {
    let summary = state
        .state
        .pool
        .drain()
        .await
        .map_err(ProblemDetails::from)?;

    Ok(Json(serde_json::json!({
        "drained": true,
        "total_tasks_completed": summary.total_tasks_completed,
        "total_cost_microdollars": summary.total_cost_microdollars,
    })))
}

/// `POST /v1/pool/scale` — scale slots up or down.
pub async fn scale<S: PoolStore + 'static>(
    State(state): State<Arc<AppState<S>>>,
    Json(req): Json<ScaleRequest>,
) -> Result<Json<ScaleResponse>, ProblemDetails> {
    let current_status = state
        .state
        .pool
        .status()
        .await
        .map_err(ProblemDetails::from)?;
    let previous = current_status.total_slots;

    match (req.target, req.delta) {
        (Some(target), None) => {
            state
                .state
                .pool
                .set_target_slots(target)
                .await
                .map_err(ProblemDetails::from)?;
        }
        (None, Some(delta)) => {
            if delta > 0 {
                state
                    .state
                    .pool
                    .scale_up(delta as usize)
                    .await
                    .map_err(ProblemDetails::from)?;
            } else if delta < 0 {
                state
                    .state
                    .pool
                    .scale_down((-delta) as usize)
                    .await
                    .map_err(ProblemDetails::from)?;
            }
        }
        (Some(_), Some(_)) => {
            return Err(ProblemDetails::bad_request(
                "Specify either 'target' or 'delta', not both.",
            ));
        }
        (None, None) => {
            return Err(ProblemDetails::bad_request(
                "Specify either 'target' or 'delta'.",
            ));
        }
    }

    let new_status = state
        .state
        .pool
        .status()
        .await
        .map_err(ProblemDetails::from)?;

    Ok(Json(ScaleResponse {
        previous_slots: previous,
        current_slots: new_status.total_slots,
    }))
}

/// Query parameters for `GET /v1/pool/metrics`.
#[derive(Debug, Deserialize, Default)]
pub struct MetricsQuery {
    /// Only include tasks created after this time (millis since epoch).
    pub since_ms: Option<u64>,
    /// Only include tasks created before this time (millis since epoch).
    pub until_ms: Option<u64>,
    /// Only include tasks with this tag.
    pub tag: Option<String>,
    /// Only include tasks that ran on this model.
    pub model: Option<String>,
}

/// `GET /v1/pool/metrics` — aggregated session metrics.
///
/// Returns cost, timing, and model breakdowns for the current session.
/// Supports optional query filters: `?since_ms=&until_ms=&tag=&model=`.
pub async fn get_metrics<S: PoolStore + 'static>(
    State(state): State<Arc<AppState<S>>>,
    axum::extract::Query(query): axum::extract::Query<MetricsQuery>,
) -> Result<Json<claude_pool::types::SessionMetrics>, ProblemDetails> {
    let filter = claude_pool::types::MetricsFilter {
        since_ms: query.since_ms,
        until_ms: query.until_ms,
        tags: query.tag.map(|t| vec![t]),
        model: query.model,
    };

    let metrics = state
        .state
        .pool
        .session_metrics(&filter)
        .await
        .map_err(ProblemDetails::from)?;

    Ok(Json(metrics))
}

/// `GET /v1/pool/events` — SSE stream of pool-wide events.
///
/// Emits periodic status snapshots and task state changes.
/// Useful for dashboards that want real-time pool visibility.
pub async fn events<S: PoolStore + 'static>(
    State(state): State<Arc<AppState<S>>>,
) -> axum::response::sse::Sse<
    impl tokio_stream::Stream<Item = Result<axum::response::sse::Event, std::convert::Infallible>>,
> {
    use std::convert::Infallible;
    use std::time::Duration;

    use axum::response::sse::Event;

    let stream = async_stream::stream! {
        let mut last_running = 0usize;
        let mut last_completed = 0u64;

        loop {
            if let Ok(status) = state.state.pool.status().await {
                let running = status.running_tasks;
                let completed = status.total_spend_microdollars; // proxy for progress

                // Emit status on every poll (dashboards want heartbeats).
                yield Ok::<_, Infallible>(Event::default()
                    .event("status")
                    .data(serde_json::json!({
                        "total_slots": status.total_slots,
                        "idle_slots": status.idle_slots,
                        "busy_slots": status.busy_slots,
                        "running_tasks": status.running_tasks,
                        "pending_tasks": status.pending_tasks,
                        "total_spend_microdollars": status.total_spend_microdollars,
                        "shutdown": status.shutdown,
                    }).to_string()));

                // Detect transitions.
                if running != last_running || completed != last_completed {
                    last_running = running;
                    last_completed = completed;
                }

                if status.shutdown {
                    yield Ok(Event::default()
                        .event("shutdown")
                        .data(r#"{"shutdown": true}"#));
                    break;
                }
            }

            tokio::time::sleep(Duration::from_secs(2)).await;
        }
    };

    axum::response::sse::Sse::new(stream).keep_alive(crate::rest::sse::keep_alive())
}
