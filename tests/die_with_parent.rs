//! The parent-death policy: does the option plumb through, and on Linux does
//! the child actually die with its parent.
//!
//! The behavioural half is inherently a multi-process test: it spawns a helper
//! that itself spawns a child through this crate, SIGKILLs the helper, and asks
//! whether the grandchild survived. It is `#[ignore]`d off Linux, where
//! `PR_SET_PDEATHSIG` has no equivalent and the answer is "it survives, by
//! design".

#![cfg(all(feature = "async", feature = "json"))]

use std::sync::{Arc, Mutex};

use claude_wrapper::Claude;

fn fake() -> &'static str {
    concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fake-claude.sh")
}

#[test]
fn support_is_reported_honestly_per_platform() {
    // The whole point of exposing this: a caller must be able to tell whether
    // the option did anything, rather than assuming coverage it lacks.
    assert_eq!(
        claude_wrapper::exec::die_with_parent_supported(),
        cfg!(target_os = "linux"),
        "support must track the platform, not the build"
    );
}

#[tokio::test]
async fn enabling_it_does_not_disturb_a_normal_run() {
    // Off Linux this proves the no-op is truly a no-op; on Linux it proves the
    // pre_exec hook does not break an ordinary spawn.
    let claude = Claude::builder()
        .binary(fake())
        .die_with_parent(true)
        .build()
        .unwrap();

    let out = claude_wrapper::exec::run_claude(&claude, vec!["--version".into()])
        .await
        .expect("a run with die_with_parent enabled still succeeds");
    assert!(!out.stdout.is_empty());
}

#[tokio::test]
async fn it_composes_with_the_spawn_observer() {
    // A supervisor will use both: pdeathsig for the common case, the pid for
    // the platforms and crash modes it cannot cover.
    let seen = Arc::new(Mutex::new(Vec::new()));
    let sink = seen.clone();
    let claude = Claude::builder()
        .binary(fake())
        .die_with_parent(true)
        .on_spawn(Arc::new(move |info| sink.lock().unwrap().push(info)))
        .build()
        .unwrap();

    claude_wrapper::exec::run_claude(&claude, vec!["--version".into()])
        .await
        .unwrap();

    assert_eq!(seen.lock().unwrap().len(), 1, "observer still fires");
}

#[test]
fn default_is_off() {
    // Killing children on parent exit is right for a supervisor and wrong for a
    // CLI that deliberately backgrounds work, so it must be opt-in.
    let claude = Claude::builder().binary(fake()).build().unwrap();
    let rendered = format!("{claude:?}");
    assert!(
        !rendered.contains("die_with_parent: true"),
        "default must be off, got {rendered}"
    );
}

/// Set on the re-executed copy of this binary that plays the middle process.
const HELPER_ENV: &str = "CLAUDE_WRAPPER_PDEATHSIG_HELPER";

/// The middle process: spawns a long-lived child through this crate with the
/// policy enabled, prints its pid, then blocks so it can be killed.
///
/// A no-op in ordinary runs; only the re-exec below sets `HELPER_ENV`.
#[test]
fn pdeathsig_helper_process() {
    if std::env::var(HELPER_ENV).is_err() {
        return;
    }
    let claude = Claude::builder()
        // `sleep` stands in for the CLI: what matters is a child that outlives
        // its parent unless something kills it.
        .binary("/bin/sleep")
        .die_with_parent(true)
        .on_spawn(Arc::new(|info| {
            // The parent reads this to learn what to watch.
            println!("PID {}", info.pid);
            use std::io::Write;
            let _ = std::io::stdout().flush();
        }))
        .build()
        .unwrap();

    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    // Blocks until killed, which is the point.
    let _ = rt.block_on(claude_wrapper::exec::run_claude(
        &claude,
        vec!["300".into()],
    ));
}

/// The behavioural test, and the only one that proves the feature works.
///
/// Linux-gated rather than `#[ignore]`d, so CI actually runs it on the one
/// platform where `PR_SET_PDEATHSIG` exists. Elsewhere it is compiled out: the
/// documented answer there is "the child survives", which is exactly why
/// `die_with_parent_supported` exists.
#[cfg(target_os = "linux")]
#[test]
fn child_dies_when_the_parent_is_sigkilled() {
    use std::io::{BufRead, BufReader};
    use std::process::{Command, Stdio};

    let exe = std::env::current_exe().expect("test binary path");
    let mut helper = Command::new(&exe)
        .args(["--exact", "pdeathsig_helper_process", "--nocapture"])
        .env(HELPER_ENV, "1")
        .stdout(Stdio::piped())
        .spawn()
        .expect("spawning the helper");

    let stdout = helper.stdout.take().expect("piped");
    let mut pid = None;
    for line in BufReader::new(stdout).lines().map_while(Result::ok) {
        if let Some(rest) = line.strip_prefix("PID ") {
            pid = rest.trim().parse::<u32>().ok();
            break;
        }
    }
    let pid = pid.expect("helper reported a child pid");

    // SIGKILL: no destructor in the helper can run, which is the scenario.
    unsafe { libc::kill(helper.id() as i32, libc::SIGKILL) };
    let _ = helper.wait();

    // The kernel delivers the death signal promptly, but not instantly.
    let mut alive = true;
    for _ in 0..50 {
        std::thread::sleep(std::time::Duration::from_millis(100));
        // SAFETY: signal 0 is an existence check only.
        if unsafe { libc::kill(pid as i32, 0) } != 0 {
            alive = false;
            break;
        }
    }
    assert!(
        !alive,
        "child {pid} survived its SIGKILLed parent; PR_SET_PDEATHSIG did not fire"
    );
}
