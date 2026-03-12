//! Task management REST endpoints.
//!
//! - `GET /v1/tasks` — list tasks with optional filtering
//! - `POST /v1/tasks` — submit a task
//! - `GET /v1/tasks/:id` — get task status/result
//! - `DELETE /v1/tasks/:id` — cancel a task
//! - `POST /v1/tasks/fan-out` — submit N parallel tasks
//! - `POST /v1/tasks/:id/approve` — approve a pending-review task
//! - `POST /v1/tasks/:id/reject` — reject with feedback

use std::sync::Arc;

use axum::Json;
use axum::extract::{Path, Query, State};
use claude_pool::{PoolStore, TaskFilter, TaskId, TaskState};
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

/// Pagination query parameters.
#[derive(Debug, Deserialize)]
pub struct PaginationQuery {
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

/// Query parameters for `GET /v1/tasks`.
#[derive(Debug, Deserialize)]
pub struct ListTasksQuery {
    /// Filter by task state (pending, running, completed, failed, cancelled).
    pub state: Option<String>,
    /// Filter by tag (any match).
    pub tag: Option<String>,
    /// Maximum number of items per page (default: 50).
    #[serde(default = "default_limit")]
    pub limit: usize,
    /// Offset for pagination (default: 0).
    #[serde(default)]
    pub offset: usize,
}

/// `GET /v1/tasks` — list tasks with optional filtering and pagination.
pub async fn list_tasks<S: PoolStore + 'static>(
    State(state): State<Arc<AppState<S>>>,
    Query(query): Query<ListTasksQuery>,
) -> Result<Json<PaginatedResponse<TaskResponse>>, ProblemDetails> {
    let task_state = query.state.as_deref().and_then(parse_task_state);
    let tags = query.tag.map(|t| vec![t]);

    let filter = TaskFilter {
        state: task_state,
        slot_id: None,
        tags,
    };

    let records = state
        .state
        .pool
        .store()
        .list_tasks(&filter)
        .await
        .map_err(ProblemDetails::from)?;

    let total = records.len();
    let start = query.offset.min(total);
    let end = (query.offset + query.limit).min(total);

    let tasks: Vec<TaskResponse> = records[start..end]
        .iter()
        .map(|record| {
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
            TaskResponse {
                task_id: record.id.0.clone(),
                state: state_str,
                output,
                success,
                cost_microdollars: cost,
                turns_used: turns,
            }
        })
        .collect();

    Ok(Json(PaginatedResponse {
        items: tasks,
        total,
        limit: query.limit,
        offset: query.offset,
    }))
}

fn parse_task_state(s: &str) -> Option<TaskState> {
    match s {
        "pending" => Some(TaskState::Pending),
        "running" => Some(TaskState::Running),
        "completed" => Some(TaskState::Completed),
        "failed" => Some(TaskState::Failed),
        "cancelled" => Some(TaskState::Cancelled),
        "pending_review" | "pendingreview" => Some(TaskState::PendingReview),
        _ => None,
    }
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

/// `GET /v1/tasks/:id/stream` — SSE stream of task output.
pub async fn stream_task<S: PoolStore + 'static>(
    State(state): State<Arc<AppState<S>>>,
    Path(task_id): Path<String>,
) -> Result<
    axum::response::sse::Sse<
        impl tokio_stream::Stream<Item = Result<axum::response::sse::Event, std::convert::Infallible>>,
    >,
    ProblemDetails,
> {
    let id = TaskId(task_id.clone());

    // Verify task exists before starting stream.
    state
        .state
        .pool
        .store()
        .get_task(&id)
        .await
        .map_err(ProblemDetails::from)?
        .ok_or_else(|| ProblemDetails::not_found("task", &task_id))?;

    let stream = crate::rest::sse::task_stream(state.state.clone(), id);
    Ok(axum::response::sse::Sse::new(stream).keep_alive(crate::rest::sse::keep_alive()))
}
