//! Chain management REST endpoints.
//!
//! - `GET /v1/chains` — list all chains
//! - `POST /v1/chains` — submit a chain
//! - `GET /v1/chains/:id` — get chain progress
//! - `DELETE /v1/chains/:id` — cancel a chain

use std::collections::HashMap;
use std::sync::Arc;

use axum::Json;
use axum::extract::{Path, Query, State};
use claude_pool::{ChainOptions, ChainStep, PoolStore, StepAction, TaskId};
use serde::{Deserialize, Serialize};

use crate::rest::AppState;
use crate::rest::error::ProblemDetails;

/// A single step in a chain submission.
#[derive(Debug, Deserialize)]
pub struct ChainStepRequest {
    /// Step name.
    pub name: String,
    /// The prompt for this step.
    pub prompt: String,
    /// Optional model override for this step.
    pub model: Option<String>,
    /// Optional effort override for this step.
    pub effort: Option<String>,
}

/// Request body for `POST /v1/chains`.
#[derive(Debug, Deserialize)]
pub struct SubmitChainRequest {
    /// Ordered list of chain steps.
    pub steps: Vec<ChainStepRequest>,
    /// Tags for grouping/filtering.
    #[serde(default)]
    pub tags: Vec<String>,
}

/// Response body for chain submission.
#[derive(Debug, Serialize)]
pub struct SubmitChainResponse {
    pub chain_id: String,
    pub total_steps: usize,
}

/// A completed step in a chain progress response.
#[derive(Debug, Serialize)]
pub struct CompletedStepResponse {
    pub name: String,
    pub success: bool,
    pub output: String,
    pub cost_microdollars: u64,
}

/// Response body for `GET /v1/chains/:id`.
#[derive(Debug, Serialize)]
pub struct ChainProgressResponse {
    pub chain_id: String,
    pub status: String,
    pub total_steps: usize,
    pub current_step: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub current_step_name: Option<String>,
    pub completed_steps: Vec<CompletedStepResponse>,
}

/// Summary entry for chain listing.
#[derive(Debug, Serialize, Clone)]
pub struct ChainSummary {
    pub chain_id: String,
    pub status: String,
    pub total_steps: usize,
    pub completed_steps: usize,
}

/// Pagination query parameters for `GET /v1/chains`.
#[derive(Debug, Deserialize)]
pub struct ListChainsQuery {
    /// Maximum number of items per page (default: 50).
    #[serde(default = "default_limit")]
    pub limit: usize,
    /// Offset for pagination (default: 0).
    #[serde(default)]
    pub offset: usize,
}

fn default_limit() -> usize {
    50
}

/// Paginated response wrapper.
#[derive(Debug, Serialize)]
pub struct PaginatedResponse<T> {
    /// Items in this page.
    pub items: Vec<T>,
    /// Total number of items available.
    pub total: usize,
    /// Items per page.
    pub limit: usize,
    /// Offset of this page.
    pub offset: usize,
}

/// `GET /v1/chains` — list all tracked chains with pagination.
pub async fn list_chains<S: PoolStore + 'static>(
    State(state): State<Arc<AppState<S>>>,
    Query(query): Query<ListChainsQuery>,
) -> Json<PaginatedResponse<ChainSummary>> {
    let entries = state.state.pool.list_chain_progress();
    let summaries: Vec<ChainSummary> = entries
        .into_iter()
        .map(|(id, progress)| ChainSummary {
            chain_id: id.0,
            status: format!("{:?}", progress.status).to_lowercase(),
            total_steps: progress.total_steps,
            completed_steps: progress.completed_steps.len(),
        })
        .collect();

    let total = summaries.len();
    let start = query.offset.min(total);
    let end = (query.offset + query.limit).min(total);
    let items = summaries[start..end].to_vec();

    Json(PaginatedResponse {
        items,
        total,
        limit: query.limit,
        offset: query.offset,
    })
}

/// `POST /v1/chains` — submit a chain for async execution.
pub async fn submit_chain<S: PoolStore + 'static>(
    State(state): State<Arc<AppState<S>>>,
    Json(req): Json<SubmitChainRequest>,
) -> Result<(axum::http::StatusCode, Json<SubmitChainResponse>), ProblemDetails> {
    if req.steps.is_empty() {
        return Err(ProblemDetails::bad_request("steps array must not be empty"));
    }

    let total_steps = req.steps.len();

    let steps: Vec<ChainStep> = req
        .steps
        .into_iter()
        .map(|s| ChainStep {
            name: s.name,
            action: StepAction::Prompt { prompt: s.prompt },
            config: None, // TODO: pass config once TaskOverrides lands
            failure_policy: Default::default(),
            output_vars: HashMap::new(),
        })
        .collect();

    let options = ChainOptions {
        tags: req.tags,
        ..Default::default()
    };

    let skills = state.state.skills.read().await;
    let chain_id = state
        .state
        .pool
        .submit_chain(steps, &skills, options)
        .await
        .map_err(ProblemDetails::from)?;

    Ok((
        axum::http::StatusCode::ACCEPTED,
        Json(SubmitChainResponse {
            chain_id: chain_id.0,
            total_steps,
        }),
    ))
}

/// `GET /v1/chains/:id` — get chain progress.
pub async fn get_chain<S: PoolStore + 'static>(
    State(state): State<Arc<AppState<S>>>,
    Path(chain_id): Path<String>,
) -> Result<Json<ChainProgressResponse>, ProblemDetails> {
    let id = TaskId(chain_id.clone());
    let progress = state
        .state
        .pool
        .chain_progress(&id)
        .ok_or_else(|| ProblemDetails::not_found("chain", &chain_id))?;

    let completed_steps = progress
        .completed_steps
        .iter()
        .map(|s| CompletedStepResponse {
            name: s.name.clone(),
            success: s.success,
            output: s.output.clone(),
            cost_microdollars: s.cost_microdollars,
        })
        .collect();

    Ok(Json(ChainProgressResponse {
        chain_id,
        status: format!("{:?}", progress.status).to_lowercase(),
        total_steps: progress.total_steps,
        current_step: progress.current_step,
        current_step_name: progress.current_step_name.clone(),
        completed_steps,
    }))
}

/// `DELETE /v1/chains/:id` — cancel a running chain.
pub async fn cancel_chain<S: PoolStore + 'static>(
    State(state): State<Arc<AppState<S>>>,
    Path(chain_id): Path<String>,
) -> Result<axum::http::StatusCode, ProblemDetails> {
    let id = TaskId(chain_id);
    state
        .state
        .pool
        .cancel_chain(&id)
        .await
        .map_err(ProblemDetails::from)?;

    Ok(axum::http::StatusCode::NO_CONTENT)
}

/// `GET /v1/chains/:id/stream` — SSE stream of chain progress.
pub async fn stream_chain<S: PoolStore + 'static>(
    State(state): State<Arc<AppState<S>>>,
    Path(chain_id): Path<String>,
) -> Result<
    axum::response::sse::Sse<
        impl tokio_stream::Stream<Item = Result<axum::response::sse::Event, std::convert::Infallible>>,
    >,
    ProblemDetails,
> {
    let id = TaskId(chain_id.clone());

    // Verify chain exists before starting stream.
    state
        .state
        .pool
        .chain_progress(&id)
        .ok_or_else(|| ProblemDetails::not_found("chain", &chain_id))?;

    let stream = crate::rest::sse::chain_stream(state.state.clone(), id);
    Ok(axum::response::sse::Sse::new(stream).keep_alive(crate::rest::sse::keep_alive()))
}
