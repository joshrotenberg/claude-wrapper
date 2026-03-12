//! REST API for claude-pool.
//!
//! Provides an axum-based HTTP API as an alternative transport to MCP.
//! Both transports share the same [`claude_pool::Pool`] backend.
//!
//! # Endpoints
//!
//! ## Tasks
//! - `GET /v1/tasks` — list tasks with filtering
//! - `POST /v1/tasks` — submit a task
//! - `GET /v1/tasks/:id` — get task result
//! - `GET /v1/tasks/:id/stream` — SSE stream of task output
//! - `DELETE /v1/tasks/:id` — cancel a task
//! - `POST /v1/tasks/fan-out` — parallel fan-out
//! - `POST /v1/tasks/:id/approve` — approve pending review
//! - `POST /v1/tasks/:id/reject` — reject with feedback
//!
//! ## Chains
//! - `GET /v1/chains` — list all chains
//! - `POST /v1/chains` — submit a chain
//! - `GET /v1/chains/:id` — get chain progress
//! - `GET /v1/chains/:id/stream` — SSE stream of chain progress
//! - `DELETE /v1/chains/:id` — cancel a chain
//!
//! ## Skills
//! - `GET /v1/skills` — list skills
//! - `GET /v1/skills/:name` — get skill details
//! - `POST /v1/skills` — register a skill
//! - `DELETE /v1/skills/:name` — remove a skill
//!
//! ## Context
//! - `GET /v1/context` — list context entries
//! - `GET /v1/context/:key` — get a value
//! - `PUT /v1/context/:key` — set a value
//! - `DELETE /v1/context/:key` — delete a key
//!
//! ## Slots
//! - `GET /v1/slots` — list slots with filtering
//! - `GET /v1/slots/:id` — get slot details
//!
//! ## Pool
//! - `GET /v1/pool/status` — pool health
//! - `GET /v1/pool/events` — SSE stream of pool events
//! - `POST /v1/pool/drain` — graceful shutdown
//! - `POST /v1/pool/scale` — scale slots

pub mod error;
pub mod routes;
pub mod sse;

use std::sync::Arc;

use axum::Router;
use axum::routing::{delete, get, post, put};
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
        .route("/", get(routes::tasks::list_tasks::<S>))
        .route("/", post(routes::tasks::submit_task::<S>))
        .route("/fan-out", post(routes::tasks::fan_out::<S>))
        .route("/{id}", get(routes::tasks::get_task::<S>))
        .route("/{id}", delete(routes::tasks::cancel_task::<S>))
        .route("/{id}/approve", post(routes::tasks::approve_task::<S>))
        .route("/{id}/reject", post(routes::tasks::reject_task::<S>))
        .route("/{id}/stream", get(routes::tasks::stream_task::<S>));

    let chains = Router::new()
        .route("/", get(routes::chains::list_chains::<S>))
        .route("/", post(routes::chains::submit_chain::<S>))
        .route("/{id}", get(routes::chains::get_chain::<S>))
        .route("/{id}", delete(routes::chains::cancel_chain::<S>))
        .route("/{id}/stream", get(routes::chains::stream_chain::<S>));

    let skills = Router::new()
        .route("/", get(routes::skills::list_skills::<S>))
        .route("/", post(routes::skills::register_skill::<S>))
        .route("/{name}", get(routes::skills::get_skill::<S>))
        .route("/{name}", delete(routes::skills::remove_skill::<S>));

    let context = Router::new()
        .route("/", get(routes::context::list_context::<S>))
        .route("/{key}", get(routes::context::get_context::<S>))
        .route("/{key}", put(routes::context::set_context::<S>))
        .route("/{key}", delete(routes::context::delete_context::<S>));

    let slots = Router::new()
        .route("/", get(routes::slots::list_slots::<S>))
        .route("/{id}", get(routes::slots::get_slot::<S>));

    let pool = Router::new()
        .route("/status", get(routes::pool::get_status::<S>))
        .route("/events", get(routes::pool::events::<S>))
        .route("/drain", post(routes::pool::drain::<S>))
        .route("/scale", post(routes::pool::scale::<S>));

    Router::new()
        .nest("/v1/tasks", tasks)
        .nest("/v1/chains", chains)
        .nest("/v1/skills", skills)
        .nest("/v1/context", context)
        .nest("/v1/slots", slots)
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
