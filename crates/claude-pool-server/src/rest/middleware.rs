//! REST API middleware: bearer auth and rate limiting.

use axum::extract::Request;
use axum::http::StatusCode;
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};

use crate::auth::BearerTokens;

/// Bearer token authentication middleware for the REST API.
///
/// Exempts `/health` from authentication. All other requests must include
/// a valid `Authorization: Bearer <token>` header.
pub async fn bearer_auth(req: Request, next: Next, tokens: BearerTokens) -> Response {
    // Allow unauthenticated access to health endpoint.
    if req.uri().path() == "/health" {
        return next.run(req).await;
    }

    let authorized = req
        .headers()
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .is_some_and(|token| tokens.validate(token));

    if authorized {
        next.run(req).await
    } else {
        StatusCode::UNAUTHORIZED.into_response()
    }
}
