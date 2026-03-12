//! Integration tests for the REST API.
//!
//! Uses `tower::ServiceExt::oneshot` to drive the axum router directly,
//! without binding a TCP listener. Tests cover endpoint behavior, auth,
//! error responses, and query parameter filtering.

#![cfg(feature = "rest")]

use std::path::PathBuf;
use std::sync::Arc;

use axum::body::Body;
use axum::http::{Method, Request, StatusCode};
use claude_pool::{
    InMemoryStore, Pool, PoolConfig, ScalingConfig, SkillRegistry, WorkflowRegistry,
};
use claude_pool_server::auth::BearerTokens;
use claude_pool_server::rest::{RestConfig, router};
use claude_pool_server::{ServerInfo, State};
use http_body_util::BodyExt;
use serde_json::{Value, json};
use tokio::sync::RwLock;
use tower::ServiceExt;

const FAKE_CLAUDE: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../test-helpers/fake-claude.sh"
);

/// Build a test `State` with the given slot count.
async fn test_state(slots: usize) -> Arc<State<InMemoryStore>> {
    let claude = claude_wrapper::Claude::builder()
        .binary(PathBuf::from(FAKE_CLAUDE))
        .build()
        .expect("failed to build Claude client");

    let config = PoolConfig {
        scaling: ScalingConfig {
            min_slots: 1,
            max_slots: 16,
        },
        ..Default::default()
    };

    let pool = Pool::builder_with_store(claude, InMemoryStore::new())
        .slots(slots)
        .config(config)
        .build()
        .await
        .expect("failed to build pool");

    Arc::new(State {
        pool,
        skills: Arc::new(RwLock::new(SkillRegistry::with_builtins())),
        workflows: WorkflowRegistry::with_builtins(),
        skills_dir: PathBuf::from(".claude-pool/skills"),
        server_info: ServerInfo::new(None, "plan".to_string(), slots),
    })
}

/// Build the REST router with no auth.
async fn test_router(slots: usize) -> axum::Router {
    let state = test_state(slots).await;
    router(state, RestConfig::default())
}

/// Build the REST router with auth enabled.
async fn test_router_with_auth(slots: usize, tokens: Vec<String>) -> axum::Router {
    let state = test_state(slots).await;
    router(
        state,
        RestConfig {
            tokens: BearerTokens::new(tokens),
            ..Default::default()
        },
    )
}

/// Helper: send a request and return (status, body as Value).
async fn send(router: axum::Router, req: Request<Body>) -> (StatusCode, Value) {
    let response = router.oneshot(req).await.expect("request failed");
    let status = response.status();
    let body = response
        .into_body()
        .collect()
        .await
        .expect("failed to read body")
        .to_bytes();
    let value: Value = if body.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&body)
            .unwrap_or(Value::String(String::from_utf8_lossy(&body).into()))
    };
    (status, value)
}

/// Helper: build a JSON POST request.
fn post_json(uri: &str, body: Value) -> Request<Body> {
    Request::builder()
        .method(Method::POST)
        .uri(uri)
        .header("content-type", "application/json")
        .body(Body::from(serde_json::to_vec(&body).unwrap()))
        .unwrap()
}

/// Helper: build a JSON PUT request.
fn put_json(uri: &str, body: Value) -> Request<Body> {
    Request::builder()
        .method(Method::PUT)
        .uri(uri)
        .header("content-type", "application/json")
        .body(Body::from(serde_json::to_vec(&body).unwrap()))
        .unwrap()
}

/// Helper: build a GET request.
fn get(uri: &str) -> Request<Body> {
    Request::builder()
        .method(Method::GET)
        .uri(uri)
        .body(Body::empty())
        .unwrap()
}

/// Helper: build a DELETE request.
fn delete(uri: &str) -> Request<Body> {
    Request::builder()
        .method(Method::DELETE)
        .uri(uri)
        .body(Body::empty())
        .unwrap()
}

/// Helper: build a GET request with auth header.
fn get_with_auth(uri: &str, token: &str) -> Request<Body> {
    Request::builder()
        .method(Method::GET)
        .uri(uri)
        .header("authorization", format!("Bearer {token}"))
        .body(Body::empty())
        .unwrap()
}

// ── Health ───────────────────────────────────────────────────────────────────

#[tokio::test]
async fn health_returns_ok() {
    let app = test_router(1).await;
    let (status, body) = send(app, get("/health")).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, Value::String("ok".into()));
}

// ── Pool Status ──────────────────────────────────────────────────────────────

#[tokio::test]
async fn pool_status_returns_slot_info() {
    let app = test_router(2).await;
    let (status, body) = send(app, get("/v1/pool/status")).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["total_slots"], 2);
    assert!(body["idle_slots"].is_number());
    assert_eq!(body["shutdown"], false);
    assert!(body["server_version"].is_string());
}

// ── Tasks ────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn list_tasks_empty() {
    let app = test_router(1).await;
    let (status, body) = send(app, get("/v1/tasks")).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, json!([]));
}

#[tokio::test]
async fn submit_and_get_task() {
    let app = test_router(1).await;

    // Submit a task.
    let (status, body) = send(
        app.clone(),
        post_json("/v1/tasks", json!({"prompt": "hello world"})),
    )
    .await;
    assert_eq!(status, StatusCode::ACCEPTED);
    let task_id = body["task_id"].as_str().unwrap().to_string();
    assert_eq!(body["state"], "pending");

    // Get the task.
    let (status, body) = send(app, get(&format!("/v1/tasks/{task_id}"))).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["task_id"], task_id);
}

#[tokio::test]
async fn get_nonexistent_task_returns_404() {
    let app = test_router(1).await;
    let (status, body) = send(app, get("/v1/tasks/nonexistent")).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert!(body["type"].as_str().unwrap().contains("not-found"));
}

#[tokio::test]
async fn cancel_task() {
    let app = test_router(1).await;

    let (_, body) = send(
        app.clone(),
        post_json("/v1/tasks", json!({"prompt": "test cancel"})),
    )
    .await;
    let task_id = body["task_id"].as_str().unwrap().to_string();

    let (status, _) = send(app, delete(&format!("/v1/tasks/{task_id}"))).await;
    assert_eq!(status, StatusCode::NO_CONTENT);
}

#[tokio::test]
async fn fan_out_empty_prompts_rejected() {
    let app = test_router(1).await;
    let (status, body) = send(app, post_json("/v1/tasks/fan-out", json!({"prompts": []}))).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(body["type"].as_str().unwrap().contains("bad-request"));
}

// ── Chains ───────────────────────────────────────────────────────────────────

#[tokio::test]
async fn list_chains_empty() {
    let app = test_router(1).await;
    let (status, body) = send(app, get("/v1/chains")).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, json!([]));
}

#[tokio::test]
async fn submit_chain_empty_steps_rejected() {
    let app = test_router(1).await;
    let (status, body) = send(app, post_json("/v1/chains", json!({"steps": []}))).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(body["type"].as_str().unwrap().contains("bad-request"));
}

#[tokio::test]
async fn submit_and_get_chain() {
    let app = test_router(1).await;

    let (status, body) = send(
        app.clone(),
        post_json(
            "/v1/chains",
            json!({
                "steps": [
                    {"name": "step1", "prompt": "do thing"},
                    {"name": "step2", "prompt": "do other thing"}
                ]
            }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::ACCEPTED);
    assert_eq!(body["total_steps"], 2);
    let chain_id = body["chain_id"].as_str().unwrap().to_string();

    let (status, body) = send(app, get(&format!("/v1/chains/{chain_id}"))).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["chain_id"], chain_id);
    assert_eq!(body["total_steps"], 2);
}

// ── Skills ───────────────────────────────────────────────────────────────────

#[tokio::test]
async fn list_skills_includes_builtins() {
    let app = test_router(1).await;
    let (status, body) = send(app, get("/v1/skills")).await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        !body.as_array().unwrap().is_empty(),
        "should have builtin skills"
    );
}

#[tokio::test]
async fn register_and_get_skill() {
    let app = test_router(1).await;

    let (status, body) = send(
        app.clone(),
        post_json(
            "/v1/skills",
            json!({
                "name": "test_skill",
                "description": "a test skill",
                "prompt": "do the thing",
                "scope": "task"
            }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    assert_eq!(body["name"], "test_skill");
    assert_eq!(body["source"], "runtime");

    let (status, body) = send(app, get("/v1/skills/test_skill")).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["name"], "test_skill");
}

#[tokio::test]
async fn get_nonexistent_skill_returns_404() {
    let app = test_router(1).await;
    let (status, _) = send(app, get("/v1/skills/nope")).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn register_skill_invalid_scope() {
    let app = test_router(1).await;
    let (status, body) = send(
        app,
        post_json(
            "/v1/skills",
            json!({
                "name": "bad",
                "description": "bad scope",
                "prompt": "x",
                "scope": "invalid"
            }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(body["type"].as_str().unwrap().contains("bad-request"));
}

#[tokio::test]
async fn remove_skill() {
    let app = test_router(1).await;

    send(
        app.clone(),
        post_json(
            "/v1/skills",
            json!({
                "name": "to_delete",
                "description": "ephemeral",
                "prompt": "x"
            }),
        ),
    )
    .await;

    let (status, _) = send(app.clone(), delete("/v1/skills/to_delete")).await;
    assert_eq!(status, StatusCode::NO_CONTENT);

    let (status, _) = send(app, get("/v1/skills/to_delete")).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

// ── Context ──────────────────────────────────────────────────────────────────

#[tokio::test]
async fn context_crud() {
    let app = test_router(1).await;

    // List empty.
    let (status, body) = send(app.clone(), get("/v1/context")).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, json!([]));

    // Set a value.
    let (status, _) = send(
        app.clone(),
        put_json("/v1/context/project", json!({"value": "claude-wrapper"})),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);

    // Get the value.
    let (status, body) = send(app.clone(), get("/v1/context/project")).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["key"], "project");
    assert_eq!(body["value"], "claude-wrapper");

    // List has one entry.
    let (status, body) = send(app.clone(), get("/v1/context")).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body.as_array().unwrap().len(), 1);

    // Delete.
    let (status, _) = send(app.clone(), delete("/v1/context/project")).await;
    assert_eq!(status, StatusCode::NO_CONTENT);

    // Gone.
    let (status, _) = send(app, get("/v1/context/project")).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn get_nonexistent_context_returns_404() {
    let app = test_router(1).await;
    let (status, _) = send(app, get("/v1/context/nope")).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

// ── Slots ────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn list_slots_returns_expected_count() {
    let app = test_router(3).await;
    let (status, body) = send(app, get("/v1/slots")).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body.as_array().unwrap().len(), 3);
}

#[tokio::test]
async fn list_slots_filter_by_state() {
    let app = test_router(2).await;
    let (status, body) = send(app, get("/v1/slots?state=idle")).await;
    assert_eq!(status, StatusCode::OK);
    // All slots should be idle at startup.
    assert_eq!(body.as_array().unwrap().len(), 2);
}

#[tokio::test]
async fn get_nonexistent_slot_returns_404() {
    let app = test_router(1).await;
    let (status, _) = send(app, get("/v1/slots/nonexistent")).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

// ── Webhooks ─────────────────────────────────────────────────────────────────

#[tokio::test]
async fn webhook_crud() {
    let app = test_router(1).await;

    // List empty.
    let (status, body) = send(app.clone(), get("/v1/webhooks")).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, json!([]));

    // Register.
    let (status, body) = send(
        app.clone(),
        post_json(
            "/v1/webhooks",
            json!({"url": "http://localhost:9999/hook", "events": ["task_completed"]}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let webhook_id = body["id"].as_str().unwrap().to_string();
    assert!(webhook_id.starts_with("wh_"));

    // List has one.
    let (status, body) = send(app.clone(), get("/v1/webhooks")).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body.as_array().unwrap().len(), 1);

    // Remove.
    let (status, _) = send(app.clone(), delete(&format!("/v1/webhooks/{webhook_id}"))).await;
    assert_eq!(status, StatusCode::NO_CONTENT);

    // List empty again.
    let (status, body) = send(app, get("/v1/webhooks")).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, json!([]));
}

#[tokio::test]
async fn webhook_https_rejected() {
    let app = test_router(1).await;
    let (status, body) = send(
        app,
        post_json("/v1/webhooks", json!({"url": "https://example.com/hook"})),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(body["type"].as_str().unwrap().contains("bad-request"));
}

#[tokio::test]
async fn remove_nonexistent_webhook_returns_404() {
    let app = test_router(1).await;
    let (status, _) = send(app, delete("/v1/webhooks/wh_nonexistent")).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

// ── Pool Scale ───────────────────────────────────────────────────────────────

#[tokio::test]
async fn scale_with_target() {
    let app = test_router(2).await;
    let (status, body) = send(app, post_json("/v1/pool/scale", json!({"target": 4}))).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["previous_slots"], 2);
    assert_eq!(body["current_slots"], 4);
}

#[tokio::test]
async fn scale_with_delta() {
    let app = test_router(2).await;
    let (status, body) = send(app, post_json("/v1/pool/scale", json!({"delta": 1}))).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["previous_slots"], 2);
    assert_eq!(body["current_slots"], 3);
}

#[tokio::test]
async fn scale_both_target_and_delta_rejected() {
    let app = test_router(1).await;
    let (status, body) = send(
        app,
        post_json("/v1/pool/scale", json!({"target": 3, "delta": 1})),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(body["type"].as_str().unwrap().contains("bad-request"));
}

#[tokio::test]
async fn scale_neither_target_nor_delta_rejected() {
    let app = test_router(1).await;
    let (status, body) = send(app, post_json("/v1/pool/scale", json!({}))).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(body["type"].as_str().unwrap().contains("bad-request"));
}

// ── Auth ─────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn auth_rejects_without_token() {
    let app = test_router_with_auth(1, vec!["sk-test-123".into()]).await;
    let (status, _) = send(app, get("/v1/pool/status")).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn auth_rejects_wrong_token() {
    let app = test_router_with_auth(1, vec!["sk-test-123".into()]).await;
    let (status, _) = send(app, get_with_auth("/v1/pool/status", "sk-wrong")).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn auth_accepts_valid_token() {
    let app = test_router_with_auth(1, vec!["sk-test-123".into()]).await;
    let (status, body) = send(app, get_with_auth("/v1/pool/status", "sk-test-123")).await;
    assert_eq!(status, StatusCode::OK);
    assert!(body["total_slots"].is_number());
}

#[tokio::test]
async fn auth_health_exempt() {
    let app = test_router_with_auth(1, vec!["sk-test-123".into()]).await;
    let (status, _) = send(app, get("/health")).await;
    assert_eq!(status, StatusCode::OK);
}
