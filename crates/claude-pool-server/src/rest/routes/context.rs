//! Context key-value store REST endpoints.
//!
//! - `GET /v1/context` — list all context entries
//! - `GET /v1/context/:key` — get a value
//! - `PUT /v1/context/:key` — set a value
//! - `DELETE /v1/context/:key` — delete a key

use std::sync::Arc;

use axum::Json;
use axum::extract::{Path, State};
use claude_pool::PoolStore;
use serde::{Deserialize, Serialize};

use crate::rest::AppState;
use crate::rest::error::ProblemDetails;

/// Response body for a single context entry.
#[derive(Debug, Serialize)]
pub struct ContextEntry {
    pub key: String,
    pub value: String,
}

/// Request body for `PUT /v1/context/:key`.
#[derive(Debug, Deserialize)]
pub struct SetContextRequest {
    pub value: String,
}

/// `GET /v1/context` — list all context key-value pairs.
pub async fn list_context<S: PoolStore + 'static>(
    State(state): State<Arc<AppState<S>>>,
) -> Json<Vec<ContextEntry>> {
    let entries: Vec<ContextEntry> = state
        .state
        .pool
        .list_context()
        .into_iter()
        .map(|(key, value)| ContextEntry { key, value })
        .collect();
    Json(entries)
}

/// `GET /v1/context/:key` — get a context value.
pub async fn get_context<S: PoolStore + 'static>(
    State(state): State<Arc<AppState<S>>>,
    Path(key): Path<String>,
) -> Result<Json<ContextEntry>, ProblemDetails> {
    let value = state
        .state
        .pool
        .get_context(&key)
        .ok_or_else(|| ProblemDetails::not_found("context key", &key))?;
    Ok(Json(ContextEntry { key, value }))
}

/// `PUT /v1/context/:key` — set a context value.
pub async fn set_context<S: PoolStore + 'static>(
    State(state): State<Arc<AppState<S>>>,
    Path(key): Path<String>,
    Json(req): Json<SetContextRequest>,
) -> axum::http::StatusCode {
    state.state.pool.set_context(key, req.value);
    axum::http::StatusCode::NO_CONTENT
}

/// `DELETE /v1/context/:key` — delete a context entry.
pub async fn delete_context<S: PoolStore + 'static>(
    State(state): State<Arc<AppState<S>>>,
    Path(key): Path<String>,
) -> Result<axum::http::StatusCode, ProblemDetails> {
    state
        .state
        .pool
        .delete_context(&key)
        .ok_or_else(|| ProblemDetails::not_found("context key", &key))?;
    Ok(axum::http::StatusCode::NO_CONTENT)
}
