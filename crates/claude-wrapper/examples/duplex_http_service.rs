//! Minimal HTTP "claude as a service": one `DuplexSession` behind two routes.
//!
//! Spawns a single [`DuplexSession`] on startup and serves it over HTTP via
//! `axum`. Concurrent requests serialize through a `tokio::sync::Mutex` so
//! they never overlap inside the wrapper (which would otherwise return
//! [`Error::DuplexTurnInFlight`](claude_wrapper::Error::DuplexTurnInFlight)
//! and surface as a 500). Cumulative cost and turn count are tracked by hand
//! in shared state.
//!
//! Routes:
//!
//! - `POST /chat` -- body `{"prompt": "..."}`, returns
//!   `{"text": "...", "session_id": "...", "turn_cost_usd": 0.0}`.
//! - `GET /health` -- returns
//!   `{"alive": true, "cumulative_cost_usd": 0.0, "turns": 0}`.
//!
//! Ctrl+C closes the session cleanly before shutting the server down.
//!
//! ```sh
//! cargo run --example duplex_http_service
//!
//! # in another terminal
//! curl -X POST http://127.0.0.1:3000/chat \
//!   -H 'content-type: application/json' \
//!   -d '{"prompt":"What is 2+2? Reply with just the number."}'
//! # => {"text":"4","session_id":"...","turn_cost_usd":0.0001}
//!
//! curl http://127.0.0.1:3000/health
//! # => {"alive":true,"cumulative_cost_usd":0.0001,"turns":1}
//! ```
//!
//! ## Intentional simplicity
//!
//! This is the canonical "DuplexSession over HTTP" reference: one session,
//! no pool, no auth. It also rolls its own bookkeeping by hand so it can
//! double as a before/after demo for the wrapper types that tighten this
//! shape:
//!
//! - The `cumulative_cost_usd` / `turns` mutexes collapse into
//!   [`Conversation`](claude_wrapper::conversation::Conversation), which
//!   already tracks both. Replacing `Arc<Mutex<DuplexSession>>` with
//!   `Arc<Mutex<Conversation>>` removes ~10 lines of state plumbing.
//! - The hardcoded `alive: true` is a placeholder for a future
//!   `session.is_alive()` health primitive (tracked in
//!   <https://github.com/joshrotenberg/claude-wrapper/issues/572>).

use std::sync::Arc;

use axum::{
    Json, Router,
    extract::State,
    http::StatusCode,
    response::IntoResponse,
    routing::{get, post},
};
use serde::{Deserialize, Serialize};
use tokio::sync::Mutex;

use claude_wrapper::Claude;
use claude_wrapper::duplex::{DuplexOptions, DuplexSession};

#[derive(Clone)]
struct AppState {
    session: Arc<Mutex<DuplexSession>>,
    cumulative_cost_usd: Arc<Mutex<f64>>,
    turns: Arc<Mutex<u32>>,
}

#[derive(Deserialize)]
struct ChatRequest {
    prompt: String,
}

#[derive(Serialize)]
struct ChatResponse {
    text: String,
    session_id: Option<String>,
    turn_cost_usd: f64,
}

#[derive(Serialize)]
struct HealthResponse {
    // TODO: replace with `session.is_alive()` once
    // https://github.com/joshrotenberg/claude-wrapper/issues/572 lands.
    alive: bool,
    cumulative_cost_usd: f64,
    turns: u32,
}

async fn chat(
    State(state): State<AppState>,
    Json(body): Json<ChatRequest>,
) -> Result<Json<ChatResponse>, (StatusCode, String)> {
    let session = state.session.lock().await;
    let turn = session
        .send(body.prompt)
        .await
        .map_err(|err| (StatusCode::INTERNAL_SERVER_ERROR, err.to_string()))?;

    let turn_cost = turn.total_cost_usd().unwrap_or(0.0);
    *state.cumulative_cost_usd.lock().await += turn_cost;
    *state.turns.lock().await += 1;

    Ok(Json(ChatResponse {
        text: turn.result_text().unwrap_or("").to_string(),
        session_id: turn.session_id().map(String::from),
        turn_cost_usd: turn_cost,
    }))
}

async fn health(State(state): State<AppState>) -> impl IntoResponse {
    Json(HealthResponse {
        alive: true,
        cumulative_cost_usd: *state.cumulative_cost_usd.lock().await,
        turns: *state.turns.lock().await,
    })
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let claude = Claude::builder().build()?;
    let session = DuplexSession::spawn(&claude, DuplexOptions::default().model("haiku")).await?;

    let state = AppState {
        session: Arc::new(Mutex::new(session)),
        cumulative_cost_usd: Arc::new(Mutex::new(0.0)),
        turns: Arc::new(Mutex::new(0)),
    };

    let app = Router::new()
        .route("/chat", post(chat))
        .route("/health", get(health))
        .with_state(state.clone());

    let addr = "127.0.0.1:3000";
    let listener = tokio::net::TcpListener::bind(addr).await?;
    println!("listening on http://{addr}");
    println!("  POST /chat   -- body: {{\"prompt\":\"...\"}}");
    println!("  GET  /health -- cumulative cost + turn count");
    println!("(Ctrl+C to shut down)");

    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await?;

    println!("closing duplex session...");
    let session = Arc::try_unwrap(state.session)
        .map_err(|_| anyhow::anyhow!("session still has outstanding references"))?
        .into_inner();
    session.close().await?;
    Ok(())
}

async fn shutdown_signal() {
    tokio::signal::ctrl_c()
        .await
        .expect("failed to install Ctrl+C handler");
}
