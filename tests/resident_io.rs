//! Controlled child-process checks for resident I/O and exit evidence.
#![cfg(all(feature = "async", feature = "json", unix))]

use std::os::unix::fs::PermissionsExt;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use claude_wrapper::duplex::{
    DuplexOptions, DuplexSession, PermissionDecision, PermissionHandler, SessionExitStatus,
};
use claude_wrapper::{Claude, Error};

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
        DuplexSession::spawn(&self.client(), DuplexOptions::default())
            .await
            .unwrap()
    }
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
