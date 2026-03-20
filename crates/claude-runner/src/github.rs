//! GitHub platform adapter — issues, PRs, comments, labels.
//!
//! Isolates all GitHub-specific behavior. The domain layer never touches
//! GitHub API shapes directly.

use serde::{Deserialize, Serialize};

/// A normalized issue representation, independent of GitHub API shape.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IssueCandidate {
    /// Issue number.
    pub number: u64,
    /// Repository in `owner/name` format.
    pub repo: String,
    /// Issue title.
    pub title: String,
    /// Issue body/description.
    pub body: String,
    /// Labels on the issue.
    pub labels: Vec<String>,
    /// Issue state (open, closed).
    pub state: String,
    /// When the issue was created.
    pub created_at: String,
    /// When the issue was last updated.
    pub updated_at: String,
    /// Whether the issue is assigned to someone.
    pub is_assigned: bool,
    /// URL for linking back.
    pub html_url: String,
}

/// Minimal PR representation for tracking automation-owned PRs.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PullRequest {
    /// PR number.
    pub number: u64,
    /// Source branch.
    pub head_branch: String,
    /// PR state.
    pub state: String,
    /// Whether CI checks have passed.
    pub checks_passing: bool,
    /// Whether there are requested changes.
    pub changes_requested: bool,
    /// Whether the PR is mergeable.
    pub mergeable: bool,
}
