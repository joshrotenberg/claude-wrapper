//! Triage engine — evaluate issue readiness and decide next action.

use serde::{Deserialize, Serialize};

use crate::github::IssueCandidate;
use crate::policy::RepoPolicy;

/// The outcome of triaging an issue.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum TriageDecision {
    /// Issue is ready to plan and execute.
    Ready,
    /// Issue needs clarification — includes questions to ask.
    NeedsClarification(Vec<String>),
    /// Issue is blocked by another issue or external dependency.
    Blocked(String),
    /// Issue is a duplicate.
    Duplicate(u64),
    /// Issue is out of scope for automation.
    OutOfScope(String),
    /// Issue is already in progress (has an active lease or PR).
    AlreadyInProgress,
}

/// Evaluate whether an issue is ready for automation.
pub fn triage(issue: &IssueCandidate, policy: &RepoPolicy) -> TriageDecision {
    // Check exclusion labels.
    for label in &issue.labels {
        if policy.exclude_labels.contains(label) {
            return TriageDecision::OutOfScope(format!("excluded by label: {label}"));
        }
    }

    // Check eligibility labels (if configured, at least one must match).
    if !policy.eligible_labels.is_empty()
        && !issue
            .labels
            .iter()
            .any(|l| policy.eligible_labels.contains(l))
    {
        return TriageDecision::OutOfScope("no eligible label".to_string());
    }

    // Check if already assigned.
    if issue.is_assigned {
        return TriageDecision::AlreadyInProgress;
    }

    // Check body quality — very basic heuristic.
    if issue.body.trim().len() < 20 {
        return TriageDecision::NeedsClarification(vec![
            "The issue description is too brief. Can you provide more detail about the expected behavior and acceptance criteria?".to_string(),
        ]);
    }

    TriageDecision::Ready
}
