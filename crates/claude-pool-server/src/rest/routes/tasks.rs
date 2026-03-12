//! Task management REST endpoints.
//!
//! - `POST /v1/tasks` — submit a task
//! - `GET /v1/tasks/:id` — get task status/result
//! - `DELETE /v1/tasks/:id` — cancel a task
//! - `POST /v1/tasks/fan-out` — submit N parallel tasks
//! - `POST /v1/tasks/:id/approve` — approve a pending-review task
//! - `POST /v1/tasks/:id/reject` — reject with feedback

use std::sync::Arc;

use axum::Json;
use axum::extract::{Path, State};
use claude_pool::{PoolStore, TaskId};
use serde::{Deserialize, Serialize};

use crate::rest::AppState;
use crate::rest::error::ProblemDetails;

/// Request body for `POST /v1/tasks`.
#[derive(Debug, Deserialize)]
pub struct SubmitTaskRequest {
    /// The prompt to execute.
    pub prompt: String,
    /// Optional model override.
    pub model: Option<String>,
    /// Optional effort override.
    pub effort: Option<String>,
    /// Tags for grouping/filtering.
    #[serde(default)]
    pub tags: Vec<String>,
    /// Whether this task requires coordinator approval before completion.
    #[serde(default)]
    pub review_required: bool,
    /// Maximum rejections before auto-failing (only with review_required).
    pub max_rejections: Option<u32>,
}

/// Response body for task submission.
#[derive(Debug, Serialize)]
pub struct SubmitTaskResponse {
    pub task_id: String,
    pub state: String,
}

/// Response body for `GET /v1/tasks/:id`.
#[derive(Debug, Serialize)]
pub struct TaskResponse {
    pub task_id: String,
    pub state: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub success: Option<bool>,
    pub cost_microdollars: u64,
    pub turns_used: u32,
}

/// Request body for `POST /v1/tasks/fan-out`.
#[derive(Debug, Deserialize)]
pub struct FanOutRequest {
    /// Prompts to execute in parallel.
    pub prompts: Vec<String>,
    /// Optional model override for all tasks.
    pub model: Option<String>,
    /// Optional effort override for all tasks.
    pub effort: Option<String>,
}

/// Response body for fan-out.
#[derive(Debug, Serialize)]
pub struct FanOutResponse {
    pub results: Vec<FanOutTaskResult>,
}

/// Individual result from a fan-out.
#[derive(Debug, Serialize)]
pub struct FanOutTaskResult {
    pub output: String,
    pub success: bool,
    pub cost_microdollars: u64,
}

/// Request body for `POST /v1/tasks/:id/reject`.
#[derive(Debug, Deserialize)]
pub struct RejectRequest {
    /// Feedback to append before re-queuing.
    pub feedback: String,
}

/// `POST /v1/tasks` — submit a task for async execution.
pub async fn submit_task<S: PoolStore + 'static>(
    State(state): State<Arc<AppState<S>>>,
    Json(req): Json<SubmitTaskRequest>,
) -> Result<(axum::http::StatusCode, Json<SubmitTaskResponse>), ProblemDetails> {
    let task_id = if req.review_required {
        state
            .state
            .pool
            .submit_with_review(
                &req.prompt,
                None, // TODO: pass config once TaskOverrides lands
                req.tags,
                req.max_rejections,
            )
            .await
            .map_err(ProblemDetails::from)?
    } else {
        state
            .state
            .pool
            .submit(&req.prompt)
            .await
            .map_err(ProblemDetails::from)?
    };

    Ok((
        axum::http::StatusCode::ACCEPTED,
        Json(SubmitTaskResponse {
            task_id: task_id.0,
            state: "pending".to_string(),
        }),
    ))
}

/// `GET /v1/tasks/:id` — get task status and result.
pub async fn get_task<S: PoolStore + 'static>(
    State(state): State<Arc<AppState<S>>>,
    Path(task_id): Path<String>,
) -> Result<Json<TaskResponse>, ProblemDetails> {
    let id = TaskId(task_id.clone());
    let record = state
        .state
        .pool
        .store()
        .get_task(&id)
        .await
        .map_err(ProblemDetails::from)?
        .ok_or_else(|| ProblemDetails::not_found("task", &task_id))?;

    let state_str = format!("{:?}", record.state).to_lowercase();

    let (output, success, cost, turns) = match record.result {
        Some(ref r) => (
            Some(r.output.clone()),
            Some(r.success),
            r.cost_microdollars,
            r.turns_used,
        ),
        None => (None, None, 0, 0),
    };

    Ok(Json(TaskResponse {
        task_id: record.id.0,
        state: state_str,
        output,
        success,
        cost_microdollars: cost,
        turns_used: turns,
    }))
}

/// `DELETE /v1/tasks/:id` — cancel a task.
pub async fn cancel_task<S: PoolStore + 'static>(
    State(state): State<Arc<AppState<S>>>,
    Path(task_id): Path<String>,
) -> Result<axum::http::StatusCode, ProblemDetails> {
    let id = TaskId(task_id);
    state
        .state
        .pool
        .cancel(&id)
        .await
        .map_err(ProblemDetails::from)?;

    Ok(axum::http::StatusCode::NO_CONTENT)
}

/// `POST /v1/tasks/fan-out` — submit N tasks in parallel, wait for all results.
pub async fn fan_out<S: PoolStore + 'static>(
    State(state): State<Arc<AppState<S>>>,
    Json(req): Json<FanOutRequest>,
) -> Result<Json<FanOutResponse>, ProblemDetails> {
    if req.prompts.is_empty() {
        return Err(ProblemDetails::bad_request(
            "prompts array must not be empty",
        ));
    }

    let prompt_refs: Vec<&str> = req.prompts.iter().map(|s| s.as_str()).collect();
    let results = state
        .state
        .pool
        .fan_out(&prompt_refs)
        .await
        .map_err(ProblemDetails::from)?;

    let results = results
        .into_iter()
        .map(|r| FanOutTaskResult {
            output: r.output,
            success: r.success,
            cost_microdollars: r.cost_microdollars,
        })
        .collect();

    Ok(Json(FanOutResponse { results }))
}

/// `POST /v1/tasks/:id/approve` — approve a pending-review task.
pub async fn approve_task<S: PoolStore + 'static>(
    State(state): State<Arc<AppState<S>>>,
    Path(task_id): Path<String>,
) -> Result<axum::http::StatusCode, ProblemDetails> {
    let id = TaskId(task_id);
    state
        .state
        .pool
        .approve_result(&id)
        .await
        .map_err(ProblemDetails::from)?;

    Ok(axum::http::StatusCode::OK)
}

/// `POST /v1/tasks/:id/reject` — reject with feedback and re-queue.
pub async fn reject_task<S: PoolStore + 'static>(
    State(state): State<Arc<AppState<S>>>,
    Path(task_id): Path<String>,
    Json(req): Json<RejectRequest>,
) -> Result<axum::http::StatusCode, ProblemDetails> {
    let id = TaskId(task_id);
    state
        .state
        .pool
        .reject_result(&id, &req.feedback)
        .await
        .map_err(ProblemDetails::from)?;

    Ok(axum::http::StatusCode::OK)
}
