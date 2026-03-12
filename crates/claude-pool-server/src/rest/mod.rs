//! REST API for claude-pool.
//!
//! Provides an axum-based HTTP API as an alternative transport to MCP.
//! Both transports share the same [`claude_pool::Pool`] backend.
//!
//! Base URL: `http://localhost:{port}/v1` (port configured via `--rest-port`).
//! All request/response bodies are `application/json`.
//!
//! # Endpoints
//!
//! ## Health
//! - `GET /health` — liveness check, always returns `"ok"`, no auth required
//!
//! ## Tasks (8 endpoints)
//! - `GET /v1/tasks` — list tasks (`?state=&tag=&limit=50&offset=0`)
//! - `POST /v1/tasks` — submit a task (`{prompt, model?, effort?, tags?, review_required?}`)
//! - `GET /v1/tasks/:id` — get task status and result
//! - `GET /v1/tasks/:id/stream` — SSE stream of task output
//! - `DELETE /v1/tasks/:id` — cancel a pending/running task
//! - `POST /v1/tasks/fan-out` — parallel fan-out (`{prompts[], model?, effort?}`)
//! - `POST /v1/tasks/:id/approve` — approve a task pending review
//! - `POST /v1/tasks/:id/reject` — reject with feedback (`{feedback}`)
//!
//! ## Chains (5 endpoints)
//! - `GET /v1/chains` — list chains (`?limit=50&offset=0`)
//! - `POST /v1/chains` — submit a chain (`{steps[], tags?, isolation?}`)
//! - `GET /v1/chains/:id` — get chain progress and completed steps
//! - `GET /v1/chains/:id/stream` — SSE stream of chain progress
//! - `DELETE /v1/chains/:id` — cancel a running chain
//!
//! ## Slots (2 endpoints)
//! - `GET /v1/slots` — list slots (`?name=&role=&state=`)
//! - `GET /v1/slots/:id` — get slot details
//!
//! ## Skills (4 endpoints)
//! - `GET /v1/skills` — list all registered skills
//! - `GET /v1/skills/:name` — get skill details
//! - `POST /v1/skills` — register a skill (`{name, description, prompt, scope?, arguments?}`)
//! - `DELETE /v1/skills/:name` — remove a skill
//!
//! ## Context (4 endpoints)
//! - `GET /v1/context` — list all key-value entries
//! - `GET /v1/context/:key` — get a value
//! - `PUT /v1/context/:key` — set a value (`{value}`)
//! - `DELETE /v1/context/:key` — delete a key
//!
//! ## Webhooks (3 endpoints)
//! - `GET /v1/webhooks` — list registered webhooks
//! - `POST /v1/webhooks` — register a webhook (`{url, events?}`, HTTP only)
//! - `DELETE /v1/webhooks/:id` — remove a webhook
//!
//! ## Pool (5 endpoints)
//! - `GET /v1/pool/status` — pool health snapshot (slots, tasks, spend, budget)
//! - `GET /v1/pool/metrics` — session metrics (`?since_ms=&until_ms=&tag=&model=`)
//! - `GET /v1/pool/events` — SSE stream of pool status updates
//! - `POST /v1/pool/drain` — graceful shutdown, waits for in-flight tasks
//! - `POST /v1/pool/scale` — scale slots (`{target}` or `{delta}`, not both)
//!
//! # Authentication
//!
//! When bearer tokens are configured via `--http-token`, all endpoints except
//! `/health` require a valid `Authorization: Bearer <token>` header.
//! Multiple tokens can be configured. If no tokens are set, all endpoints are public.
//!
//! ```bash
//! # Start with auth
//! claude-pool-server --rest --rest-port 3200 --http-token sk-test-123
//!
//! # Use in requests
//! curl -H "Authorization: Bearer sk-test-123" http://localhost:3200/v1/pool/status
//! ```
//!
//! # Pagination
//!
//! List endpoints for tasks and chains support `?limit=N&offset=M` query parameters.
//! Default: `limit=50, offset=0`. Response wraps items in:
//!
//! ```json
//! {"items": [...], "total": 150, "limit": 50, "offset": 0}
//! ```
//!
//! # Error Responses
//!
//! All errors use [RFC 9457](https://www.rfc-editor.org/rfc/rfc9457) Problem Details
//! with content type `application/problem+json`:
//!
//! ```json
//! {
//!   "type": "urn:claude-pool:error:not-found",
//!   "title": "Task not found",
//!   "status": 404,
//!   "detail": "Task task_abc123 does not exist"
//! }
//! ```
//!
//! Common status codes: 400 (bad request), 404 (not found), 409 (conflict/no slots),
//! 500 (internal error), 503 (concurrency limit exceeded).
//!
//! # Concurrency Limiting
//!
//! When configured via [`RestConfig`], a global concurrency limit caps the
//! number of in-flight requests. Excess requests receive 503 Service Unavailable.
//!
//! # Server-Sent Events (SSE)
//!
//! Three endpoints stream events: task output (`/v1/tasks/:id/stream`),
//! chain progress (`/v1/chains/:id/stream`), and pool status (`/v1/pool/events`).
//! Connections close automatically on completion or shutdown.

pub mod error;
pub mod middleware;
pub mod routes;
pub mod sse;
pub mod webhooks;

use std::sync::Arc;

use axum::Router;
use axum::routing::{delete, get, post, put};
use claude_pool::PoolStore;
use tower_http::cors::CorsLayer;
use tower_http::trace::TraceLayer;

use crate::State;
use crate::auth::BearerTokens;

use self::webhooks::WebhookRegistry;

/// Configuration for the REST API server.
pub struct RestConfig {
    /// Bearer tokens for authentication. Empty disables auth.
    pub tokens: BearerTokens,
    /// Maximum concurrent requests (0 = unlimited).
    pub max_concurrent_requests: usize,
}

impl Default for RestConfig {
    fn default() -> Self {
        Self {
            tokens: BearerTokens::new(vec![]),
            max_concurrent_requests: 0,
        }
    }
}

/// Shared state for REST handlers.
///
/// Wraps the existing MCP [`State`] so both transports share the same pool.
pub struct AppState<S: PoolStore> {
    pub state: Arc<State<S>>,
    pub webhooks: WebhookRegistry,
}

/// Build the REST API router.
///
/// The returned router includes all v1 endpoints with CORS, tracing,
/// optional bearer auth, and optional rate limiting.
pub fn router<S: PoolStore + 'static>(state: Arc<State<S>>, config: RestConfig) -> Router {
    let app_state = Arc::new(AppState {
        state,
        webhooks: WebhookRegistry::new(),
    });

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

    let webhooks = Router::new()
        .route("/", get(routes::webhooks::list_webhooks::<S>))
        .route("/", post(routes::webhooks::register_webhook::<S>))
        .route("/{id}", delete(routes::webhooks::remove_webhook::<S>));

    let pool = Router::new()
        .route("/status", get(routes::pool::get_status::<S>))
        .route("/metrics", get(routes::pool::get_metrics::<S>))
        .route("/events", get(routes::pool::events::<S>))
        .route("/drain", post(routes::pool::drain::<S>))
        .route("/scale", post(routes::pool::scale::<S>));

    let mut app = Router::new()
        .nest("/v1/tasks", tasks)
        .nest("/v1/chains", chains)
        .nest("/v1/skills", skills)
        .nest("/v1/context", context)
        .nest("/v1/slots", slots)
        .nest("/v1/webhooks", webhooks)
        .nest("/v1/pool", pool)
        .route("/health", get(health))
        .with_state(app_state);

    // Apply bearer auth middleware if tokens are configured.
    if !config.tokens.is_empty() {
        let tokens = config.tokens;
        app = app.layer(axum::middleware::from_fn(move |req, next| {
            middleware::bearer_auth(req, next, tokens.clone())
        }));
    }

    // Apply concurrency limiting if configured.
    if config.max_concurrent_requests > 0 {
        app = app.layer(tower::limit::ConcurrencyLimitLayer::new(
            config.max_concurrent_requests,
        ));
    }

    app.layer(TraceLayer::new_for_http())
        .layer(CorsLayer::permissive())
}

/// Health check endpoint.
async fn health() -> &'static str {
    "ok"
}
