//! Planner — turn an issue + workflow template into an execution plan.

use serde::{Deserialize, Serialize};

use crate::github::IssueCandidate;
use crate::workflow::{StageKind, WorkflowTemplate};

/// A work plan derived from an issue and workflow template.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkPlan {
    /// Stable plan identifier.
    pub plan_id: String,
    /// The issue being addressed.
    pub issue_number: u64,
    /// Repository.
    pub repo: String,
    /// Workflow template used.
    pub workflow: String,
    /// Ordered stages with per-stage context.
    pub stages: Vec<PlannedStage>,
    /// Branch name for this work.
    pub branch: String,
}

/// A stage in the work plan with attached context.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlannedStage {
    /// Stage kind.
    pub kind: StageKind,
    /// Agent prompt for this stage.
    pub prompt: String,
    /// Files this stage is allowed to modify (if known).
    pub allowed_files: Option<Vec<String>>,
    /// Validation commands to run after this stage.
    pub validation: Vec<String>,
    /// Whether this stage is optional.
    pub optional: bool,
    /// Maximum retries.
    pub max_retries: u32,
}

impl PlannedStage {
    /// Human-readable name for this stage kind.
    pub fn kind_name(&self) -> &'static str {
        match self.kind {
            StageKind::Triage => "triage",
            StageKind::Clarify => "clarify",
            StageKind::Plan => "plan",
            StageKind::Implement => "implement",
            StageKind::Test => "test",
            StageKind::Review => "review",
            StageKind::OpenPr => "open_pr",
            StageKind::RevisePr => "revise_pr",
            StageKind::FixCi => "fix_ci",
            StageKind::Merge => "merge",
            StageKind::Research => "research",
            StageKind::Comment => "comment",
        }
    }
}

/// Generate a work plan from an issue and workflow template.
pub fn create_plan(issue: &IssueCandidate, template: &WorkflowTemplate, branch: &str) -> WorkPlan {
    let stages = template
        .stages
        .iter()
        .map(|stage| {
            let prompt = generate_stage_prompt(stage.kind, issue);
            PlannedStage {
                kind: stage.kind,
                prompt,
                allowed_files: None,
                validation: Vec::new(),
                optional: stage.optional,
                max_retries: stage.max_retries,
            }
        })
        .collect();

    WorkPlan {
        plan_id: generate_plan_id(),
        issue_number: issue.number,
        repo: issue.repo.clone(),
        workflow: template.name.clone(),
        stages,
        branch: branch.to_string(),
    }
}

fn generate_stage_prompt(kind: StageKind, issue: &IssueCandidate) -> String {
    match kind {
        StageKind::Plan => format!(
            "Read issue #{} and analyze the codebase to create an implementation plan.\n\n\
             Issue title: {}\n\n\
             Issue body:\n{}\n\n\
             Research the relevant files, understand the current code, and write a detailed plan.",
            issue.number, issue.title, issue.body
        ),
        StageKind::Implement => format!(
            "Implement the changes for issue #{}.\n\n\
             Issue title: {}\n\n\
             Read the plan from the previous stage's breadcrumb for context.\n\
             Make the code changes, run tests, and commit.",
            issue.number, issue.title
        ),
        StageKind::Test => format!(
            "Run the test suite and fix any failures from the implementation of issue #{}.",
            issue.number
        ),
        StageKind::Review => format!(
            "Review the changes for issue #{}. Check for bugs, missing tests, \
             code quality issues. Document any findings.",
            issue.number
        ),
        StageKind::OpenPr => format!(
            "Create a pull request for issue #{}.\n\n\
             Push the branch and create a PR with a detailed description \
             linking to the issue.",
            issue.number
        ),
        StageKind::Clarify => format!(
            "Issue #{} may need clarification. Read the issue and identify \
             any ambiguities or missing information. Post clarifying questions \
             as a comment.",
            issue.number
        ),
        StageKind::Research => format!(
            "Research issue #{}: {}\n\n{}\n\n\
             Gather relevant information and write a summary.",
            issue.number, issue.title, issue.body
        ),
        StageKind::Comment => format!(
            "Post a summary comment on issue #{} with your findings.",
            issue.number
        ),
        StageKind::RevisePr | StageKind::FixCi | StageKind::Merge | StageKind::Triage => {
            format!(
                "Handle {} stage for issue #{}.",
                kind_name(kind),
                issue.number
            )
        }
    }
}

fn kind_name(kind: StageKind) -> &'static str {
    match kind {
        StageKind::Triage => "triage",
        StageKind::Clarify => "clarify",
        StageKind::Plan => "plan",
        StageKind::Implement => "implement",
        StageKind::Test => "test",
        StageKind::Review => "review",
        StageKind::OpenPr => "open_pr",
        StageKind::RevisePr => "revise_pr",
        StageKind::FixCi => "fix_ci",
        StageKind::Merge => "merge",
        StageKind::Research => "research",
        StageKind::Comment => "comment",
    }
}

fn generate_plan_id() -> String {
    let now = chrono::Utc::now();
    let timestamp = now.format("%Y%m%d-%H%M%S");
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos() as u64;
    let suffix = (nanos ^ (nanos >> 24)) & 0xFFFF_FFFF;
    format!("plan-{timestamp}-{suffix:08x}")
}
