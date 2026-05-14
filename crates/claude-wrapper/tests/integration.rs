//! Integration tests that require a real `claude` binary in PATH.
//!
//! These tests are ignored by default. Run with:
//! ```sh
//! cargo test --test integration -- --ignored
//! ```

#![cfg(feature = "async")]

use claude_wrapper::{Claude, ClaudeCommand, QueryCommand, VersionCommand};

fn claude_client() -> Claude {
    Claude::builder()
        .build()
        .expect("claude binary not found in PATH")
}

#[tokio::test]
#[ignore = "requires claude binary"]
async fn test_version() {
    let claude = claude_client();
    let output = VersionCommand::new().execute(&claude).await.unwrap();
    assert!(output.success);
    assert!(!output.stdout.is_empty());
}

#[tokio::test]
#[ignore = "requires claude binary and auth"]
async fn test_auth_status() {
    let claude = claude_client();
    let status = claude_wrapper::AuthStatusCommand::new()
        .execute_json(&claude)
        .await
        .unwrap();
    assert!(status.logged_in);
}

#[tokio::test]
#[ignore = "requires claude binary and auth"]
async fn test_simple_query() {
    let claude = claude_client();
    let output = QueryCommand::new("What is 2+2? Reply with just the number.")
        .model("haiku")
        .max_turns(1)
        .no_session_persistence()
        .permission_mode(claude_wrapper::PermissionMode::Plan)
        .execute(&claude)
        .await
        .unwrap();

    assert!(output.success);
    assert!(output.stdout.contains('4'));
}

#[tokio::test]
#[ignore = "requires claude binary and auth"]
async fn test_query_json_output() {
    let claude = claude_client();
    let result = QueryCommand::new("What is 2+2? Reply with just the number.")
        .model("haiku")
        .max_turns(1)
        .no_session_persistence()
        .permission_mode(claude_wrapper::PermissionMode::Plan)
        .execute_json(&claude)
        .await
        .unwrap();

    assert!(!result.result.is_empty());
    assert!(!result.session_id.is_empty());
}

#[tokio::test]
#[ignore = "requires claude binary"]
async fn test_mcp_list() {
    let claude = claude_client();
    let output = claude_wrapper::McpListCommand::new()
        .execute(&claude)
        .await
        .unwrap();
    assert!(output.success);
}

#[tokio::test]
#[ignore = "requires claude binary and auth"]
async fn test_duplex_send_and_close() {
    use claude_wrapper::duplex::{DuplexOptions, DuplexSession};

    let claude = claude_client();
    let session = DuplexSession::spawn(&claude, DuplexOptions::default().model("haiku"))
        .await
        .expect("spawn duplex session");

    let turn = session
        .send("What is 2+2? Reply with just the number.")
        .await
        .expect("send turn");

    assert!(
        turn.session_id().is_some(),
        "result should carry a session_id"
    );
    let text = turn.result_text().expect("result text present");
    assert!(text.contains('4'), "expected '4' in {text:?}");

    session.close().await.expect("close session");
}

#[tokio::test]
#[ignore = "requires claude binary and auth"]
async fn test_duplex_two_turns_share_session() {
    use claude_wrapper::duplex::{DuplexOptions, DuplexSession};

    let claude = claude_client();
    let session = DuplexSession::spawn(&claude, DuplexOptions::default().model("haiku"))
        .await
        .expect("spawn duplex session");

    let first = session
        .send("Remember the number 7.")
        .await
        .expect("first turn");
    let second = session
        .send("What number did I ask you to remember? Reply with just the number.")
        .await
        .expect("second turn");

    let first_id = first.session_id().expect("first session_id");
    let second_id = second.session_id().expect("second session_id");
    assert_eq!(
        first_id, second_id,
        "session id should persist across turns"
    );

    let text = second.result_text().unwrap_or("");
    assert!(text.contains('7'), "expected '7' in {text:?}");

    session.close().await.expect("close session");
}

#[tokio::test]
#[ignore = "requires claude binary and auth"]
async fn test_duplex_subscribe_sees_assistant_before_result() {
    use claude_wrapper::duplex::{DuplexOptions, DuplexSession, InboundEvent};

    let claude = claude_client();
    let session = DuplexSession::spawn(&claude, DuplexOptions::default().model("haiku"))
        .await
        .expect("spawn duplex session");

    // Subscribe BEFORE sending so the broadcast buffer captures every
    // event for this turn.
    let mut rx = session.subscribe();

    let _turn = session
        .send("Reply with just the word 'hi'.")
        .await
        .expect("send turn");

    // Drain whatever the broadcast buffered while the turn was in
    // flight. Capacity is 256 by default; one turn never approaches
    // that, so try_recv until empty is fine.
    let mut received = Vec::new();
    while let Ok(event) = rx.try_recv() {
        received.push(event);
    }

    assert!(!received.is_empty(), "expected at least one inbound event");
    assert!(
        received
            .iter()
            .any(|e| matches!(e, InboundEvent::Assistant(_))),
        "expected at least one Assistant event in {received:#?}"
    );
    assert!(
        received
            .iter()
            .any(|e| matches!(e, InboundEvent::SystemInit { .. })),
        "expected a SystemInit event in {received:#?}"
    );

    session.close().await.expect("close session");
}

// Note on permission tests: in current claude (v2.1.x), the stream-json
// duplex mode does not reliably route tool-use prompts through
// --permission-prompt-tool stdio without an additional PreToolUse hook
// that keeps the stream open (per the official agent SDK docs). Our
// wire-level handling (parse control_request, write control_response)
// is exercised by unit tests in src/duplex.rs::tests. The live test
// here only verifies that a session configured with on_permission
// runs to completion without protocol corruption -- it does not
// assert the handler is invoked.
#[tokio::test]
#[ignore = "requires claude binary and auth"]
async fn test_duplex_permission_handler_does_not_break_session() {
    use claude_wrapper::duplex::{
        DuplexOptions, DuplexSession, PermissionDecision, PermissionHandler,
    };

    let handler = PermissionHandler::new(|_req| async move {
        PermissionDecision::Allow {
            updated_input: None,
        }
    });

    let claude = claude_client();
    let session = DuplexSession::spawn(
        &claude,
        DuplexOptions::default()
            .model("haiku")
            .on_permission(handler),
    )
    .await
    .expect("spawn duplex session");

    let turn = session
        .send("What is 2+2? Reply with just the number.")
        .await
        .expect("send turn");

    assert!(turn.session_id().is_some());
    session.close().await.expect("close session");
}

#[tokio::test]
#[ignore = "requires claude binary and auth"]
async fn test_duplex_interrupt_closes_in_flight_turn() {
    use claude_wrapper::duplex::{DuplexOptions, DuplexSession};
    use std::time::Duration;

    let claude = claude_client();
    let session = DuplexSession::spawn(&claude, DuplexOptions::default().model("haiku"))
        .await
        .expect("spawn duplex session");

    // Drive a slow-ish turn alongside an interrupt issued shortly
    // after. Cap the join with an outer timeout so a misbehaving
    // interrupt never hangs the test runner.
    let outcome = tokio::time::timeout(Duration::from_secs(60), async {
        let send_fut = session.send(
            "Write a long, detailed essay about the history of \
             programming languages, covering at least 20 distinct \
             languages chronologically.",
        );
        let interrupt_fut = async {
            tokio::time::sleep(Duration::from_millis(500)).await;
            session.interrupt().await
        };
        tokio::join!(send_fut, interrupt_fut)
    })
    .await
    .expect("turn + interrupt should resolve within 60s");

    let (turn, interrupt_result) = outcome;
    let turn = turn.expect("send turn resolved");
    interrupt_result.expect("interrupt acknowledged");

    // The interrupted turn closes with one of several signals: an
    // explicit `stop_reason`, `is_error: true`, `subtype:
    // "error_during_execution"`, or `terminal_reason:
    // "aborted_streaming"`. Don't pin to one -- just assert the turn
    // returned and shows at least one of those interrupt indicators.
    let result = &turn.result;
    let interrupted = result
        .get("is_error")
        .and_then(|v| v.as_bool())
        .unwrap_or(false)
        || result.get("subtype").and_then(|v| v.as_str()) == Some("error_during_execution")
        || result.get("terminal_reason").and_then(|v| v.as_str()) == Some("aborted_streaming")
        || result
            .get("stop_reason")
            .and_then(|v| v.as_str())
            .is_some_and(|s| s != "end_turn");
    assert!(
        interrupted,
        "expected an interrupt indicator on the truncated turn: {result:?}"
    );

    session.close().await.expect("close session");
}

#[tokio::test]
#[ignore = "requires claude binary and auth"]
async fn test_duplex_three_turns_subscribe_drains() {
    use claude_wrapper::duplex::{DuplexOptions, DuplexSession, InboundEvent};

    let claude = claude_client();
    let session = DuplexSession::spawn(&claude, DuplexOptions::default().model("haiku"))
        .await
        .expect("spawn duplex session");
    let mut rx = session.subscribe();

    let prompts = [
        "Reply with just the number 1.",
        "Reply with just the number 2.",
        "Reply with just the number 3.",
    ];

    let mut session_ids = Vec::new();
    for p in prompts {
        let turn = session.send(p).await.expect("send turn");
        session_ids.push(turn.session_id().expect("session id").to_string());
    }

    // All three turns should share the same session id.
    assert_eq!(
        session_ids[0], session_ids[1],
        "session id stable across turns"
    );
    assert_eq!(session_ids[1], session_ids[2]);

    // Subscribers see at least one Assistant per turn and at least
    // one SystemInit. Each turn currently emits its own SystemInit,
    // all carrying the same session_id; assert all observed
    // SystemInit ids match.
    let mut received: Vec<InboundEvent> = Vec::new();
    while let Ok(ev) = rx.try_recv() {
        received.push(ev);
    }
    let init_ids: Vec<&str> = received
        .iter()
        .filter_map(|e| match e {
            InboundEvent::SystemInit { session_id } => Some(session_id.as_str()),
            _ => None,
        })
        .collect();
    let assistant_count = received
        .iter()
        .filter(|e| matches!(e, InboundEvent::Assistant(_)))
        .count();
    assert!(!init_ids.is_empty(), "expected at least one SystemInit");
    assert!(
        init_ids.iter().all(|id| *id == init_ids[0]),
        "all SystemInit events should carry the same session_id, got {init_ids:?}"
    );
    assert_eq!(
        init_ids[0], session_ids[0],
        "SystemInit session_id should match the turn's result session_id"
    );
    assert!(
        assistant_count >= 3,
        "expected at least one Assistant per turn, got {assistant_count}"
    );

    session.close().await.expect("close session");
}

#[tokio::test]
#[ignore = "requires claude binary and auth"]
async fn test_duplex_multiple_subscribers_see_same_events() {
    use claude_wrapper::duplex::{DuplexOptions, DuplexSession, InboundEvent};

    let claude = claude_client();
    let session = DuplexSession::spawn(&claude, DuplexOptions::default().model("haiku"))
        .await
        .expect("spawn duplex session");

    let mut rx_a = session.subscribe();
    let mut rx_b = session.subscribe();
    let mut rx_c = session.subscribe();

    let _turn = session
        .send("Reply with just the word 'hi'.")
        .await
        .expect("send turn");

    fn drain(rx: &mut tokio::sync::broadcast::Receiver<InboundEvent>) -> usize {
        let mut n = 0;
        while rx.try_recv().is_ok() {
            n += 1;
        }
        n
    }

    let n_a = drain(&mut rx_a);
    let n_b = drain(&mut rx_b);
    let n_c = drain(&mut rx_c);

    assert!(n_a > 0, "subscriber a saw at least one event");
    assert_eq!(n_a, n_b, "subscribers a and b saw the same count");
    assert_eq!(n_b, n_c, "subscribers b and c saw the same count");

    session.close().await.expect("close session");
}

#[tokio::test]
#[ignore = "requires claude binary and auth"]
async fn test_duplex_slow_subscriber_lags_without_blocking() {
    use claude_wrapper::duplex::{DuplexOptions, DuplexSession};
    use tokio::sync::broadcast::error::TryRecvError;

    let claude = claude_client();
    // Tiny capacity so a single turn overruns the buffer for a
    // subscriber that never drains.
    let session = DuplexSession::spawn(
        &claude,
        DuplexOptions::default()
            .model("haiku")
            .subscriber_capacity(1),
    )
    .await
    .expect("spawn duplex session");

    let mut rx = session.subscribe();

    // Drive a turn that emits several events. The subscriber doesn't
    // drain until after the turn completes.
    let _turn = session
        .send("Reply with just the word 'hello'.")
        .await
        .expect("send turn");

    // First try_recv on a lagged subscriber returns Lagged with a
    // skip count; subsequent calls return the surviving events.
    let mut saw_lagged = false;
    loop {
        match rx.try_recv() {
            Ok(_) => continue,
            Err(TryRecvError::Lagged(n)) => {
                assert!(n > 0, "Lagged should report a non-zero skip");
                saw_lagged = true;
            }
            Err(TryRecvError::Empty) => break,
            Err(TryRecvError::Closed) => break,
        }
    }
    assert!(
        saw_lagged,
        "expected a Lagged signal when capacity=1 and subscriber doesn't drain"
    );

    session.close().await.expect("close session");
}

#[tokio::test]
#[ignore = "requires claude binary and auth"]
async fn test_duplex_send_while_turn_in_flight_returns_error() {
    use claude_wrapper::Error;
    use claude_wrapper::duplex::{DuplexOptions, DuplexSession};

    let claude = claude_client();
    let session = DuplexSession::spawn(&claude, DuplexOptions::default().model("haiku"))
        .await
        .expect("spawn duplex session");

    // Drive a first turn forward enough that its OutboundMsg::Send
    // reaches the session task and `pending` is set, then issue a
    // second send while the first is still pending. Scope the
    // pinned `first` future inside a block so its borrow of
    // `session` ends before we call `close()` below.
    {
        let first = session.send("Reply with just the word 'first'.");
        tokio::pin!(first);

        tokio::select! {
            biased;
            res = &mut first => panic!("first turn finished too fast: {res:?}"),
            _ = tokio::time::sleep(std::time::Duration::from_millis(150)) => {}
        }

        let second = session.send("hi").await;
        match second {
            Err(Error::DuplexTurnInFlight) => {}
            other => panic!("expected DuplexTurnInFlight, got {other:?}"),
        }

        let _first_turn = (&mut first).await.expect("first turn completed");
    }

    session.close().await.expect("close session");
}

#[tokio::test]
#[ignore = "requires claude binary and auth"]
async fn test_conversation_three_turns_track_history_and_cost() {
    use claude_wrapper::Conversation;
    use claude_wrapper::duplex::{DuplexOptions, DuplexSession};

    let claude = claude_client();
    let session = DuplexSession::spawn(&claude, DuplexOptions::default().model("haiku"))
        .await
        .expect("spawn duplex session");

    let mut conv = Conversation::new(session);

    let prompts = [
        "Reply with just the number 1.",
        "Reply with just the number 2.",
        "Reply with just the number 3.",
    ];

    let mut session_ids = Vec::new();
    for p in prompts {
        let turn = conv.send(p).await.expect("send turn");
        session_ids.push(turn.session_id().expect("session id").to_string());
    }

    assert_eq!(conv.history().len(), 3, "history should record three turns");
    assert_eq!(conv.total_turns(), 3, "total_turns should match send count");
    assert!(
        conv.total_cost_usd() > 0.0,
        "total_cost_usd should accumulate non-zero across three live turns, got {}",
        conv.total_cost_usd()
    );

    // All three turns share the same session id (single duplex
    // child), and Conversation::session_id reports the most recent.
    assert_eq!(session_ids[0], session_ids[1]);
    assert_eq!(session_ids[1], session_ids[2]);
    assert_eq!(conv.session_id(), Some(session_ids[2].as_str()));

    conv.close().await.expect("close conversation");
}

#[tokio::test]
#[ignore = "requires claude binary and auth"]
async fn test_duplex_health_primitives_lifecycle() {
    use claude_wrapper::duplex::{DuplexOptions, DuplexSession, SessionExitStatus};

    let claude = claude_client();
    let session = DuplexSession::spawn(&claude, DuplexOptions::default().model("haiku"))
        .await
        .expect("spawn duplex session");

    assert!(session.is_alive(), "session should be alive after spawn");
    assert!(
        matches!(session.exit_status(), SessionExitStatus::Running),
        "exit_status should be Running after spawn"
    );

    let _turn = session
        .send("Reply with just the word 'hi'.")
        .await
        .expect("send turn");

    assert!(
        session.is_alive(),
        "session should still be alive after a successful turn"
    );

    session.close().await.expect("close session");
    // close(self) consumes the session, so we cannot observe
    // is_alive()/wait_for_exit() through the same handle afterwards.
    // The post-exit watch transition is exercised by unit tests in
    // src/duplex.rs::tests against the same state machine.
}

#[tokio::test]
#[ignore = "requires claude binary"]
async fn test_mcp_config_builder() {
    use claude_wrapper::McpConfigBuilder;

    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join(".mcp.json");

    let written_path = McpConfigBuilder::new()
        .http_server("test-hub", "http://127.0.0.1:9090")
        .stdio_server("test-tool", "echo", ["hello"])
        .write_to(&path)
        .unwrap();

    assert_eq!(written_path, path);

    let contents = std::fs::read_to_string(&path).unwrap();
    let parsed: serde_json::Value = serde_json::from_str(&contents).unwrap();

    assert!(
        parsed["mcpServers"]["test-hub"]["url"]
            .as_str()
            .unwrap()
            .contains("9090")
    );
    assert_eq!(
        parsed["mcpServers"]["test-tool"]["command"]
            .as_str()
            .unwrap(),
        "echo"
    );
}
