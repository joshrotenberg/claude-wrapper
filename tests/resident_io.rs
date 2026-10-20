//! Controlled child-process checks for resident I/O and exit evidence.
#![cfg(all(feature = "async", feature = "json", unix))]

use std::os::unix::fs::PermissionsExt;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use claude_wrapper::duplex::{
    DuplexOptions, DuplexSession, InboundEvent, PermissionDecision, PermissionHandler,
    SessionExitStatus,
};
use claude_wrapper::{Claude, DuplexLimit, DuplexOperation, Error};

const RESULT: &str = r#"{"type":"result","subtype":"success","result":"ok","session_id":"fixture-session","total_cost_usd":0.0,"is_error":false}"#;

struct Fixture {
    dir: tempfile::TempDir,
    binary: std::path::PathBuf,
}

impl Fixture {
    fn new(body: &str) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let binary = dir.path().join("resident.sh");
        std::fs::write(&binary, format!("#!/bin/bash\nset -e\n{body}\n")).unwrap();
        std::fs::set_permissions(&binary, std::fs::Permissions::from_mode(0o755)).unwrap();
        Self { dir, binary }
    }

    fn client(&self) -> Claude {
        Claude::builder().binary(&self.binary).build().unwrap()
    }

    async fn spawn(&self) -> DuplexSession {
        self.spawn_with(DuplexOptions::default()).await
    }

    /// Spawn in the fixture directory, so scripts can use relative
    /// handshake files.
    async fn spawn_with(&self, opts: DuplexOptions) -> DuplexSession {
        let client = self.client().with_working_dir(self.dir.path());
        DuplexSession::spawn(&client, opts).await.unwrap()
    }

    /// Wait for a handshake file the fixture script writes.
    async fn wait_for(&self, name: &str) {
        let path = self.dir.path().join(name);
        tokio::time::timeout(SETTLE, async {
            while !path.exists() {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .unwrap_or_else(|_| panic!("fixture never wrote {name}"));
    }

    fn touch(&self, name: &str) {
        std::fs::write(self.dir.path().join(name), b"").unwrap();
    }
}

/// Short policy deadline used by the deadline tests.
const DEADLINE: Duration = Duration::from_secs(1);
/// Generous bound for settlement; failures here mean a hang, not slowness.
const SETTLE: Duration = Duration::from_secs(5);

#[cfg(target_os = "linux")]
#[tokio::test]
async fn transient_executable_busy_is_retried_before_resident_spawn() {
    let fixture = Fixture::new("cat >/dev/null");
    // Linux rejects exec while any writable handle to this file is open.
    // Keep that condition deterministic until the retrying spawn is pending.
    let writable = std::fs::OpenOptions::new()
        .write(true)
        .open(&fixture.binary)
        .unwrap();
    let client = fixture.client();
    let spawn = DuplexSession::spawn(&client, DuplexOptions::default());
    tokio::pin!(spawn);
    tokio::select! {
        early = &mut spawn => panic!("spawn must retry while executable is busy: {early:?}"),
        _ = tokio::time::sleep(Duration::from_millis(100)) => {},
    }
    drop(writable);
    let session = tokio::time::timeout(Duration::from_secs(3), spawn)
        .await
        .expect("spawn retry is bounded")
        .expect("spawn succeeds once executable is no longer busy");
    session.close().await.unwrap();
}

#[tokio::test]
async fn stderr_flood_does_not_block_resident_result() {
    let fixture = Fixture::new(&format!(
        "read -r request\nhead -c 1048576 /dev/zero >&2\nprintf '%s\\n' '{RESULT}'\ncat >/dev/null"
    ));
    let session = fixture.spawn().await;
    let turn = tokio::time::timeout(Duration::from_secs(3), session.send("probe")).await;
    let closed = session.close().await;
    assert_eq!(
        turn.expect("stderr must not block stdout")
            .unwrap()
            .result_text(),
        Some("ok")
    );
    closed.unwrap();
}

#[tokio::test]
async fn nonzero_exit_without_result_reaches_all_exit_observers() {
    let fixture = Fixture::new("read -r request\necho native-failure >&2\nexit 7");
    let session = fixture.spawn().await;
    let turn = session.send("probe").await;
    let (first, second) = tokio::join!(session.wait_for_exit(), session.wait_for_exit());
    let snapshot = session.exit_status();
    let closed = session.close().await;
    assert!(matches!(turn, Err(Error::DuplexClosed)));
    for state in [first, second, snapshot] {
        assert!(matches!(state, SessionExitStatus::Failed(_)), "{state:?}");
    }
    match closed {
        Err(Error::DuplexProcessFailed { status, stderr, .. }) => {
            assert_eq!(status.code(), Some(7));
            assert!(stderr.contains("native-failure"));
        }
        other => panic!("expected native process failure, got {other:?}"),
    }
}

#[tokio::test]
async fn result_success_does_not_hide_later_process_failure() {
    let fixture = Fixture::new(&format!(
        "read -r request\nprintf '%s\\n' '{RESULT}'\necho late-failure >&2\nexit 9"
    ));
    let session = fixture.spawn().await;
    let turn = session.send("probe").await.unwrap();
    let exit = session.wait_for_exit().await;
    let closed = session.close().await;
    assert_eq!(turn.subtype(), Some("success"));
    assert!(!turn.is_error());
    assert!(matches!(exit, SessionExitStatus::Failed(_)), "{exit:?}");
    assert!(
        matches!(closed, Err(Error::DuplexProcessFailed { status, .. }) if status.code() == Some(9)),
        "{closed:?}"
    );
}

#[tokio::test]
async fn stderr_failure_retains_a_bounded_diagnostic_tail() {
    let fixture = Fixture::new(
        "read -r request\necho discarded-prefix >&2\nhead -c 1048576 /dev/zero | tr '\\0' x >&2\necho important-tail >&2\nexit 11",
    );
    let session = fixture.spawn().await;
    let turn = tokio::time::timeout(Duration::from_secs(3), session.send("probe")).await;
    let closed = session.close().await;
    assert!(turn.is_ok(), "stderr backpressure blocked process exit");
    match closed {
        Err(Error::DuplexProcessFailed { status, stderr, .. }) => {
            assert_eq!(status.code(), Some(11));
            assert!(stderr.ends_with("important-tail\n"));
            assert!(!stderr.contains("discarded-prefix"));
            assert!(stderr.len() <= 64 * 1024, "{} bytes retained", stderr.len());
        }
        other => panic!("expected bounded stderr diagnostics, got {other:?}"),
    }
}

#[tokio::test]
async fn clean_exit_with_stderr_is_completed() {
    let fixture = Fixture::new("echo harmless-diagnostic >&2\nexit 0");
    let session = fixture.spawn().await;
    let exit = session.wait_for_exit().await;
    let closed = session.close().await;
    assert!(matches!(exit, SessionExitStatus::Completed), "{exit:?}");
    closed.unwrap();
}

#[tokio::test]
async fn stderr_eof_does_not_stop_an_active_session() {
    let fixture = Fixture::new(&format!(
        "exec 2>&-\nread -r request\nprintf '%s\\n' '{RESULT}'\ncat >/dev/null"
    ));
    let session = fixture.spawn().await;
    let turn = session.send("probe").await.unwrap();
    assert_eq!(turn.result_text(), Some("ok"));
    session.close().await.unwrap();
}

#[tokio::test]
async fn permission_handler_wait_does_not_stop_stderr_drain() {
    // This is the existing can_use_tool wire shape. The test proves I/O
    // progress during a handler wait, not native permission delivery.
    let request = r#"{"type":"control_request","request_id":"fixture-permission","request":{"subtype":"can_use_tool","tool_name":"Bash","input":{"command":"true"}}}"#;
    let fixture = Fixture::new(&format!(
        "read -r request\nprintf '%s\\n' '{request}'\nhead -c 1048576 /dev/zero >&2\necho ready > ready\nread -r response\nprintf '%s\\n' '{RESULT}'\ncat >/dev/null"
    ));
    let ready = fixture.dir.path().join("ready");
    let observed = Arc::new(AtomicBool::new(false));
    let flag = Arc::clone(&observed);
    let handler = PermissionHandler::new(move |_| {
        let ready = ready.clone();
        let flag = Arc::clone(&flag);
        async move {
            let waited = tokio::time::timeout(Duration::from_secs(2), async {
                while !ready.exists() {
                    tokio::time::sleep(Duration::from_millis(5)).await;
                }
            })
            .await;
            flag.store(waited.is_ok(), Ordering::SeqCst);
            PermissionDecision::Deny {
                message: "fixture denial".into(),
            }
        }
    });
    let client = fixture.client().with_working_dir(fixture.dir.path());
    let session = DuplexSession::spawn(&client, DuplexOptions::default().on_permission(handler))
        .await
        .unwrap();
    let turn = tokio::time::timeout(Duration::from_secs(3), session.send("probe")).await;
    let closed = session.close().await;
    assert!(
        observed.load(Ordering::SeqCst),
        "handler stalled the stderr reader"
    );
    assert_eq!(turn.unwrap().unwrap().result_text(), Some("ok"));
    closed.unwrap();
}

#[tokio::test]
async fn native_signal_is_preserved_without_an_invented_exit_code() {
    use std::os::unix::process::ExitStatusExt;
    let fixture = Fixture::new("read -r request\nkill -TERM $$");
    let session = fixture.spawn().await;
    let _turn = session.send("probe").await;
    let exit = session.wait_for_exit().await;
    let closed = session.close().await;
    assert!(matches!(exit, SessionExitStatus::Failed(_)));
    match closed {
        Err(Error::DuplexProcessFailed {
            status,
            shutdown_budget_exceeded,
            ..
        }) => {
            assert_eq!(status.code(), None);
            assert_eq!(status.signal(), Some(libc::SIGTERM));
            assert!(!shutdown_budget_exceeded);
        }
        other => panic!("expected native signal evidence, got {other:?}"),
    }
}

#[tokio::test]
async fn forced_shutdown_retains_native_status_and_deadline_context() {
    use std::os::unix::process::ExitStatusExt;
    let fixture = Fixture::new("while :; do sleep 1; done");
    let session = fixture.spawn().await;
    let closed = tokio::time::timeout(Duration::from_secs(9), session.close())
        .await
        .expect("shutdown must settle after its five-second budget");
    match closed {
        Err(Error::DuplexProcessFailed {
            status,
            shutdown_budget_exceeded,
            ..
        }) => {
            assert_eq!(status.code(), None);
            assert_eq!(status.signal(), Some(libc::SIGKILL));
            assert!(shutdown_budget_exceeded);
        }
        other => panic!("expected forced-shutdown evidence, got {other:?}"),
    }
}

#[tokio::test]
async fn inherited_stderr_does_not_hold_exit_state_open() {
    let fixture = Fixture::new(
        "read -r request\n(while [[ ! -e release ]]; do sleep 0.01; done; echo done > done) >/dev/null &\necho native-failure >&2\nexit 7",
    );
    let client = fixture.client().with_working_dir(fixture.dir.path());
    let session = DuplexSession::spawn(&client, DuplexOptions::default())
        .await
        .unwrap();
    let _turn = session.send("probe").await;
    let settled = tokio::time::timeout(Duration::from_secs(3), session.wait_for_exit()).await;
    // Release the deliberately surviving pipe holder before any assertion,
    // including when settlement failed, so the fixture cannot leave a child.
    std::fs::write(fixture.dir.path().join("release"), b"release").unwrap();
    let closed = session.close().await;
    tokio::time::timeout(Duration::from_secs(3), async {
        while !fixture.dir.path().join("done").exists() {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("fixture descendant must finish");
    assert!(
        matches!(settled, Ok(SessionExitStatus::Failed(_))),
        "{settled:?}"
    );
    assert!(
        matches!(closed, Err(Error::DuplexProcessFailed { status, .. }) if status.code()==Some(7))
    );
}

// -- Opt-in resource limits (#838) ----------------------------------------

fn assert_limit<T: std::fmt::Debug>(result: Result<T, Error>, expected: DuplexLimit, max: usize) {
    match result {
        Err(Error::DuplexLimitExceeded {
            limit,
            max: got,
            process_settled,
            ..
        }) => {
            assert_eq!(limit, expected);
            assert_eq!(got, max);
            assert!(process_settled, "the direct child must be reaped");
        }
        other => panic!("expected {expected} limit error, got {other:?}"),
    }
}

/// Settle a session that a policy closed: every exit observer sees a
/// failure and `close` returns the same policy error kind.
async fn assert_closed_by_policy(session: DuplexSession, needle: &str) -> Result<(), Error> {
    let exit = tokio::time::timeout(SETTLE, session.wait_for_exit())
        .await
        .expect("policy closure must settle");
    match &exit {
        SessionExitStatus::Failed(message) => assert!(message.contains(needle), "{message}"),
        other => panic!("expected a failed exit, got {other:?}"),
    }
    assert!(matches!(
        session.send("after closure").await,
        Err(Error::DuplexClosed)
    ));
    tokio::time::timeout(SETTLE, session.close())
        .await
        .expect("close must settle")
}

#[tokio::test]
async fn oversized_unterminated_frame_closes_the_session() {
    let fixture = Fixture::new(
        "read -r request\nhead -c 4194304 /dev/zero | tr '\\0' x\nwhile :; do sleep 1; done",
    );
    let session = fixture
        .spawn_with(DuplexOptions::default().max_frame_bytes(64 * 1024))
        .await;
    let turn = tokio::time::timeout(SETTLE, session.send("probe"))
        .await
        .expect("frame limit must settle the turn");
    assert_limit(turn, DuplexLimit::FrameBytes, 64 * 1024);
    let closed = assert_closed_by_policy(session, "frame bytes").await;
    assert_limit(closed, DuplexLimit::FrameBytes, 64 * 1024);
}

#[tokio::test]
async fn many_small_events_without_a_result_trip_the_event_limit() {
    let fixture = Fixture::new(
        "read -r request\nwhile :; do printf '%s\\n' '{\"type\":\"assistant\",\"message\":{}}'; done",
    );
    let session = fixture
        .spawn_with(DuplexOptions::default().max_turn_events(100))
        .await;
    let mut rx = session.subscribe();
    let turn = tokio::time::timeout(SETTLE, session.send("probe"))
        .await
        .expect("event limit must settle the turn");
    assert_limit(turn, DuplexLimit::TurnEvents, 100);
    let mut broadcast = 0;
    while let Ok(event) = rx.try_recv() {
        assert!(matches!(event, InboundEvent::Assistant(_)));
        broadcast += 1;
    }
    assert_eq!(broadcast, 100, "only retained events are broadcast");
    let closed = assert_closed_by_policy(session, "turn events").await;
    assert_limit(closed, DuplexLimit::TurnEvents, 100);
}

#[tokio::test]
async fn blank_and_malformed_lines_count_toward_turn_bytes() {
    // Nothing here is retained, so only byte accounting can stop it.
    let fixture = Fixture::new("read -r request\nwhile :; do printf '\\n\\nnot-json\\n\\n'; done");
    let session = fixture
        .spawn_with(DuplexOptions::default().max_turn_bytes(4096))
        .await;
    let turn = tokio::time::timeout(SETTLE, session.send("probe"))
        .await
        .expect("blank-line flood must trip the byte budget");
    assert_limit(turn, DuplexLimit::TurnBytes, 4096);
    let closed = assert_closed_by_policy(session, "turn bytes").await;
    assert_limit(closed, DuplexLimit::TurnBytes, 4096);
}

#[tokio::test]
async fn turn_budgets_reset_for_each_turn() {
    // Each turn writes one event plus the result, about 160 bytes. Three
    // turns exceed the per-turn budget only if it accumulates.
    let fixture = Fixture::new(&format!(
        "while read -r request; do\n  printf '%s\\n' '{{\"type\":\"assistant\",\"message\":{{}}}}'\n  printf '%s\\n' '{RESULT}'\ndone"
    ));
    let session = fixture
        .spawn_with(
            DuplexOptions::default()
                .max_turn_bytes(200)
                .max_turn_events(1),
        )
        .await;
    for _ in 0..3 {
        let turn = session.send("probe").await.unwrap();
        assert_eq!(turn.result_text(), Some("ok"));
        assert_eq!(turn.events.len(), 1);
    }
    session.close().await.unwrap();
}

#[tokio::test]
async fn overflow_settles_both_the_turn_and_a_pending_control_request() {
    // The flood starts only after both requests were written.
    let fixture = Fixture::new(
        "read -r turn\nread -r control\nhead -c 1048576 /dev/zero | tr '\\0' x\nwhile :; do sleep 1; done",
    );
    let session = fixture
        .spawn_with(DuplexOptions::default().max_frame_bytes(4096))
        .await;
    let (turn, control) = tokio::time::timeout(SETTLE, async {
        tokio::join!(session.send("probe"), session.interrupt())
    })
    .await
    .expect("overflow must settle every waiter");
    assert_limit(turn, DuplexLimit::FrameBytes, 4096);
    assert_limit(control, DuplexLimit::FrameBytes, 4096);
    let closed = assert_closed_by_policy(session, "frame bytes").await;
    assert_limit(closed, DuplexLimit::FrameBytes, 4096);
}

#[tokio::test]
async fn limits_are_off_by_default() {
    // A 4 MiB frame and a few thousand events pass without any opt-in.
    let fixture = Fixture::new(&format!(
        "read -r request\nprintf '{{\"type\":\"assistant\",\"pad\":\"'\nhead -c 4194304 /dev/zero | tr '\\0' x\nprintf '\"}}\\n'\nfor i in $(seq 2000); do printf '%s\\n' '{{\"type\":\"assistant\"}}'; done\nprintf '%s\\n' '{RESULT}'\ncat >/dev/null"
    ));
    let session = fixture.spawn().await;
    let turn = tokio::time::timeout(SETTLE, session.send("probe"))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(turn.events.len(), 2001);
    session.close().await.unwrap();
}

// -- Opt-in deadlines (#839) ----------------------------------------------

fn assert_deadline<T: std::fmt::Debug>(result: Result<T, Error>, expected: DuplexOperation) {
    assert_deadline_after(result, expected, DEADLINE);
}

fn assert_deadline_after<T: std::fmt::Debug>(
    result: Result<T, Error>,
    expected: DuplexOperation,
    expected_timeout: Duration,
) {
    match result {
        Err(Error::DuplexDeadlineExceeded {
            operation,
            timeout,
            process_settled,
            ..
        }) => {
            assert_eq!(operation, expected);
            assert_eq!(timeout, expected_timeout);
            assert!(process_settled, "the direct child must be reaped");
        }
        other => panic!("expected {expected} deadline error, got {other:?}"),
    }
}

async fn timed<F: std::future::Future>(future: F) -> (F::Output, Duration) {
    let started = std::time::Instant::now();
    let output = tokio::time::timeout(SETTLE, future)
        .await
        .expect("deadline must settle the operation");
    (output, started.elapsed())
}

#[tokio::test]
async fn turn_without_a_result_expires_and_closes_the_session() {
    let fixture = Fixture::new("read -r request\ncat >/dev/null");
    let session = fixture
        .spawn_with(DuplexOptions::default().turn_timeout(DEADLINE))
        .await;
    let (turn, elapsed) = timed(session.send("probe")).await;
    assert!(elapsed >= DEADLINE, "expired early after {elapsed:?}");
    assert_deadline(turn, DuplexOperation::Turn);
    let closed = assert_closed_by_policy(session, "turn deadline").await;
    assert_deadline(closed, DuplexOperation::Turn);
}

#[tokio::test]
async fn control_request_without_an_ack_expires() {
    let fixture = Fixture::new("cat >/dev/null");
    let session = fixture
        .spawn_with(DuplexOptions::default().control_timeout(DEADLINE))
        .await;
    let (control, _) = timed(session.interrupt()).await;
    assert_deadline(control, DuplexOperation::Control);
    let closed = assert_closed_by_policy(session, "control request deadline").await;
    assert_deadline(closed, DuplexOperation::Control);
}

#[tokio::test]
async fn interrupt_ack_does_not_satisfy_the_turn_deadline() {
    let ack = r#"{"type":"control_response","response":{"request_id":"interrupt-1","subtype":"success"}}"#;
    let fixture = Fixture::new(&format!(
        "read -r turn\nread -r control\nprintf '%s\\n' '{ack}'\ncat >/dev/null"
    ));
    let session = fixture
        .spawn_with(
            DuplexOptions::default()
                .turn_timeout(DEADLINE)
                .control_timeout(SETTLE),
        )
        .await;
    let ((turn, control), _) =
        timed(async { tokio::join!(session.send("probe"), session.interrupt()) }).await;
    control.expect("the ack resolves the interrupt");
    assert_deadline(turn, DuplexOperation::Turn);
    let _ = session.close().await;
}

#[tokio::test]
async fn result_before_expiry_succeeds_and_clears_the_deadline() {
    let fixture = Fixture::new(&format!(
        "while read -r request; do printf '%s\\n' '{RESULT}'; done"
    ));
    let session = fixture
        .spawn_with(
            DuplexOptions::default()
                .turn_timeout(DEADLINE)
                .control_timeout(DEADLINE),
        )
        .await;
    assert_eq!(
        session.send("first").await.unwrap().result_text(),
        Some("ok")
    );
    // Idle past the deadline: no operation is pending, so nothing expires.
    tokio::time::sleep(DEADLINE * 2).await;
    assert!(session.is_alive());
    assert_eq!(
        session.send("second").await.unwrap().result_text(),
        Some("ok")
    );
    session.close().await.unwrap();
}

#[tokio::test]
async fn closed_stdout_does_not_extend_a_turn_deadline_to_the_shutdown_budget() {
    // EOF without exit would otherwise wait the five-second shutdown budget.
    let fixture = Fixture::new("read -r request\nexec >&-\nwhile :; do sleep 1; done");
    let session = fixture
        .spawn_with(DuplexOptions::default().turn_timeout(DEADLINE))
        .await;
    let (turn, elapsed) = timed(session.send("probe")).await;
    assert!(elapsed < Duration::from_secs(3), "waited {elapsed:?}");
    assert_deadline(turn, DuplexOperation::Turn);
    let _ = session.close().await;
}

#[tokio::test]
async fn native_exit_racing_the_deadline_settles_either_way() {
    let fixture = Fixture::new("read -r request\nsleep 1\nexit 0");
    let session = fixture
        .spawn_with(DuplexOptions::default().turn_timeout(DEADLINE))
        .await;
    let (turn, _) = timed(session.send("probe")).await;
    let closed = tokio::time::timeout(SETTLE, session.close())
        .await
        .expect("close must settle");
    match (turn, closed) {
        (Err(Error::DuplexClosed), Ok(())) => {}
        (Err(Error::DuplexDeadlineExceeded { .. }), Err(Error::DuplexDeadlineExceeded { .. })) => {}
        other => panic!("unexpected race outcome: {other:?}"),
    }
}

#[tokio::test]
async fn continuously_ready_stdout_does_not_starve_the_deadline() {
    let fixture = Fixture::new("read -r request\nyes ''");
    let session = fixture
        .spawn_with(DuplexOptions::default().turn_timeout(DEADLINE))
        .await;
    let (turn, _) = timed(session.send("probe")).await;
    assert_deadline(turn, DuplexOperation::Turn);
    let _ = session.close().await;
}

#[tokio::test]
async fn deferred_approval_counts_toward_the_turn_deadline() {
    let request = r#"{"type":"control_request","request_id":"fixture-permission","request":{"subtype":"can_use_tool","tool_name":"Bash","input":{"command":"true"}}}"#;
    let fixture = Fixture::new(&format!(
        "read -r request\nprintf '%s\\n' '{request}'\nread -r response\nprintf '%s\\n' '{RESULT}'\ncat >/dev/null"
    ));
    let deferred = Arc::new(AtomicBool::new(false));
    let flag = Arc::clone(&deferred);
    let handler = PermissionHandler::new(move |_| {
        let flag = Arc::clone(&flag);
        async move {
            flag.store(true, Ordering::SeqCst);
            PermissionDecision::Defer
        }
    });
    let session = fixture
        .spawn_with(
            DuplexOptions::default()
                .on_permission(handler)
                .turn_timeout(DEADLINE),
        )
        .await;
    let (turn, _) = timed(session.send("probe")).await;
    assert!(deferred.load(Ordering::SeqCst));
    assert_deadline(turn, DuplexOperation::Turn);
    let _ = session.close().await;
}

#[tokio::test]
async fn deferred_approval_answered_in_time_succeeds() {
    let request = r#"{"type":"control_request","request_id":"fixture-permission","request":{"subtype":"can_use_tool","tool_name":"Bash","input":{"command":"true"}}}"#;
    let fixture = Fixture::new(&format!(
        "read -r request\nprintf '%s\\n' '{request}'\nread -r response\nprintf '%s\\n' '{RESULT}'\ncat >/dev/null"
    ));
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let handler = PermissionHandler::new(move |req| {
        let _ = tx.send(req.request_id);
        async { PermissionDecision::Defer }
    });
    let session = fixture
        .spawn_with(
            DuplexOptions::default()
                .on_permission(handler)
                .turn_timeout(SETTLE),
        )
        .await;
    let answer = async {
        let id = rx.recv().await.unwrap();
        session
            .respond_to_permission(
                id,
                PermissionDecision::Deny {
                    message: "fixture denial".into(),
                },
            )
            .unwrap();
    };
    let (turn, ()) = tokio::join!(session.send("probe"), answer);
    assert_eq!(turn.unwrap().result_text(), Some("ok"));
    session.close().await.unwrap();
}

#[tokio::test]
async fn stuck_permission_callback_is_cancelled_at_the_deadline() {
    struct DropFlag(Arc<AtomicBool>);
    impl Drop for DropFlag {
        fn drop(&mut self) {
            self.0.store(true, Ordering::SeqCst);
        }
    }
    let request = r#"{"type":"control_request","request_id":"fixture-permission","request":{"subtype":"can_use_tool","tool_name":"Bash","input":{"command":"true"}}}"#;
    let fixture = Fixture::new(&format!(
        "read -r request\nprintf '%s\\n' '{request}'\ncat >/dev/null"
    ));
    let dropped = Arc::new(AtomicBool::new(false));
    let flag = Arc::clone(&dropped);
    let handler = PermissionHandler::new(move |_| {
        let guard = DropFlag(Arc::clone(&flag));
        async move {
            let _guard = guard;
            std::future::pending::<PermissionDecision>().await
        }
    });
    let session = fixture
        .spawn_with(
            DuplexOptions::default()
                .on_permission(handler)
                .turn_timeout(DEADLINE),
        )
        .await;
    let (turn, _) = timed(session.send("probe")).await;
    assert_deadline(turn, DuplexOperation::Turn);
    assert!(
        dropped.load(Ordering::SeqCst),
        "callback future must be dropped"
    );
    let closed = assert_closed_by_policy(session, "turn deadline").await;
    assert_deadline(closed, DuplexOperation::Turn);
}

#[tokio::test]
async fn blocked_stdin_write_is_bounded_by_the_turn_deadline() {
    // The child never reads stdin, so a prompt larger than the pipe buffer
    // blocks the write indefinitely.
    let fixture = Fixture::new("while :; do sleep 1; done");
    let session = fixture
        .spawn_with(DuplexOptions::default().turn_timeout(DEADLINE))
        .await;
    let (turn, _) = timed(session.send("x".repeat(4 << 20))).await;
    assert_deadline(turn, DuplexOperation::Turn);
    let closed = assert_closed_by_policy(session, "turn deadline").await;
    assert_deadline(closed, DuplexOperation::Turn);
}

#[tokio::test]
async fn dropped_send_abandons_only_the_wait() {
    let fixture = Fixture::new(&format!(
        "read -r request\ntouch accepted\nwhile [[ ! -e release ]]; do sleep 0.01; done\nprintf '%s\\n' '{RESULT}'\nwhile read -r request; do printf '%s\\n' '{RESULT}'; done"
    ));
    let session = fixture.spawn().await;
    {
        let first = session.send("first");
        tokio::pin!(first);
        tokio::select! {
            early = &mut first => panic!("turn resolved before release: {early:?}"),
            () = fixture.wait_for("accepted") => {}
        }
    }
    // The abandoned turn is still in flight in the protocol.
    assert!(matches!(
        session.send("second").await,
        Err(Error::DuplexTurnInFlight)
    ));
    fixture.touch("release");
    let turn = tokio::time::timeout(SETTLE, async {
        loop {
            match session.send("third").await {
                Err(Error::DuplexTurnInFlight) => {
                    tokio::time::sleep(Duration::from_millis(5)).await;
                }
                other => break other,
            }
        }
    })
    .await
    .expect("the abandoned turn must finish once its result arrives");
    assert_eq!(turn.unwrap().result_text(), Some("ok"));
    session.close().await.unwrap();
}

#[tokio::test]
async fn dropped_send_still_expires_at_its_deadline() {
    let fixture = Fixture::new("read -r request\ntouch accepted\ncat >/dev/null");
    let session = fixture
        .spawn_with(DuplexOptions::default().turn_timeout(DEADLINE))
        .await;
    {
        let first = session.send("first");
        tokio::pin!(first);
        tokio::select! {
            early = &mut first => panic!("turn resolved early: {early:?}"),
            () = fixture.wait_for("accepted") => {}
        }
    }
    let closed = assert_closed_by_policy(session, "turn deadline").await;
    assert_deadline(closed, DuplexOperation::Turn);
}

// -- Partial messages (#840) ----------------------------------------------

#[tokio::test]
async fn partial_messages_are_forwarded_only_when_requested() {
    let partial = r#"{"type":"stream_event","event":{"type":"content_block_delta","delta":{"type":"text_delta","text":"o"}}}"#;
    let fixture = Fixture::new(&format!(
        "partial=\nfor arg in \"$@\"; do\n  if [[ $arg == --include-partial-messages ]]; then partial=1; fi\ndone\nwhile read -r request; do\n  if [[ -n $partial ]]; then printf '%s\\n' '{partial}'; fi\n  printf '%s\\n' '{RESULT}'\ndone"
    ));
    for opt_in in [false, true] {
        let opts = if opt_in {
            DuplexOptions::default().include_partial_messages()
        } else {
            DuplexOptions::default()
        };
        let session = fixture.spawn_with(opts).await;
        let mut rx = session.subscribe();
        let turn = session.send("probe").await.unwrap();
        let retained = turn
            .events
            .iter()
            .filter(|event| event["type"] == "stream_event")
            .count();
        let broadcast = std::iter::from_fn(|| rx.try_recv().ok())
            .filter(|event| matches!(event, InboundEvent::StreamEvent(_)))
            .count();
        assert_eq!(retained, usize::from(opt_in));
        assert_eq!(broadcast, usize::from(opt_in));
        session.close().await.unwrap();
    }
}

#[tokio::test]
async fn invalid_utf8_is_not_silently_skipped() {
    let fixture = Fixture::new(&format!(
        "read -r request\nprintf '\\377\\n'\nprintf '%s\\n' '{RESULT}'\ncat >/dev/null"
    ));
    let session = fixture.spawn().await;
    let turn = session.send("probe").await;
    let closed = session.close().await;
    assert!(
        matches!(turn, Err(Error::DuplexClosed)),
        "invalid UTF-8 must close the stream: {turn:?}"
    );
    assert!(matches!(closed, Err(Error::Io { .. })), "{closed:?}");
}

#[tokio::test]
async fn deadline_survives_shutdown_grace() {
    let fixture = Fixture::new("read -r request\nexec 1>&-\nwhile :; do sleep 1; done");
    let client = Claude::builder()
        .binary(&fixture.binary)
        .kill_grace(Duration::from_secs(2))
        .build()
        .unwrap();
    let session = DuplexSession::spawn(
        &client,
        DuplexOptions::default().turn_timeout(Duration::from_secs(6)),
    )
    .await
    .unwrap();
    let turn = tokio::time::timeout(Duration::from_secs(10), session.send("probe")).await;
    let closed = session.close().await;
    assert!(
        matches!(
            turn,
            Ok(Err(Error::DuplexDeadlineExceeded {
                process_settled: true,
                ..
            }))
        ),
        "{turn:?}"
    );
    assert!(
        matches!(
            closed,
            Err(Error::DuplexDeadlineExceeded {
                process_settled: true,
                ..
            })
        ),
        "{closed:?}"
    );
}

#[tokio::test]
async fn stdout_flood_does_not_starve_control_admission() {
    let fixture = Fixture::new("echo $$ > flood.pid\nexec yes '{\"type\":\"assistant\"}'");
    let client = fixture.client().with_working_dir(fixture.dir.path());
    let session = DuplexSession::spawn(
        &client,
        DuplexOptions::default().control_timeout(Duration::from_millis(100)),
    )
    .await
    .unwrap();
    let mut events = session.subscribe();
    tokio::time::timeout(Duration::from_secs(3), events.recv())
        .await
        .unwrap()
        .unwrap();
    let outcome = tokio::time::timeout(Duration::from_secs(3), session.interrupt()).await;
    if outcome.is_err() {
        let pid: i32 = std::fs::read_to_string(fixture.dir.path().join("flood.pid"))
            .unwrap()
            .trim()
            .parse()
            .unwrap();
        // The deliberately stalled owned fixture must not survive a failed test.
        unsafe {
            libc::kill(-pid, libc::SIGKILL);
        }
    }
    let closed = session.close().await;
    assert!(
        matches!(outcome, Ok(Err(Error::DuplexDeadlineExceeded { .. }))),
        "{outcome:?}; close: {closed:?}"
    );
}

#[tokio::test]
async fn zero_deadline_expires_the_accepted_turn() {
    let fixture = Fixture::new("cat >/dev/null");
    let session = fixture
        .spawn_with(DuplexOptions::default().turn_timeout(Duration::ZERO))
        .await;
    let turn = tokio::time::timeout(Duration::from_secs(3), session.send("probe"))
        .await
        .unwrap();
    assert_deadline_after(turn, DuplexOperation::Turn, Duration::ZERO);
    assert_deadline_after(
        assert_closed_by_policy(session, "turn deadline").await,
        DuplexOperation::Turn,
        Duration::ZERO,
    );
}
