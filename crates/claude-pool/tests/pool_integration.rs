#![cfg(unix)]
//! Integration tests for claude-pool using the fake-claude binary.
//!
//! These tests require the fake-claude.sh script (which behaves as a real binary).

mod helpers;

use std::time::Duration;

use claude_pool::PoolStore as _;
use claude_pool::{
    ChainIsolation, ChainOptions, ChainResult, ChainStep, Pool, StepAction,
    check_and_restart_slots,
    types::{SlotId, SlotState},
};
use helpers::{claude_with_fake_binary, fake_claude_path, write_env_wrapper};

// ── Shared helpers ────────────────────────────────────────────────────────────

fn make_prompt_step(name: &str, prompt: &str) -> ChainStep {
    ChainStep {
        name: name.into(),
        action: StepAction::Prompt {
            prompt: prompt.into(),
        },
        config: None,
        failure_policy: Default::default(),
        output_vars: Default::default(),
    }
}

fn no_isolation() -> ChainOptions {
    ChainOptions {
        isolation: ChainIsolation::None,
        ..Default::default()
    }
}

/// Poll `pool.result(task_id)` every 50 ms until it returns `Some`, or panic on timeout.
macro_rules! poll_result {
    ($pool:expr, $task_id:expr) => {
        poll_result!($pool, $task_id, 10)
    };
    ($pool:expr, $task_id:expr, $timeout_secs:expr) => {
        tokio::time::timeout(Duration::from_secs($timeout_secs), async {
            loop {
                if let Some(r) = $pool.result($task_id).await.unwrap() {
                    return r;
                }
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
        })
        .await
        .expect("task did not complete within timeout")
    };
}

// ── Tests ─────────────────────────────────────────────────────────────────────

/// Submit a task asynchronously, poll until complete, verify output/cost/session_id.
#[tokio::test]
async fn pool_submit_and_retrieve_result() {
    let claude = claude_with_fake_binary(&fake_claude_path());
    let pool = Pool::builder(claude).slots(1).build().await.unwrap();

    let task_id = pool.submit("hello world").await.unwrap();
    let result = poll_result!(pool, &task_id);

    assert!(result.success, "task failed: {}", result.output);
    assert_eq!(result.output, "fake response");
    assert_eq!(result.session_id.as_deref(), Some("fake-session-id"));
    assert_eq!(result.cost_microdollars, 0);
}

/// Run a 2-step chain. Both steps should complete, cost accumulates, step names preserved.
#[tokio::test]
async fn pool_chain_executes_all_steps() {
    let claude = claude_with_fake_binary(&fake_claude_path());
    let pool = Pool::builder(claude).slots(2).build().await.unwrap();

    let steps = vec![
        make_prompt_step("step1", "do thing one"),
        make_prompt_step("step2", "do thing two based on {previous_output}"),
    ];

    let task_id = pool.submit_chain(steps, no_isolation()).await.unwrap();

    let result = poll_result!(pool, &task_id);
    assert!(result.success, "chain task failed: {}", result.output);

    let chain: ChainResult = serde_json::from_str(&result.output).unwrap();
    assert!(chain.success, "chain not successful: {:?}", chain.steps);
    assert_eq!(chain.steps.len(), 2);
    assert_eq!(chain.steps[0].name, "step1");
    assert_eq!(chain.steps[1].name, "step2");
    assert!(chain.steps[0].success);
    assert!(chain.steps[1].success);
    assert_eq!(chain.total_cost_microdollars, 0);
    assert_eq!(chain.final_output, "fake response");
}

/// Step 1 returns JSON; output_vars extracts a field; step 2 uses it via template.
#[tokio::test]
async fn pool_chain_output_vars_flow() {
    // Wrapper that makes fake-claude output JSON with a "summary" field.
    let json_output = r#"{"summary": "all good"}"#;
    // The stream-json format wraps the OUTPUT variable in the result line.
    // The pool uses streaming internally, so FAKE_CLAUDE_OUTPUT sets the echoed text.
    let wrapper = write_env_wrapper(&[("FAKE_CLAUDE_OUTPUT", json_output)], &fake_claude_path());

    let claude = claude_with_fake_binary(wrapper.path());
    let pool = Pool::builder(claude).slots(2).build().await.unwrap();

    let mut output_vars = std::collections::HashMap::new();
    output_vars.insert("summary".to_string(), "summary".to_string());

    let steps = vec![
        ChainStep {
            name: "extract".into(),
            action: StepAction::Prompt {
                prompt: "produce json".into(),
            },
            config: None,
            failure_policy: Default::default(),
            output_vars,
        },
        make_prompt_step("use_result", "summary is: {steps.extract.summary}"),
    ];

    let task_id = pool.submit_chain(steps, no_isolation()).await.unwrap();

    let result = poll_result!(pool, &task_id);
    assert!(result.success, "chain failed: {}", result.output);

    let chain: ChainResult = serde_json::from_str(&result.output).unwrap();
    assert!(chain.success, "chain not successful: {:?}", chain.steps);
    assert_eq!(chain.steps.len(), 2);
    // Step 1 output should contain the JSON we returned.
    assert!(
        chain.steps[0].output.contains("all good"),
        "step1 output: {}",
        chain.steps[0].output
    );
    // Step 2 ran successfully (template expansion worked without error).
    assert!(chain.steps[1].success);
}

/// Submit a 3-step chain with a 1-second delay per step. Cancel after step 1 starts.
/// Remaining steps should be skipped.
#[tokio::test]
async fn pool_chain_cancellation_skips_remaining() {
    // Wrapper: each invocation sleeps 1 second before responding.
    let wrapper = write_env_wrapper(&[("FAKE_CLAUDE_DELAY", "1")], &fake_claude_path());

    let claude = claude_with_fake_binary(wrapper.path());
    let pool = Pool::builder(claude).slots(1).build().await.unwrap();

    let steps = vec![
        make_prompt_step("step1", "do step one"),
        make_prompt_step("step2", "do step two"),
        make_prompt_step("step3", "do step three"),
    ];

    let task_id = pool.submit_chain(steps, no_isolation()).await.unwrap();

    // Yield briefly — step1 starts but is blocked sleeping for 1 second.
    tokio::time::sleep(Duration::from_millis(200)).await;
    pool.cancel_chain(&task_id).await.unwrap();

    // Wait up to 5s for step1 to finish its sleep, then remaining steps are skipped.
    let result = poll_result!(pool, &task_id, 5);

    let chain: ChainResult = serde_json::from_str(&result.output).unwrap();
    assert!(
        !chain.success,
        "chain should not be successful after cancel"
    );

    let skipped: Vec<_> = chain.steps.iter().filter(|s| s.skipped).collect();
    assert!(
        !skipped.is_empty(),
        "expected at least one skipped step, got: {:?}",
        chain.steps
    );
    // The skipped steps should not have output.
    for s in &skipped {
        assert!(s.output.is_empty(), "skipped step had output: {}", s.output);
    }
}

/// Fan out 3 single-step chains. All should complete with distinct task IDs.
#[tokio::test]
async fn pool_fan_out_parallel() {
    let claude = claude_with_fake_binary(&fake_claude_path());
    let pool = Pool::builder(claude).slots(3).build().await.unwrap();

    let chains: Vec<Vec<ChainStep>> = (0..3)
        .map(|i| {
            vec![make_prompt_step(
                &format!("step{i}"),
                &format!("prompt {i}"),
            )]
        })
        .collect();

    let task_ids = pool.fan_out_chains(chains, no_isolation()).await.unwrap();

    assert_eq!(task_ids.len(), 3, "expected 3 task IDs");

    // All task IDs should be distinct.
    let unique: std::collections::HashSet<_> = task_ids.iter().map(|id| &id.0).collect();
    assert_eq!(unique.len(), 3, "task IDs not distinct: {:?}", task_ids);

    // All chains should complete successfully.
    for task_id in &task_ids {
        let result = poll_result!(pool, task_id);
        assert!(
            result.success,
            "chain {} failed: {}",
            task_id.0, result.output
        );

        let chain: ChainResult = serde_json::from_str(&result.output).unwrap();
        assert!(chain.success);
        assert_eq!(chain.steps.len(), 1);
    }
}

/// Chain with Worktree isolation: worktree is created during execution and cleaned up after.
#[tokio::test]
async fn pool_chain_worktree_creates_and_cleans() {
    let repo = helpers::temp_git_repo();
    let fake = fake_claude_path();

    // Build Claude pointing at the temp git repo so the pool uses it as repo root.
    let claude = claude_wrapper::Claude::builder()
        .binary(&fake)
        .working_dir(repo.path())
        .build()
        .unwrap();

    let pool = Pool::builder(claude).slots(1).build().await.unwrap();

    let steps = vec![make_prompt_step("step1", "hello from worktree")];
    let task_id = pool
        .submit_chain(
            steps,
            ChainOptions {
                isolation: ChainIsolation::Worktree,
                ..Default::default()
            },
        )
        .await
        .unwrap();

    // Record the expected worktree path before we wait for completion.
    let wt_path = {
        let base = std::env::temp_dir().join("claude-pool").join("worktrees");
        base.join("chains").join(&task_id.0)
    };

    let result = poll_result!(pool, &task_id);
    assert!(result.success, "chain failed: {}", result.output);

    let chain: ChainResult = serde_json::from_str(&result.output).unwrap();
    assert!(chain.success);

    // After completion, the chain worktree should have been removed.
    assert!(
        !wt_path.exists(),
        "chain worktree was not cleaned up: {}",
        wt_path.display()
    );

    // Confirm via git as well.
    let git_out = std::process::Command::new("git")
        .args(["worktree", "list"])
        .current_dir(repo.path())
        .output()
        .unwrap();
    let worktree_list = String::from_utf8_lossy(&git_out.stdout);
    assert!(
        !worktree_list.contains(&task_id.0),
        "chain worktree still listed in git: {worktree_list}"
    );
}

/// Mark a slot as errored, run check_and_restart_slots, verify recovery. Then run a task.
#[tokio::test]
async fn supervisor_restarts_errored_slot_integration() {
    let claude = claude_with_fake_binary(&fake_claude_path());
    let pool = Pool::builder(claude).slots(2).build().await.unwrap();

    let slot_id = SlotId("slot-0".into());

    // Mark slot-0 as errored.
    let mut slot = pool
        .store()
        .get_slot(&slot_id)
        .await
        .unwrap()
        .expect("slot-0 not found");
    slot.state = SlotState::Errored;
    pool.store().put_slot(slot).await.unwrap();

    // Run one supervisor check pass.
    let restarted = check_and_restart_slots(&pool).await;
    assert_eq!(restarted, 1, "expected 1 slot restarted");

    // Slot should now be idle with restart_count incremented.
    let slot = pool
        .store()
        .get_slot(&slot_id)
        .await
        .unwrap()
        .expect("slot-0 not found after restart");
    assert_eq!(slot.state, SlotState::Idle);
    assert_eq!(slot.restart_count, 1);
    assert!(slot.session_id.is_none());

    // Pool should accept tasks on the recovered slot.
    let result = pool.run("task after supervisor restart").await.unwrap();
    assert!(result.success);
    assert_eq!(result.output, "fake response");
}
