//! What the crate's spans publish, asserted against a real subscriber.
//!
//! The span field set is a contract: hosts filter and correlate on it, so a
//! renamed field or a dropped `record` is a breaking change that no compiler
//! catches. These tests install a collecting subscriber and assert on what it
//! actually receives.
//!
//! They also pin the safety property that matters more than any field: **no
//! span carries prompt text, environment, or credentials.** Argv positionals
//! are where prompts live, so a span that recorded the full argv would leak
//! every prompt into logs at debug level.

#![cfg(all(feature = "async", feature = "json"))]

use std::sync::{Arc, Mutex};

use claude_wrapper::Claude;
use tracing::subscriber::with_default;
use tracing::{Event, Metadata, Subscriber, span};

/// One span's name and the fields recorded on it.
type SpanRecord = (String, Vec<(String, String)>);

/// A subscriber that records span names and their recorded field values.
#[derive(Clone, Default)]
struct Collector {
    spans: Arc<Mutex<Vec<SpanRecord>>>,
}

impl Collector {
    fn names(&self) -> Vec<String> {
        self.spans
            .lock()
            .unwrap()
            .iter()
            .map(|(n, _)| n.clone())
            .collect()
    }

    /// Every field recorded on spans of the given name, flattened.
    fn fields_of(&self, span_name: &str) -> Vec<(String, String)> {
        self.spans
            .lock()
            .unwrap()
            .iter()
            .filter(|(n, _)| n == span_name)
            .flat_map(|(_, f)| f.clone())
            .collect()
    }

    fn all_field_values(&self) -> Vec<String> {
        self.spans
            .lock()
            .unwrap()
            .iter()
            .flat_map(|(_, f)| f.iter().map(|(_, v)| v.clone()).collect::<Vec<_>>())
            .collect()
    }
}

/// Visits field values into `(name, debug-rendered value)` pairs.
struct Visitor(Vec<(String, String)>);

impl tracing::field::Visit for Visitor {
    fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
        self.0
            .push((field.name().to_string(), format!("{value:?}")));
    }
    fn record_str(&mut self, field: &tracing::field::Field, value: &str) {
        self.0.push((field.name().to_string(), value.to_string()));
    }
}

impl Subscriber for Collector {
    fn enabled(&self, _: &Metadata<'_>) -> bool {
        true
    }

    fn new_span(&self, attrs: &span::Attributes<'_>) -> span::Id {
        let mut v = Visitor(Vec::new());
        attrs.record(&mut v);
        let mut spans = self.spans.lock().unwrap();
        spans.push((attrs.metadata().name().to_string(), v.0));
        span::Id::from_u64(spans.len() as u64)
    }

    fn record(&self, id: &span::Id, values: &span::Record<'_>) {
        let mut v = Visitor(Vec::new());
        values.record(&mut v);
        let idx = id.into_u64() as usize - 1;
        if let Some(entry) = self.spans.lock().unwrap().get_mut(idx) {
            entry.1.extend(v.0);
        }
    }

    fn record_follows_from(&self, _: &span::Id, _: &span::Id) {}
    fn event(&self, _: &Event<'_>) {}
    fn enter(&self, _: &span::Id) {}
    fn exit(&self, _: &span::Id) {}
}

/// A client pointed at the repo's fake CLI, so nothing real is spawned.
fn fake_claude() -> Claude {
    let fake = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fake-claude.sh");
    Claude::builder()
        .binary(fake)
        .working_dir(env!("CARGO_MANIFEST_DIR"))
        .build()
        .expect("building a client over the fake binary")
}

#[test]
fn exec_span_carries_command_binary_cwd_and_outcome() {
    let collector = Collector::default();
    let handle = collector.clone();
    let claude = fake_claude();

    with_default(collector, || {
        block_on(async {
            let _ = claude_wrapper::exec::run_claude(&claude, vec!["--version".into()]).await;
        });
    });

    assert!(
        handle.names().iter().any(|n| n == "claude.exec"),
        "expected a claude.exec span, saw {:?}",
        handle.names()
    );
    let fields = handle.fields_of("claude.exec");
    let names: Vec<&str> = fields.iter().map(|(k, _)| k.as_str()).collect();
    for expected in ["command", "mode", "binary", "exit_code", "duration_ms"] {
        assert!(names.contains(&expected), "missing {expected} in {names:?}");
    }
    // `command` is the first argv token, which is the subcommand for
    // subcommand invocations and the leading flag for print-mode runs.
    let command = fields.iter().find(|(k, _)| k == "command").unwrap();
    assert!(
        command.1.contains("--version"),
        "command should name the invocation, got {:?}",
        command.1
    );
}

#[test]
fn spans_never_carry_the_prompt() {
    // The safety property. Prompts arrive as argv positionals, so a span that
    // recorded argv would put every prompt in the logs.
    const SECRET: &str = "SPAN-LEAK-CANARY-do-not-log-this-prompt";
    let collector = Collector::default();
    let handle = collector.clone();
    let claude = fake_claude();

    with_default(collector, || {
        block_on(async {
            let _ = claude_wrapper::exec::run_claude(
                &claude,
                vec!["--print".into(), "--".into(), SECRET.into()],
            )
            .await;
        });
    });

    for value in handle.all_field_values() {
        assert!(
            !value.contains(SECRET),
            "a span field leaked the prompt: {value:?}"
        );
    }
    assert!(
        handle.names().iter().any(|n| n == "claude.exec"),
        "sanity: the span was actually created"
    );
}

#[test]
fn retry_span_wraps_attempts() {
    use claude_wrapper::RetryPolicy;

    let collector = Collector::default();
    let handle = collector.clone();
    // A binary that does not exist fails every attempt, so the retry loop runs
    // to exhaustion without spawning anything.
    let claude = Claude::builder()
        .binary("/nonexistent/claude-for-span-test")
        .retry(
            RetryPolicy::new()
                .max_attempts(2)
                .fixed()
                .initial_backoff(std::time::Duration::from_millis(1)),
        )
        .build()
        .unwrap();

    with_default(collector, || {
        block_on(async {
            let _ = claude_wrapper::exec::run_claude(&claude, vec!["--version".into()]).await;
        });
    });

    let names = handle.names();
    assert!(
        names.iter().any(|n| n == "claude.retry"),
        "expected a claude.retry span, saw {names:?}"
    );
    let fields = handle.fields_of("claude.retry");
    let keys: Vec<&str> = fields.iter().map(|(k, _)| k.as_str()).collect();
    assert!(keys.contains(&"max_attempts"), "got {keys:?}");
}

/// Run `fut` to completion on this thread.
///
/// `with_default` installs the subscriber per-thread and is synchronous, so
/// the runtime is built inside its scope rather than the test being a
/// `#[tokio::test]` (whose runtime thread would not see the subscriber).
fn block_on<F: std::future::Future>(fut: F) -> F::Output {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("building a test runtime")
        .block_on(fut)
}
