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

// ───────────────────────────────────────────────────────────────────
// Auth-free smoke tests: local CLI commands that make no model API
// call (plugin list, version parse, mcp add/get/remove round-trip).
// Fast and deterministic.
//
// NOTE: `claude doctor` and `claude marketplace list` are deliberately
// NOT live-tested -- both are interactive/TUI in claude 2.1.x and hang
// without a TTY, so a headless test can't drive them (a real caller
// must set a client timeout). Their arg construction is unit-tested.
// ───────────────────────────────────────────────────────────────────

#[tokio::test]
#[ignore = "requires claude binary"]
async fn test_plugin_list() {
    let claude = claude_client();
    let output = claude_wrapper::PluginListCommand::new()
        .execute(&claude)
        .await
        .unwrap();
    assert!(output.success, "plugin list failed: {}", output.stderr);
}

#[tokio::test]
#[ignore = "requires claude binary"]
async fn test_cli_version_parses() {
    let claude = claude_client();
    let v = claude.cli_version().await.unwrap();
    assert!(
        v.satisfies_minimum(&claude_wrapper::CliVersion::new(2, 0, 0)),
        "expected claude >= 2.0.0, parsed {v}"
    );
}

#[tokio::test]
#[ignore = "requires claude binary"]
async fn test_mcp_add_get_remove_roundtrip() {
    use claude_wrapper::{McpAddCommand, McpGetCommand, McpRemoveCommand, Scope};

    // Isolate in a temp working dir at local scope so the round-trip
    // never touches the caller's real MCP config.
    let dir = tempfile::tempdir().unwrap();
    let claude = Claude::builder()
        .working_dir(dir.path())
        .build()
        .expect("claude binary not found in PATH");

    let name = "claude_wrapper_itest_srv";

    let add = McpAddCommand::new(name, "echo")
        .scope(Scope::Local)
        .server_args(["hello"])
        .execute(&claude)
        .await
        .unwrap();
    assert!(add.success, "mcp add failed: {}", add.stderr);

    let get = McpGetCommand::new(name).execute(&claude).await.unwrap();
    assert!(get.success, "mcp get failed: {}", get.stderr);
    assert!(
        get.stdout.contains(name),
        "mcp get should show the added server, got: {}",
        get.stdout
    );

    let remove = McpRemoveCommand::new(name)
        .scope(Scope::Local)
        .execute(&claude)
        .await
        .unwrap();
    assert!(remove.success, "mcp remove failed: {}", remove.stderr);
}

// ───────────────────────────────────────────────────────────────────
// Streaming and the transient Session (the non-Duplex multi-turn
// path). Both had no live coverage before.
// ───────────────────────────────────────────────────────────────────

#[tokio::test]
#[ignore = "requires claude binary and auth"]
async fn test_stream_query_emits_result_event() {
    use claude_wrapper::OutputFormat;
    use claude_wrapper::streaming::{StreamEvent, stream_query};
    use std::sync::{Arc, Mutex};

    let claude = claude_client();
    let cmd = QueryCommand::new("Reply with just the number 5.")
        .model("haiku")
        .max_turns(1)
        .no_session_persistence()
        .permission_mode(claude_wrapper::PermissionMode::Plan)
        .output_format(OutputFormat::StreamJson);

    let events = Arc::new(Mutex::new(Vec::<StreamEvent>::new()));
    let sink = Arc::clone(&events);
    stream_query(&claude, &cmd, move |event: StreamEvent| {
        sink.lock().unwrap().push(event);
    })
    .await
    .expect("stream_query");

    let events = events.lock().unwrap();
    assert!(!events.is_empty(), "expected streamed NDJSON events");
    let result_text = events
        .iter()
        .find(|e| e.is_result())
        .and_then(|e| e.result_text())
        .expect("expected a terminal result event with text");
    assert!(
        result_text.contains('5'),
        "expected '5' in streamed result {result_text:?}"
    );
}

#[tokio::test]
#[ignore = "requires claude binary and auth"]
async fn test_session_two_turns_resume_and_track() {
    use claude_wrapper::session::Session;
    use std::sync::Arc;

    // No no_session_persistence(): a transient Session resumes via
    // --resume, which needs the session persisted to disk. No Plan
    // mode: it injects an ExitPlanMode tool turn that a tight max_turns
    // can't absorb (the Duplex recall test likewise runs without it).
    fn haiku(prompt: &str) -> QueryCommand {
        QueryCommand::new(prompt).model("haiku").max_turns(2)
    }

    let claude = Arc::new(claude_client());
    let mut session = Session::new(claude);

    // Two independent single-turn prompts. We assert the transient
    // Session's *mechanics* -- two turns dispatched (the second via
    // --resume), with history and cost accumulated -- rather than
    // conversational recall: in this environment a "remember this"
    // prompt triggers agentic tool use and overruns max_turns. Recall
    // across turns is covered by the Duplex test.
    let first = session
        .execute(haiku("What is 2+2? Reply with just the number."))
        .await
        .expect("first turn");
    let second = session
        .execute(haiku("What is 3+3? Reply with just the number."))
        .await
        .expect("second turn");

    assert!(first.result.contains('4'), "got {:?}", first.result);
    assert!(second.result.contains('6'), "got {:?}", second.result);
    assert!(
        !first.session_id.is_empty(),
        "first turn should report a session id"
    );
    assert!(
        !second.session_id.is_empty(),
        "second turn (--resume) should report a session id"
    );
    assert_eq!(session.history().len(), 2, "two turns recorded");
    assert!(
        session.total_cost_usd() > 0.0,
        "cost should accumulate, got {}",
        session.total_cost_usd()
    );
    assert!(session.id().is_some());
}

// ───────────────────────────────────────────────────────────────────
// QueryCommand option coverage: confirm flags are accepted by the
// real CLI and take effect end-to-end.
// ───────────────────────────────────────────────────────────────────

#[tokio::test]
#[ignore = "requires claude binary and auth"]
async fn test_query_system_prompt_is_honored() {
    let claude = claude_client();
    // A system prompt that forces an observable, content-independent
    // marker, so we verify the prompt actually reached the model.
    let output = QueryCommand::new("Say hello.")
        .model("haiku")
        .max_turns(1)
        .no_session_persistence()
        .permission_mode(claude_wrapper::PermissionMode::Plan)
        .system_prompt("End every response with the exact token MARKER42.")
        .execute(&claude)
        .await
        .unwrap();

    assert!(output.success, "stderr: {}", output.stderr);
    assert!(
        output.stdout.contains("MARKER42"),
        "system-prompt marker missing from reply: {}",
        output.stdout
    );
}

// NOTE: a live `--json-schema` test was intentionally dropped. In this
// environment structured output runs a multi-turn formatting pass
// (observed 4+ turns) that doesn't complete under a sane max_turns cap
// and is costly/non-deterministic. The flag's arg emission is covered
// by unit tests instead.

#[tokio::test]
#[ignore = "requires claude binary and auth"]
async fn test_query_options_accepted_together() {
    use claude_wrapper::ToolPattern;
    let claude = claude_client();
    // Several flags at once: confirm the wrapper's arg construction is
    // accepted by the real CLI and still returns a result.
    let output = QueryCommand::new("What is 2+2? Reply with just the number.")
        .model("haiku")
        .max_turns(1)
        .no_session_persistence()
        .permission_mode(claude_wrapper::PermissionMode::Plan)
        .append_system_prompt("Be extremely concise.")
        .allowed_tool(ToolPattern::tool("Read"))
        .max_budget_usd(1.0)
        .execute(&claude)
        .await
        .unwrap();

    assert!(output.success, "stderr: {}", output.stderr);
    assert!(output.stdout.contains('4'), "got: {}", output.stdout);
}

#[tokio::test]
#[ignore = "requires claude binary and auth"]
async fn test_query_effort_accepted() {
    use claude_wrapper::Effort;
    let claude = claude_client();
    let output = QueryCommand::new("What is 2+2? Reply with just the number.")
        .model("haiku")
        .max_turns(1)
        .no_session_persistence()
        .permission_mode(claude_wrapper::PermissionMode::Plan)
        .effort(Effort::Low)
        .execute(&claude)
        .await
        .unwrap();

    assert!(output.success, "stderr: {}", output.stderr);
    assert!(output.stdout.contains('4'), "got: {}", output.stdout);
}

// ───────────────────────────────────────────────────────────────────
// Flags added in the roba ergonomics batch (#629/#630/#601): confirm
// the live CLI accepts them, with observable effect where applicable.
// (--replay-user-messages is omitted: it needs a bidirectional
// stream-json stdin session, impractical to drive here, and its arg
// emission is covered by unit tests.)
// ───────────────────────────────────────────────────────────────────

#[tokio::test]
#[ignore = "requires claude binary and auth"]
async fn test_query_prompt_suggestions_accepted() {
    use claude_wrapper::OutputFormat;
    let claude = claude_client();
    // Confirm --prompt-suggestions is accepted on a stream-json print
    // query. The CLI documents a prompt_suggestion message in print/SDK
    // mode, but it is not reliably emitted in a headless one-shot run
    // (observed stream types: system/assistant/result only), so we
    // assert acceptance + a successful result rather than the message.
    let output = QueryCommand::new("Say hi.")
        .model("haiku")
        .max_turns(1)
        .no_session_persistence()
        .permission_mode(claude_wrapper::PermissionMode::Plan)
        .output_format(OutputFormat::StreamJson)
        .prompt_suggestions(true)
        .execute(&claude)
        .await
        .unwrap();

    assert!(output.success, "stderr: {}", output.stderr);
}

#[tokio::test]
#[ignore = "requires claude binary and auth"]
async fn test_query_verbose_accepted() {
    let claude = claude_client();
    // --verbose overrides the verbose-mode setting; confirm the CLI
    // accepts it on a print-mode query and still returns a result.
    let output = QueryCommand::new("What is 2+2? Reply with just the number.")
        .model("haiku")
        .max_turns(1)
        .no_session_persistence()
        .permission_mode(claude_wrapper::PermissionMode::Plan)
        .verbose(true)
        .execute(&claude)
        .await
        .unwrap();

    assert!(output.success, "stderr: {}", output.stderr);
    assert!(output.stdout.contains('4'), "got: {}", output.stdout);
}
