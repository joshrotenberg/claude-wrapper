#![cfg(unix)]
//! Integration tests for git worktree management.

mod helpers;

use claude_pool::{WorktreeManager, types::SlotId};

/// Create a worktree for a slot, verify the path exists, then remove it and
/// verify the path is gone.
#[tokio::test]
async fn worktree_create_and_remove_round_trip() {
    let repo = helpers::temp_git_repo();
    let base_dir = tempfile::tempdir().expect("temp dir");

    let mgr = WorktreeManager::new(repo.path(), Some(base_dir.path().to_path_buf()));
    let slot_id = SlotId("slot-test-0".into());

    let wt_path = mgr.create(&slot_id).await.expect("worktree create");
    assert!(wt_path.exists(), "worktree path should exist after create");
    assert!(
        wt_path.join(".git").exists(),
        "worktree should contain a .git file"
    );

    mgr.remove(&slot_id).await.expect("worktree remove");
    assert!(
        !wt_path.exists(),
        "worktree path should be gone after remove"
    );
}

/// Create a chain worktree, verify it exists, then remove it.
#[tokio::test]
async fn chain_worktree_create_and_remove_round_trip() {
    let repo = helpers::temp_git_repo();
    let base_dir = tempfile::tempdir().expect("temp dir");

    let mgr = WorktreeManager::new(repo.path(), Some(base_dir.path().to_path_buf()));
    let task_id = claude_pool::types::TaskId("chain-abc123".into());

    let wt_path = mgr
        .create_for_chain(&task_id)
        .await
        .expect("chain worktree create");
    assert!(wt_path.exists(), "chain worktree path should exist");

    mgr.remove_chain(&task_id)
        .await
        .expect("chain worktree remove");
    assert!(!wt_path.exists(), "chain worktree path should be gone");
}
