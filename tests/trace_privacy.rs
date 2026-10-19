//! Synthetic content markers test wrapper diagnostics, not native wire behavior.
#![cfg(all(unix, any(feature = "async", feature = "sync")))]

#[cfg(feature = "json")]
use claude_wrapper::QueryCommand;
use claude_wrapper::{Claude, RetryPolicy};
use std::cell::RefCell;
use std::os::unix::fs::PermissionsExt;
use std::sync::{Arc, Mutex, Once};
use std::time::Duration;
use tracing::field::{Field, Visit};
use tracing::span::{Attributes, Id, Record};
use tracing::{Event, Metadata, Subscriber};

type Sink = Arc<Mutex<String>>;
thread_local! { static SINK: RefCell<Option<Sink>> = const { RefCell::new(None) }; }
struct Capture;
impl Capture {
    fn record(f: impl FnOnce(&mut String)) {
        SINK.with(|s| {
            if let Some(s) = s.borrow().as_ref() {
                f(&mut s.lock().unwrap());
            }
        });
    }
}
struct Fields<'a>(&'a mut String);
impl Visit for Fields<'_> {
    fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
        use std::fmt::Write;
        writeln!(self.0, "{}={value:?}", field.name()).unwrap();
    }
}
impl Subscriber for Capture {
    fn enabled(&self, m: &Metadata<'_>) -> bool {
        m.target().starts_with("claude_wrapper")
    }
    fn new_span(&self, a: &Attributes<'_>) -> Id {
        Self::record(|s| {
            s.push_str(a.metadata().name());
            a.record(&mut Fields(s));
        });
        Id::from_u64(1)
    }
    fn record(&self, _: &Id, r: &Record<'_>) {
        Self::record(|s| r.record(&mut Fields(s)));
    }
    fn record_follows_from(&self, _: &Id, _: &Id) {}
    fn event(&self, e: &Event<'_>) {
        Self::record(|s| e.record(&mut Fields(s)));
    }
    fn enter(&self, _: &Id) {}
    fn exit(&self, _: &Id) {}
}
struct Recorder(Sink);
impl Recorder {
    fn start() -> Self {
        static INIT: Once = Once::new();
        INIT.call_once(|| tracing::subscriber::set_global_default(Capture).unwrap());
        let sink = Arc::new(Mutex::new(String::new()));
        SINK.with(|s| *s.borrow_mut() = Some(sink.clone()));
        Self(sink)
    }
    fn assert_private(&self) {
        let s = self.0.lock().unwrap();
        assert!(s.contains("message="), "subscriber must see events: {s}");
        assert!(!s.contains("TRACE_PRIVATE"), "private content leaked: {s}");
        assert!(
            s.contains("arg_count="),
            "useful operation metadata is present: {s}"
        );
    }
}
impl Drop for Recorder {
    fn drop(&mut self) {
        SINK.with(|s| *s.borrow_mut() = None);
    }
}
struct Fixture {
    dir: tempfile::TempDir,
    binary: std::path::PathBuf,
}
impl Fixture {
    fn new(body: &str) -> Self {
        let dir = tempfile::Builder::new()
            .prefix("TRACE_PRIVATE_path")
            .tempdir()
            .unwrap();
        let binary = dir.path().join("TRACE_PRIVATE_binary");
        std::fs::write(&binary, format!("#!/bin/bash\n{body}\n")).unwrap();
        std::fs::set_permissions(&binary, std::fs::Permissions::from_mode(0o755)).unwrap();
        Self { dir, binary }
    }
    fn client(&self) -> Claude {
        Claude::builder()
            .binary(&self.binary)
            .working_dir(self.dir.path())
            .arg("TRACE_PRIVATE_global")
            .env("TRACE_PRIVATE_key", "TRACE_PRIVATE_env")
            .kill_grace(Duration::from_millis(5))
            .build()
            .unwrap()
    }
}
const RESULT: &str = r#"{"type":"result","subtype":"success","result":"TRACE_PRIVATE_output","session_id":"TRACE_PRIVATE_native_id","total_cost_usd":0.0,"is_error":false}"#;
fn body() -> String {
    format!(
        "cat >/dev/null\nprintf '%s\\n' 'TRACE_PRIVATE_malformed_line' '{RESULT}'\nprintf 'TRACE_PRIVATE_stderr' >&2"
    )
}
fn args() -> Vec<String> {
    vec![
        "--print".into(),
        "TRACE_PRIVATE_prompt".into(),
        "--resume".into(),
        "TRACE_PRIVATE_resume".into(),
        "--settings".into(),
        "TRACE_PRIVATE_config".into(),
    ]
}

#[cfg(feature = "async")]
#[tokio::test]
async fn async_execution_diagnostics_exclude_content() {
    use claude_wrapper::exec::*;
    let record = Recorder::start();
    let fixture = Fixture::new(&body());
    let client = fixture.client();
    assert!(
        run_claude(&client, args())
            .await
            .unwrap()
            .stdout
            .contains("TRACE_PRIVATE")
    );
    run_claude_cancellable(&client, args(), std::future::pending())
        .await
        .unwrap();
    run_claude_with_stdin_prompt(&client, args(), "TRACE_PRIVATE_stdin".into())
        .await
        .unwrap();
    run_claude_with_stdin_prompt_cancellable(
        &client,
        args(),
        "TRACE_PRIVATE_stdin".into(),
        std::future::pending(),
    )
    .await
    .unwrap();
    record.assert_private();
}

#[cfg(all(feature = "async", feature = "json"))]
#[tokio::test]
async fn streaming_diagnostics_exclude_malformed_lines_and_arguments() {
    use claude_wrapper::streaming::*;
    let record = Recorder::start();
    let fixture = Fixture::new(&body());
    let client = fixture.client();
    let cmd = QueryCommand::new("TRACE_PRIVATE_prompt").resume("TRACE_PRIVATE_resume");
    let mut seen = 0;
    stream_query(&client, &cmd, |_| seen += 1).await.unwrap();
    stream_query_cancellable(&client, &cmd, std::future::pending(), |_| {})
        .await
        .unwrap();
    assert!(seen > 0, "valid protocol events remain delivered");
    assert!(
        cmd.to_command_string(&client).contains("TRACE_PRIVATE"),
        "explicit preview retains caller-controlled values"
    );
    record.assert_private();
}

#[cfg(all(feature = "async", feature = "json"))]
#[tokio::test]
async fn resident_diagnostics_exclude_protocol_content_and_identifiers() {
    use claude_wrapper::duplex::{DuplexOptions, DuplexSession};
    let record = Recorder::start();
    // Deliberately unrecognized/malformed messages exercise diagnostic boundaries.
    let fixture = Fixture::new(&format!(
        "read -r request\nprintf '%s\\n' 'TRACE_PRIVATE_malformed_line' '{{\"type\":\"system\",\"subtype\":\"init\",\"session_id\":\"TRACE_PRIVATE_id\"}}' '{{\"type\":\"control_request\",\"request_id\":\"TRACE_PRIVATE_request\",\"request\":{{\"subtype\":\"TRACE_PRIVATE_unknown\"}}}}' '{RESULT}'\ncat >/dev/null"
    ));
    let client = fixture.client();
    let session = DuplexSession::spawn(
        &client,
        DuplexOptions::default().model("TRACE_PRIVATE_model"),
    )
    .await
    .unwrap();
    assert_eq!(
        session
            .send("TRACE_PRIVATE_prompt")
            .await
            .unwrap()
            .result_text(),
        Some("TRACE_PRIVATE_output")
    );
    session.close().await.unwrap();
    record.assert_private();
}

#[cfg(feature = "async")]
#[tokio::test]
async fn retry_diagnostics_exclude_error_content() {
    let record = Recorder::start();
    let fixture =
        Fixture::new("printf 'TRACE_PRIVATE_stdout'; printf 'TRACE_PRIVATE_stderr' >&2; exit 7");
    let client = Claude::builder()
        .binary(&fixture.binary)
        .retry(
            RetryPolicy::new()
                .max_attempts(2)
                .initial_backoff(Duration::ZERO)
                .retry_on_exit_codes([7]),
        )
        .build()
        .unwrap();
    let error = claude_wrapper::exec::run_claude(&client, args())
        .await
        .unwrap_err();
    assert!(
        error.to_string().contains("TRACE_PRIVATE"),
        "explicit error retains diagnostics"
    );
    assert!(
        record
            .0
            .lock()
            .unwrap()
            .contains("retrying after transient error")
    );
    assert!(record.0.lock().unwrap().contains("exit_code=7"));
    record.assert_private();
}

#[cfg(feature = "sync")]
#[test]
fn sync_execution_and_retry_diagnostics_exclude_content() {
    use claude_wrapper::exec::*;
    let record = Recorder::start();
    let fixture = Fixture::new(&body());
    let client = fixture.client();
    assert!(
        run_claude_sync(&client, args())
            .unwrap()
            .stdout
            .contains("TRACE_PRIVATE")
    );
    run_claude_with_stdin_prompt_sync(&client, args(), "TRACE_PRIVATE_stdin".into()).unwrap();
    #[cfg(feature = "json")]
    {
        claude_wrapper::streaming::stream_query_sync(
            &client,
            &QueryCommand::new("TRACE_PRIVATE_prompt"),
            |_| {},
        )
        .unwrap();
    }
    let failing = Fixture::new("printf 'TRACE_PRIVATE_output'; exit 7");
    let client = Claude::builder()
        .binary(&failing.binary)
        .retry(
            RetryPolicy::new()
                .max_attempts(2)
                .initial_backoff(Duration::ZERO)
                .retry_on_exit_codes([7]),
        )
        .build()
        .unwrap();
    assert!(
        run_claude_sync(&client, args())
            .unwrap_err()
            .to_string()
            .contains("TRACE_PRIVATE")
    );
    record.assert_private();
}

#[cfg(feature = "async")]
#[tokio::test]
async fn capture_limit_diagnostics_exclude_retained_output() {
    let record = Recorder::start();
    let fixture = Fixture::new("printf 'TRACE_PRIVATE_output'; sleep 0.05; head -c 4096 /dev/zero");
    let client = Claude::builder()
        .binary(&fixture.binary)
        .output_limit(64)
        .build()
        .unwrap();
    assert!(matches!(
        claude_wrapper::exec::run_claude(&client, args()).await,
        Err(claude_wrapper::Error::OutputLimitExceeded { .. })
    ));
    #[cfg(feature = "sync")]
    assert!(matches!(
        claude_wrapper::exec::run_claude_sync(&client, args()),
        Err(claude_wrapper::Error::OutputLimitExceeded { .. })
    ));
    record.assert_private();
    assert!(
        record.0.lock().unwrap().contains("retained_prefix_bytes="),
        "capture warning was observed"
    );
}
