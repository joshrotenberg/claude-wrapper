//! REST API for claude-pool.
//!
//! Provides an axum-based HTTP API as an alternative transport to MCP.
//! Both transports share the same [`claude_pool::Pool`] backend.
//!
//! # Endpoints
//!
//! ## Tasks
//! - `POST /v1/tasks` — submit a task
//! - `GET /v1/tasks/:id` — get task result
//! - `DELETE /v1/tasks/:id` — cancel a task
//! - `POST /v1/tasks/fan-out` — parallel fan-out
//! - `POST /v1/tasks/:id/approve` — approve pending review
//! - `POST /v1/tasks/:id/reject` — reject with feedback
//!
//! ## Chains
//! - `POST /v1/chains` — submit a chain
//! - `GET /v1/chains/:id` — get chain progress
//! - `DELETE /v1/chains/:id` — cancel a chain
//!
//! ## Pool
//! - `GET /v1/pool/status` — pool health
//! - `POST /v1/pool/drain` — graceful shutdown
//! - `POST /v1/pool/scale` — scale slots

pub mod error;
pub mod routes;

use std::sync::Arc;

use axum::Router;
use axum::routing::{delete, get, post};
use claude_pool::PoolStore;
use tower_http::cors::CorsLayer;
use tower_http::trace::TraceLayer;

use crate::State;

/// Shared state for REST handlers.
///
/// Wraps the existing MCP [`State`] so both transports share the same pool.
pub struct AppState<S: PoolStore> {
    pub state: Arc<State<S>>,
}

/// Build the REST API router.
///
/// The returned router includes all v1 endpoints with CORS and tracing middleware.
pub fn router<S: PoolStore + 'static>(state: Arc<State<S>>) -> Router {
    let app_state = Arc::new(AppState { state });

    let tasks = Router::new()
        .route("/", post(routes::tasks::submit_task::<S>))
        .route("/fan-out", post(routes::tasks::fan_out::<S>))
        .route("/{id}", get(routes::tasks::get_task::<S>))
        .route("/{id}", delete(routes::tasks::cancel_task::<S>))
        .route("/{id}/approve", post(routes::tasks::approve_task::<S>))
        .route("/{id}/reject", post(routes::tasks::reject_task::<S>));

    let chains = Router::new()
        .route("/", post(routes::chains::submit_chain::<S>))
        .route("/{id}", get(routes::chains::get_chain::<S>))
        .route("/{id}", delete(routes::chains::cancel_chain::<S>));

    let pool = Router::new()
        .route("/status", get(routes::pool::get_status::<S>))
        .route("/drain", post(routes::pool::drain::<S>))
        .route("/scale", post(routes::pool::scale::<S>));

    Router::new()
        .nest("/v1/tasks", tasks)
        .nest("/v1/chains", chains)
        .nest("/v1/pool", pool)
        .route("/health", get(health))
        .with_state(app_state)
        .layer(TraceLayer::new_for_http())
        .layer(CorsLayer::permissive())
}

/// Health check endpoint.
async fn health() -> &'static str {
    "ok"
}
