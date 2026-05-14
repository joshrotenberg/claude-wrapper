//! HTTP transport smoke tests.
//!
//! These drive the axum `Router` returned by
//! [`tower_mcp::HttpTransport::into_router`] in-process via
//! [`tower::ServiceExt::oneshot`] -- no socket, no port, no listener.
//! That keeps the test fast and CI-friendly while exercising the
//! exact same code path real callers would hit over HTTP.
//!
//! No live `claude` invocations here -- the transport is the unit
//! under test, not the agent.

use axum::body::Body;
use axum::http::{Method, Request, StatusCode, header};
use claude_server::{ServerConfig, build_router};
use http_body_util::BodyExt;
use serde_json::Value;
use tower::ServiceExt;
use tower_mcp::HttpTransport;

fn cfg() -> ServerConfig {
    ServerConfig::default()
}

async fn read_body(resp: axum::http::Response<Body>) -> Value {
    let bytes = resp
        .into_body()
        .collect()
        .await
        .expect("collect body")
        .to_bytes();
    serde_json::from_slice(&bytes).expect("body is JSON")
}

fn jsonrpc_request(method: &str, id: u64, params: Value) -> Value {
    serde_json::json!({
        "jsonrpc": "2.0",
        "id": id,
        "method": method,
        "params": params,
    })
}

async fn post_jsonrpc(
    app: &mut axum::Router,
    body: Value,
    extra_headers: &[(&str, &str)],
) -> axum::http::Response<Body> {
    let mut req = Request::builder()
        .method(Method::POST)
        .uri("/")
        .header(header::CONTENT_TYPE, "application/json")
        .header(header::ACCEPT, "application/json, text/event-stream");
    for (k, v) in extra_headers {
        req = req.header(*k, *v);
    }
    let req = req.body(Body::from(body.to_string())).expect("build req");
    app.clone().oneshot(req).await.expect("oneshot")
}

#[tokio::test]
async fn http_initialize_then_tools_list_returns_known_tools() {
    let router = build_router(cfg()).expect("router built");
    let mut app = HttpTransport::new(router).into_router();

    // initialize
    let init_body = jsonrpc_request(
        "initialize",
        1,
        serde_json::json!({
            "protocolVersion": "2025-11-25",
            "capabilities": {},
            "clientInfo": {"name": "http-test", "version": "0.0.1"},
        }),
    );
    let resp = post_jsonrpc(&mut app, init_body, &[]).await;
    assert_eq!(resp.status(), StatusCode::OK, "initialize status");
    // Capture the session id the transport assigns so subsequent
    // requests are routed to the same session.
    let session_id = resp
        .headers()
        .get("mcp-session-id")
        .map(|v| v.to_str().expect("ascii").to_string());

    let v = read_body(resp).await;
    assert!(v["result"]["protocolVersion"].as_str().is_some());

    // initialized notification (required by the lifecycle)
    let initialized = serde_json::json!({
        "jsonrpc": "2.0",
        "method": "notifications/initialized",
    });
    let mut headers: Vec<(&str, &str)> = vec![];
    if let Some(ref sid) = session_id {
        headers.push(("mcp-session-id", sid));
    }
    let _ = post_jsonrpc(&mut app, initialized, &headers).await;

    // tools/list
    let list_body = jsonrpc_request("tools/list", 2, serde_json::json!({}));
    let resp = post_jsonrpc(&mut app, list_body, &headers).await;
    assert_eq!(resp.status(), StatusCode::OK, "tools/list status");
    let v = read_body(resp).await;
    let tools = v["result"]["tools"].as_array().expect("tools array");
    let names: Vec<&str> = tools.iter().filter_map(|t| t["name"].as_str()).collect();
    for expected in [
        "claude_version",
        "claude_query",
        "chat_open",
        "chat_send",
        "turn_get",
        "turn_wait",
    ] {
        assert!(names.contains(&expected), "missing {expected} in {names:?}");
    }
}

#[tokio::test]
async fn http_bearer_layer_rejects_missing_token() {
    use axum::middleware;
    let router = build_router(cfg()).expect("router built");
    let mut app = HttpTransport::new(router).into_router();
    async fn guard(
        req: axum::extract::Request,
        next: axum::middleware::Next,
    ) -> Result<axum::response::Response, axum::http::StatusCode> {
        let header = req
            .headers()
            .get(axum::http::header::AUTHORIZATION)
            .and_then(|v| v.to_str().ok())
            .and_then(|s| s.strip_prefix("Bearer "));
        match header {
            Some("secret") => Ok(next.run(req).await),
            _ => Err(axum::http::StatusCode::UNAUTHORIZED),
        }
    }
    app = app.layer(middleware::from_fn(guard));

    // No auth header: 401.
    let init_no_auth = jsonrpc_request(
        "initialize",
        1,
        serde_json::json!({"protocolVersion": "2025-11-25", "capabilities": {}, "clientInfo": {"name": "x", "version": "0"}}),
    );
    let resp = post_jsonrpc(&mut app, init_no_auth.clone(), &[]).await;
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED, "no token => 401");

    // Wrong token: 401.
    let resp = post_jsonrpc(
        &mut app,
        init_no_auth.clone(),
        &[("authorization", "Bearer wrong")],
    )
    .await;
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED, "bad token => 401");

    // Correct token: 200.
    let resp = post_jsonrpc(
        &mut app,
        init_no_auth,
        &[("authorization", "Bearer secret")],
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK, "good token => 200");
}

#[tokio::test]
async fn http_call_tool_claude_version_returns_envelope() {
    let router = build_router(cfg()).expect("router built");
    let mut app = HttpTransport::new(router).into_router();

    // initialize
    let init = jsonrpc_request(
        "initialize",
        1,
        serde_json::json!({
            "protocolVersion": "2025-11-25",
            "capabilities": {},
            "clientInfo": {"name": "http-test", "version": "0.0.1"},
        }),
    );
    let resp = post_jsonrpc(&mut app, init, &[]).await;
    let session_id = resp
        .headers()
        .get("mcp-session-id")
        .map(|v| v.to_str().expect("ascii").to_string())
        .expect("session id assigned");
    let _ = read_body(resp).await;
    let _ = post_jsonrpc(
        &mut app,
        serde_json::json!({"jsonrpc": "2.0", "method": "notifications/initialized"}),
        &[("mcp-session-id", &session_id)],
    )
    .await;

    // tools/call
    let body = jsonrpc_request(
        "tools/call",
        3,
        serde_json::json!({"name": "claude_version", "arguments": {}}),
    );
    let resp = post_jsonrpc(&mut app, body, &[("mcp-session-id", &session_id)]).await;
    assert_eq!(resp.status(), StatusCode::OK);
    let v = read_body(resp).await;
    let content = v["result"]["content"].as_array().expect("content");
    let text = content
        .iter()
        .filter_map(|c| c["text"].as_str())
        .collect::<String>();
    assert!(text.contains("claude-server"), "body text: {text}");
}
