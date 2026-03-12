//! Webhook management REST endpoints.
//!
//! - `GET /v1/webhooks` — list registered webhooks
//! - `POST /v1/webhooks` — register a new webhook
//! - `DELETE /v1/webhooks/:id` — remove a webhook

use std::sync::Arc;

use axum::Json;
use axum::extract::{Path, State};
use claude_pool::PoolStore;
use serde::{Deserialize, Serialize};

use crate::rest::AppState;
use crate::rest::error::ProblemDetails;
use crate::rest::webhooks::{Webhook, WebhookEvent};

/// Request body for `POST /v1/webhooks`.
#[derive(Debug, Deserialize)]
pub struct RegisterWebhookRequest {
    /// URL to POST to (HTTP only for now).
    pub url: String,
    /// Events to subscribe to. Empty means all events.
    #[serde(default)]
    pub events: Vec<WebhookEvent>,
}

/// Response body for webhook registration.
#[derive(Debug, Serialize)]
pub struct RegisterWebhookResponse {
    pub id: String,
    pub url: String,
    pub events: Vec<WebhookEvent>,
}

/// `GET /v1/webhooks` — list all registered webhooks.
pub async fn list_webhooks<S: PoolStore + 'static>(
    State(state): State<Arc<AppState<S>>>,
) -> Json<Vec<Webhook>> {
    Json(state.webhooks.list().await)
}

/// `POST /v1/webhooks` — register a new webhook.
pub async fn register_webhook<S: PoolStore + 'static>(
    State(state): State<Arc<AppState<S>>>,
    Json(req): Json<RegisterWebhookRequest>,
) -> Result<(axum::http::StatusCode, Json<RegisterWebhookResponse>), ProblemDetails> {
    if !req.url.starts_with("http://") {
        return Err(ProblemDetails::bad_request(
            "only http:// URLs are supported (HTTPS support coming soon)",
        ));
    }

    let id = state
        .webhooks
        .register(req.url.clone(), req.events.clone())
        .await;

    Ok((
        axum::http::StatusCode::CREATED,
        Json(RegisterWebhookResponse {
            id,
            url: req.url,
            events: req.events,
        }),
    ))
}

/// `DELETE /v1/webhooks/:id` — remove a webhook.
pub async fn remove_webhook<S: PoolStore + 'static>(
    State(state): State<Arc<AppState<S>>>,
    Path(id): Path<String>,
) -> Result<axum::http::StatusCode, ProblemDetails> {
    if state.webhooks.remove(&id).await {
        Ok(axum::http::StatusCode::NO_CONTENT)
    } else {
        Err(ProblemDetails::not_found("webhook", &id))
    }
}
