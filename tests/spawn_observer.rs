//! The spawn observer: does it fire, when, and with what.
//!
//! A supervisor uses this to record `(run_id, pid)` durably *before* a run can
//! be orphaned, so the properties that matter are timing (before output) and
//! the honesty of `pgid` (only `Some` when the pid is safe to `killpg`).

#![cfg(all(feature = "async", feature = "json"))]

use std::sync::{Arc, Mutex};

use claude_wrapper::{Claude, SpawnInfo};

/// Collect every `SpawnInfo` the crate reports.
fn recording() -> (Arc<Mutex<Vec<SpawnInfo>>>, claude_wrapper::SpawnObserver) {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let sink = seen.clone();
    (seen, Arc::new(move |info| sink.lock().unwrap().push(info)))
}

fn fake() -> &'static str {
    concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fake-claude.sh")
}

#[tokio::test]
async fn observer_reports_a_live_pid_for_a_one_shot_run() {
    let (seen, observer) = recording();
    let claude = Claude::builder()
        .binary(fake())
        .on_spawn(observer)
        .build()
        .unwrap();

    claude_wrapper::exec::run_claude(&claude, vec!["--version".into()])
        .await
        .expect("fake binary reports a version");

    let seen = seen.lock().unwrap();
    assert_eq!(seen.len(), 1, "exactly one spawn, got {seen:?}");
    assert!(seen[0].pid > 0, "pid must be real, got {}", seen[0].pid);
}

#[tokio::test]
async fn pgid_is_some_only_when_the_child_leads_its_own_group() {
    // The distinction is load-bearing: with process_group disabled the child
    // shares the caller's group, so passing its pid to killpg would signal the
    // supervisor's own process tree. Reporting pgid: None is what stops that.
    for (process_group, expect_pgid) in [(true, true), (false, false)] {
        let (seen, observer) = recording();
        let claude = Claude::builder()
            .binary(fake())
            .process_group(process_group)
            .on_spawn(observer)
            .build()
            .unwrap();

        claude_wrapper::exec::run_claude(&claude, vec!["--version".into()])
            .await
            .unwrap();

        let seen = seen.lock().unwrap();
        let info = seen.first().expect("observer fired");
        assert_eq!(
            info.pgid.is_some(),
            expect_pgid,
            "process_group={process_group} should give pgid.is_some()={expect_pgid}, got {info:?}"
        );
        if let Some(pgid) = info.pgid {
            assert_eq!(pgid, info.pid, "a group leader's pgid is its own pid");
        }
    }
}

#[tokio::test]
async fn observer_fires_before_the_run_produces_output() {
    // The whole point: a pid recorded only after the run finishes is useless
    // for reconciling a supervisor crash that happened mid-run.
    let order = Arc::new(Mutex::new(Vec::<&'static str>::new()));
    let sink = order.clone();
    let claude = Claude::builder()
        .binary(fake())
        .on_spawn(Arc::new(move |_| sink.lock().unwrap().push("spawn")))
        .build()
        .unwrap();

    let out = claude_wrapper::exec::run_claude(&claude, vec!["--version".into()])
        .await
        .unwrap();
    order.lock().unwrap().push("output");

    assert_eq!(
        *order.lock().unwrap(),
        vec!["spawn", "output"],
        "the observer must fire before output is available"
    );
    assert!(!out.stdout.is_empty(), "the run really did produce output");
}

#[tokio::test]
async fn each_retry_attempt_is_its_own_spawn() {
    use claude_wrapper::RetryPolicy;

    // Every attempt is a distinct process, so a supervisor tracking pids must
    // see one report per attempt or it will fail to reconcile the earlier ones.
    let (seen, observer) = recording();
    let claude = Claude::builder()
        // Spawns successfully and exits 1, which the policy below retries on.
        // The repo's fake binary always succeeds, so it cannot drive this.
        // `false` lives in /usr/bin on macOS and /bin on most Linux.
        .binary(if std::path::Path::new("/usr/bin/false").exists() {
            "/usr/bin/false"
        } else {
            "/bin/false"
        })
        .retry(
            RetryPolicy::new()
                .max_attempts(3)
                .fixed()
                .initial_backoff(std::time::Duration::from_millis(1))
                .retry_on_exit_codes([1]),
        )
        .on_spawn(observer)
        .build()
        .unwrap();

    let _ = claude_wrapper::exec::run_claude(&claude, vec!["--version".into()]).await;

    let seen = seen.lock().unwrap();
    assert_eq!(seen.len(), 3, "one spawn per attempt, got {seen:?}");
    // Distinct processes, so distinct pids.
    let first = seen[0].pid;
    assert!(
        seen.iter().any(|i| i.pid != first),
        "attempts should be different processes, got {seen:?}"
    );
}

#[tokio::test]
async fn no_observer_configured_is_fine() {
    let claude = Claude::builder().binary(fake()).build().unwrap();
    claude_wrapper::exec::run_claude(&claude, vec!["--version".into()])
        .await
        .expect("runs normally with no observer");
}
