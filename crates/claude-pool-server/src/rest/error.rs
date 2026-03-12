//! RFC 9457 Problem Details error responses for the REST API.

use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde::Serialize;

/// RFC 9457 Problem Details response body.
#[derive(Debug, Serialize)]
pub struct ProblemDetails {
    /// URI identifying the problem type.
    #[serde(rename = "type")]
    pub problem_type: String,
    /// Short human-readable summary.
    pub title: String,
    /// HTTP status code.
    pub status: u16,
    /// Detailed human-readable explanation.
    pub detail: String,
    /// URI of the specific occurrence.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub instance: Option<String>,
}

impl IntoResponse for ProblemDetails {
    fn into_response(self) -> Response {
        let status =
            StatusCode::from_u16(self.status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
        let mut response = axum::Json(self).into_response();
        *response.status_mut() = status;
        response
            .headers_mut()
            .insert("content-type", "application/problem+json".parse().unwrap());
        response
    }
}

impl ProblemDetails {
    /// 404 — resource not found.
    pub fn not_found(resource: &str, id: &str) -> Self {
        Self {
            problem_type: format!("urn:claude-pool:error:{resource}-not-found"),
            title: format!("{} not found", capitalize(resource)),
            status: 404,
            detail: format!("{} {id} does not exist or has been cleaned up", capitalize(resource)),
            instance: None,
        }
    }

    /// 409 — conflict (e.g. no slots available, wrong task state).
    pub fn conflict(detail: impl Into<String>) -> Self {
        Self {
            problem_type: "urn:claude-pool:error:conflict".to_string(),
            title: "Conflict".to_string(),
            status: 409,
            detail: detail.into(),
            instance: None,
        }
    }

    /// 400 — bad request.
    pub fn bad_request(detail: impl Into<String>) -> Self {
        Self {
            problem_type: "urn:claude-pool:error:bad-request".to_string(),
            title: "Bad request".to_string(),
            status: 400,
            detail: detail.into(),
            instance: None,
        }
    }

    /// 500 — internal server error.
    pub fn internal(detail: impl Into<String>) -> Self {
        Self {
            problem_type: "urn:claude-pool:error:internal".to_string(),
            title: "Internal server error".to_string(),
            status: 500,
            detail: detail.into(),
            instance: None,
        }
    }
}

/// Map a `claude_pool::Error` to a `ProblemDetails`.
impl From<claude_pool::Error> for ProblemDetails {
    fn from(err: claude_pool::Error) -> Self {
        use claude_pool::Error;
        match &err {
            Error::TaskNotFound(id) => ProblemDetails::not_found("task", id),
            Error::SlotNotFound(id) => ProblemDetails::not_found("slot", id),
            Error::NoSlotAvailable { timeout_secs } => ProblemDetails::conflict(format!(
                "No idle slot available after waiting {timeout_secs}s. Retry or scale up."
            )),
            Error::PoolShutdown => {
                ProblemDetails::conflict("Pool is shut down and no longer accepting work.")
            }
            Error::BudgetExhausted {
                spent_microdollars,
                limit_microdollars,
            } => ProblemDetails::conflict(format!(
                "Budget exhausted: spent ${:.2} of ${:.2} limit.",
                *spent_microdollars as f64 / 1_000_000.0,
                *limit_microdollars as f64 / 1_000_000.0
            )),
            _ => ProblemDetails::internal(err.to_string()),
        }
    }
}

fn capitalize(s: &str) -> String {
    let mut c = s.chars();
    match c.next() {
        None => String::new(),
        Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
    }
}
