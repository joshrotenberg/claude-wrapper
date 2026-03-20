//! GitHub platform adapter — issues, PRs, comments, labels.
//!
//! Uses the `gh` CLI for all GitHub operations. Handles auth,
//! pagination, and rate limiting transparently.

use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};

/// A normalized issue representation, independent of GitHub API shape.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IssueCandidate {
    pub number: u64,
    pub repo: String,
    pub title: String,
    pub body: String,
    pub labels: Vec<String>,
    pub state: String,
    pub created_at: String,
    pub updated_at: String,
    pub is_assigned: bool,
    pub html_url: String,
}

/// Minimal PR representation for tracking automation-owned PRs.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PullRequest {
    pub number: u64,
    pub head_branch: String,
    pub state: String,
    pub html_url: String,
}

// ── Raw gh CLI JSON shapes (private) ─────────────────────────────────

#[derive(Debug, Deserialize)]
struct GhIssue {
    number: u64,
    title: String,
    body: Option<String>,
    labels: Vec<GhLabel>,
    state: String,
    #[serde(rename = "createdAt")]
    created_at: String,
    #[serde(rename = "updatedAt")]
    updated_at: String,
    assignees: Vec<serde_json::Value>,
    url: String,
}

#[derive(Debug, Deserialize)]
struct GhLabel {
    name: String,
}

// GhPr can be used later for PR status checking.
#[derive(Debug, Deserialize)]
#[allow(dead_code)]
struct GhPr {
    number: u64,
    #[serde(rename = "headRefName")]
    head_ref_name: String,
    state: String,
    url: String,
}

// ── Public API ───────────────────────────────────────────────────────

/// Fetch an issue from GitHub.
pub async fn fetch_issue(repo: &str, number: u64) -> Result<IssueCandidate> {
    let output = tokio::process::Command::new("gh")
        .args([
            "issue",
            "view",
            "--repo",
            repo,
            &number.to_string(),
            "--json",
            "number,title,body,labels,state,createdAt,updatedAt,assignees,url",
        ])
        .output()
        .await
        .map_err(|e| Error::GitHub(format!("failed to run gh: {e}")))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(Error::GitHub(format!("gh issue view failed: {stderr}")));
    }

    let raw: GhIssue = serde_json::from_slice(&output.stdout)
        .map_err(|e| Error::GitHub(format!("failed to parse gh output: {e}")))?;

    Ok(IssueCandidate {
        number: raw.number,
        repo: repo.to_string(),
        title: raw.title,
        body: raw.body.unwrap_or_default(),
        labels: raw.labels.into_iter().map(|l| l.name).collect(),
        state: raw.state,
        created_at: raw.created_at,
        updated_at: raw.updated_at,
        is_assigned: !raw.assignees.is_empty(),
        html_url: raw.url,
    })
}

/// Fetch multiple open issues matching eligible labels.
pub async fn fetch_eligible_issues(
    repo: &str,
    labels: &[String],
    limit: usize,
) -> Result<Vec<IssueCandidate>> {
    let mut args = vec![
        "issue".to_string(),
        "list".to_string(),
        "--repo".to_string(),
        repo.to_string(),
        "--state".to_string(),
        "open".to_string(),
        "--json".to_string(),
        "number,title,body,labels,state,createdAt,updatedAt,assignees,url".to_string(),
        "--limit".to_string(),
        limit.to_string(),
    ];

    for label in labels {
        args.push("--label".to_string());
        args.push(label.clone());
    }

    let output = tokio::process::Command::new("gh")
        .args(&args)
        .output()
        .await
        .map_err(|e| Error::GitHub(format!("failed to run gh: {e}")))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(Error::GitHub(format!("gh issue list failed: {stderr}")));
    }

    let raw: Vec<GhIssue> = serde_json::from_slice(&output.stdout)
        .map_err(|e| Error::GitHub(format!("failed to parse gh output: {e}")))?;

    Ok(raw
        .into_iter()
        .map(|r| IssueCandidate {
            number: r.number,
            repo: repo.to_string(),
            title: r.title,
            body: r.body.unwrap_or_default(),
            labels: r.labels.into_iter().map(|l| l.name).collect(),
            state: r.state,
            created_at: r.created_at,
            updated_at: r.updated_at,
            is_assigned: !r.assignees.is_empty(),
            html_url: r.url,
        })
        .collect())
}

/// Fetch all open issues that have a specific label.
pub async fn fetch_issues_with_label(repo: &str, label: &str) -> Result<Vec<IssueCandidate>> {
    let output = tokio::process::Command::new("gh")
        .args([
            "issue",
            "list",
            "--repo",
            repo,
            "--state",
            "open",
            "--label",
            label,
            "--json",
            "number,title,body,labels,state,createdAt,updatedAt,assignees,url",
            "--limit",
            "100",
        ])
        .output()
        .await
        .map_err(|e| Error::GitHub(format!("failed to run gh: {e}")))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(Error::GitHub(format!(
            "gh issue list (label={label}) failed: {stderr}"
        )));
    }

    let raw: Vec<GhIssue> = serde_json::from_slice(&output.stdout)
        .map_err(|e| Error::GitHub(format!("failed to parse gh output: {e}")))?;

    Ok(raw
        .into_iter()
        .map(|r| IssueCandidate {
            number: r.number,
            repo: repo.to_string(),
            title: r.title,
            body: r.body.unwrap_or_default(),
            labels: r.labels.into_iter().map(|l| l.name).collect(),
            state: r.state,
            created_at: r.created_at,
            updated_at: r.updated_at,
            is_assigned: !r.assignees.is_empty(),
            html_url: r.url,
        })
        .collect())
}

/// Push a branch from a worktree to the remote.
pub async fn push_branch(work_dir: &Path, branch: &str) -> Result<()> {
    let output = tokio::process::Command::new("git")
        .args(["push", "-u", "origin", branch])
        .current_dir(work_dir)
        .output()
        .await
        .map_err(|e| Error::GitHub(format!("git push failed: {e}")))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(Error::GitHub(format!("git push failed: {stderr}")));
    }

    Ok(())
}

/// Create a pull request via gh CLI.
pub async fn create_pull_request(
    repo: &str,
    branch: &str,
    title: &str,
    body: &str,
    work_dir: &Path,
) -> Result<PullRequest> {
    let output = tokio::process::Command::new("gh")
        .args([
            "pr", "create", "--repo", repo, "--head", branch, "--title", title, "--body", body,
        ])
        .current_dir(work_dir)
        .output()
        .await
        .map_err(|e| Error::GitHub(format!("gh pr create failed: {e}")))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(Error::GitHub(format!("gh pr create failed: {stderr}")));
    }

    // gh pr create outputs the PR URL on stdout. Fetch details.
    let pr_url = String::from_utf8_lossy(&output.stdout).trim().to_string();

    // Extract PR number from URL.
    let pr_number = pr_url
        .rsplit('/')
        .next()
        .and_then(|s| s.parse::<u64>().ok())
        .unwrap_or(0);

    Ok(PullRequest {
        number: pr_number,
        head_branch: branch.to_string(),
        state: "open".to_string(),
        html_url: pr_url,
    })
}

/// Add a label to an issue.
pub async fn add_label(repo: &str, number: u64, label: &str) -> Result<()> {
    let output = tokio::process::Command::new("gh")
        .args([
            "issue",
            "edit",
            "--repo",
            repo,
            &number.to_string(),
            "--add-label",
            label,
        ])
        .output()
        .await
        .map_err(|e| Error::GitHub(format!("gh issue edit failed: {e}")))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(Error::GitHub(format!("add label failed: {stderr}")));
    }
    Ok(())
}

/// Remove a label from an issue.
pub async fn remove_label(repo: &str, number: u64, label: &str) -> Result<()> {
    let output = tokio::process::Command::new("gh")
        .args([
            "issue",
            "edit",
            "--repo",
            repo,
            &number.to_string(),
            "--remove-label",
            label,
        ])
        .output()
        .await
        .map_err(|e| Error::GitHub(format!("gh issue edit failed: {e}")))?;

    if !output.status.success() {
        tracing::debug!(label = label, "remove label failed (non-fatal)");
    }
    Ok(())
}

/// Post a comment on an issue.
pub async fn comment_on_issue(repo: &str, number: u64, body: &str) -> Result<()> {
    let output = tokio::process::Command::new("gh")
        .args([
            "issue",
            "comment",
            "--repo",
            repo,
            &number.to_string(),
            "--body",
            body,
        ])
        .output()
        .await
        .map_err(|e| Error::GitHub(format!("gh issue comment failed: {e}")))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(Error::GitHub(format!("gh issue comment failed: {stderr}")));
    }

    Ok(())
}
