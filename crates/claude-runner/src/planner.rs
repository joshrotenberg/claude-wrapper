//! Planner — turn an issue + workflow template into an execution plan.

use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::config::RunnerConfig;
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
    /// Shell command that gates execution. Exit 0 = run, non-zero = skip.
    pub condition: Option<String>,
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

/// Generate a work plan from an issue, workflow template, and optional repo policy.
pub fn create_plan(
    issue: &IssueCandidate,
    template: &WorkflowTemplate,
    branch: &str,
    policy: Option<&crate::policy::RepoPolicy>,
) -> WorkPlan {
    create_plan_with_config(issue, template, branch, policy, None)
}

/// Generate a work plan, optionally loading stage prompts from files configured in
/// `RunnerConfig.prompt_templates`. File-based overrides take precedence over
/// `RepoPolicy.stage_prompts` in-memory overrides.
///
/// `config` is a tuple of `(RunnerConfig, base_dir)` where `base_dir` is the directory
/// used to resolve relative file paths (typically the directory containing `runner.toml`).
pub fn create_plan_with_config(
    issue: &IssueCandidate,
    template: &WorkflowTemplate,
    branch: &str,
    policy: Option<&crate::policy::RepoPolicy>,
    config: Option<(&RunnerConfig, &Path)>,
) -> WorkPlan {
    let stages = template
        .stages
        .iter()
        .map(|stage| {
            let stage_name = kind_name(stage.kind);
            let prompt = if let Some((cfg, base_dir)) = config
                && let Some(file_path) = cfg.prompt_templates.get(stage_name)
                && let Some(content) = load_prompt_template(file_path, base_dir, issue, branch)
            {
                content
            } else if let Some(override_prompt) =
                policy.and_then(|p| p.stage_prompts.get(stage_name))
            {
                override_prompt.clone()
            } else {
                let validation_cmds = policy
                    .map(|p| p.validation_commands.clone())
                    .unwrap_or_default();
                generate_stage_prompt(stage.kind, issue, &validation_cmds)
            };
            let validation = policy
                .map(|p| p.validation_commands.clone())
                .unwrap_or_default();
            PlannedStage {
                kind: stage.kind,
                prompt,
                allowed_files: None,
                validation,
                optional: stage.optional,
                max_retries: stage.max_retries,
                condition: stage.condition.clone(),
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

/// Load a prompt template file and substitute issue/branch variables.
///
/// Returns `None` if the file cannot be read (logs a warning in that case).
fn load_prompt_template(
    file_path: &str,
    base_dir: &Path,
    issue: &IssueCandidate,
    branch: &str,
) -> Option<String> {
    let full_path = base_dir.join(file_path);
    match std::fs::read_to_string(&full_path) {
        Ok(content) => Some(
            content
                .replace("{issue_number}", &issue.number.to_string())
                .replace("{issue_title}", &issue.title)
                .replace("{issue_body}", &issue.body)
                .replace("{branch}", branch)
                .replace("{repo}", &issue.repo),
        ),
        Err(e) => {
            tracing::warn!(
                path = %full_path.display(),
                error = %e,
                "failed to load prompt template file, falling back to built-in prompt"
            );
            None
        }
    }
}

fn generate_stage_prompt(
    kind: StageKind,
    issue: &IssueCandidate,
    validation_commands: &[String],
) -> String {
    match kind {
        StageKind::Plan => format!(
            "Read issue #{number} and analyze the codebase to create an implementation plan.\n\n\
             Issue title: {title}\n\n\
             Issue body:\n{body}\n\n\
             ## Steps\n\n\
             1. Search for and read the relevant files — do not guess at file locations.\n\
             2. Understand the current architecture and patterns used.\n\
             3. Identify exactly which files need to change and why.\n\n\
             ## Breadcrumb\n\n\
             Write the plan to `.plan_breadcrumb.md` in the repo root with these sections:\n\
             - **Files to modify**: list each file path, one per line\n\
             - **Approach**: 2-5 sentence summary of what will change and why\n\
             - **Commit message**: the exact conventional-commit message for the implement stage \
               (e.g., `feat(module): short description closes #{number}`)\n\n\
             Do NOT modify any source files. This is a planning-only stage.",
            number = issue.number,
            title = issue.title,
            body = issue.body,
        ),
        StageKind::Implement => format!(
            "Implement the changes for issue #{number}.\n\n\
             Issue title: {title}\n\n\
             ## Context\n\n\
             Read the plan breadcrumb at `.plan_breadcrumb.md` for the list of files to modify \
             and the approach decided in the plan stage.\n\n\
             ## Instructions\n\n\
             1. Only modify the files listed in the breadcrumb. Do NOT touch any other files.\n\
             2. Follow the existing code patterns and style.\n\
             3. For Rust code: use Rust 2024 if-let chains — write \
               `if let Some(x) = y && condition {{` instead of nested if-let/if blocks.\n\
             4. After making changes, run `cargo fmt --all` to format the code.\n\
             5. Commit using the exact commit message from the breadcrumb.\n\n\
             Do NOT create a PR in this stage.",
            number = issue.number,
            title = issue.title,
        ),
        StageKind::Test => {
            let cmds = if validation_commands.is_empty() {
                "cargo test --lib --all-features".to_string()
            } else {
                validation_commands
                    .iter()
                    .map(|c| format!("- `{c}`"))
                    .collect::<Vec<_>>()
                    .join("\n")
            };
            format!(
                "Verify the implementation from issue #{number} passes all checks.\n\n\
                 ## Validation commands\n\n\
                 Run each of the following and confirm they pass:\n\
                 {cmds}\n\n\
                 If any command fails, fix the issue and re-run until all pass.\n\
                 Do NOT modify implementation logic — only fix formatting, clippy warnings, \
                 or test failures caused by missing test coverage.",
                number = issue.number,
                cmds = cmds,
            )
        }
        StageKind::Review => format!(
            "Review the changes for issue #{number}. This is a read-only verification stage.\n\n\
             ## What to check\n\n\
             - Correctness: does the implementation match the plan?\n\
             - Tests: are there tests for new behavior?\n\
             - Code quality: any obvious bugs, panics, or unsafe patterns?\n\
             - Consistency: does the style match the surrounding code?\n\n\
             ## Output format\n\n\
             Write a structured review to `.review_breadcrumb.md`:\n\n\
             ```\n\
             ## Review: issue #{number}\n\n\
             ### Issues found\n\n\
             | Severity | File | Line | Description |\n\
             |----------|------|------|-------------|\n\
             | high | src/foo.rs | 42 | description |\n\n\
             ### Verdict: PASS / FAIL\n\
             ```\n\n\
             PASS if no high-severity issues found; FAIL otherwise.\n\
             Do NOT modify any source files in this stage.",
            number = issue.number,
        ),
        StageKind::OpenPr => {
            let test_plan = if validation_commands.is_empty() {
                "- [ ] cargo test passes".to_string()
            } else {
                validation_commands
                    .iter()
                    .map(|c| format!("- [ ] `{c}`"))
                    .collect::<Vec<_>>()
                    .join("\n")
            };
            format!(
                "Create a pull request for issue #{number}.\n\n\
                 ## Steps\n\n\
                 1. Push the branch to origin if not already pushed.\n\
                 2. Read `.plan_breadcrumb.md` for the commit message and files changed.\n\
                 3. Read `.review_breadcrumb.md` if it exists for the review verdict.\n\
                 4. Create the PR using the template below.\n\n\
                 ## PR template\n\n\
                 ```\n\
                 gh pr create \\\n\
                   --title \"<commit message from plan breadcrumb>\" \\\n\
                   --body \"$(cat <<'EOF'\n\
                 ## Summary\n\
                 <2-4 bullet points describing what changed and why>\n\n\
                 ## Files changed\n\
                 <list each modified file with a one-line description>\n\n\
                 ## Test plan\n\
                 {test_plan}\n\n\
                 Closes #{number}\n\
                 EOF\n\
                 )\"\n\
                 ```\n\n\
                 Do NOT merge the PR.",
                number = issue.number,
                test_plan = test_plan,
            )
        }
        StageKind::Clarify => format!(
            "Issue #{number} may need clarification before implementation can begin.\n\n\
             Read the issue and identify any ambiguities or missing information that would \
             block a clean implementation. Post clarifying questions as a comment on the issue.\n\n\
             Issue title: {title}\n\n\
             Issue body:\n{body}",
            number = issue.number,
            title = issue.title,
            body = issue.body,
        ),
        StageKind::Research => format!(
            "Research issue #{number}: {title}\n\n\
             {body}\n\n\
             Gather relevant information, read related code and documentation, \
             and write a summary of findings. Save the summary to `.research_breadcrumb.md`.",
            number = issue.number,
            title = issue.title,
            body = issue.body,
        ),
        StageKind::Comment => format!(
            "Post a summary comment on issue #{number} with your findings.\n\n\
             Read `.research_breadcrumb.md` if it exists for prior research output. \
             Write a clear, structured comment that summarizes the findings and \
             any recommendations.",
            number = issue.number,
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::github::IssueCandidate;

    fn make_issue(number: u64, title: &str) -> IssueCandidate {
        IssueCandidate {
            number,
            repo: "owner/repo".to_string(),
            title: title.to_string(),
            body: "issue body".to_string(),
            labels: vec![],
            state: "open".to_string(),
            created_at: String::new(),
            updated_at: String::new(),
            is_assigned: false,
            html_url: String::new(),
        }
    }

    fn make_policy(validation_commands: Vec<String>) -> crate::policy::RepoPolicy {
        crate::policy::RepoPolicy {
            repo: "owner/repo".to_string(),
            eligible_labels: vec![],
            exclude_labels: vec![],
            workflows: Default::default(),
            branch_pattern: "automation/{issue}-{slug}".to_string(),
            max_concurrency: 3,
            concurrency: Default::default(),
            auto_merge: false,
            agent: "claude".to_string(),
            model: None,
            validation_commands,
            stage_prompts: Default::default(),
            default_workflow: None,
            workflow_templates: vec![],
        }
    }

    #[test]
    fn plan_prompt_contains_issue_number_and_breadcrumb_instruction() {
        let issue = make_issue(42, "feat: add something");
        let prompt = generate_stage_prompt(StageKind::Plan, &issue, &[]);
        assert!(prompt.contains("#42"));
        assert!(prompt.contains(".plan_breadcrumb.md"));
        assert!(prompt.contains("Commit message"));
        assert!(prompt.contains("Do NOT modify any source files"));
    }

    #[test]
    fn implement_prompt_references_breadcrumb_and_fmt() {
        let issue = make_issue(42, "feat: add something");
        let prompt = generate_stage_prompt(StageKind::Implement, &issue, &[]);
        assert!(prompt.contains("#42"));
        assert!(prompt.contains(".plan_breadcrumb.md"));
        assert!(prompt.contains("cargo fmt --all"));
        assert!(prompt.contains("Do NOT create a PR"));
    }

    #[test]
    fn test_prompt_uses_validation_commands() {
        let issue = make_issue(42, "feat: add something");
        let cmds = vec!["cargo test --lib".to_string(), "cargo clippy".to_string()];
        let prompt = generate_stage_prompt(StageKind::Test, &issue, &cmds);
        assert!(prompt.contains("cargo test --lib"));
        assert!(prompt.contains("cargo clippy"));
    }

    #[test]
    fn test_prompt_falls_back_when_no_validation_commands() {
        let issue = make_issue(42, "feat: add something");
        let prompt = generate_stage_prompt(StageKind::Test, &issue, &[]);
        assert!(prompt.contains("cargo test --lib --all-features"));
    }

    #[test]
    fn review_prompt_is_read_only_with_structured_output() {
        let issue = make_issue(42, "feat: add something");
        let prompt = generate_stage_prompt(StageKind::Review, &issue, &[]);
        assert!(prompt.contains(".review_breadcrumb.md"));
        assert!(prompt.contains("Severity"));
        assert!(prompt.contains("PASS / FAIL"));
        assert!(prompt.contains("Do NOT modify any source files"));
    }

    #[test]
    fn open_pr_prompt_contains_test_plan_and_closes() {
        let issue = make_issue(42, "feat: add something");
        let cmds = vec!["cargo test --lib".to_string()];
        let prompt = generate_stage_prompt(StageKind::OpenPr, &issue, &cmds);
        assert!(prompt.contains("Closes #42"));
        assert!(prompt.contains("cargo test --lib"));
        assert!(prompt.contains(".plan_breadcrumb.md"));
        assert!(prompt.contains("Do NOT merge"));
    }

    #[test]
    fn stage_prompt_override_is_used_from_policy() {
        let issue = make_issue(42, "feat: add something");
        let mut policy = make_policy(vec![]);
        policy
            .stage_prompts
            .insert("plan".to_string(), "custom plan prompt".to_string());
        let template = crate::workflow::builtin_templates()
            .into_iter()
            .find(|t| t.name == "feature")
            .unwrap();
        let plan = create_plan(
            &issue,
            &template,
            "automation/42-feat-add-something",
            Some(&policy),
        );
        let plan_stage = plan
            .stages
            .iter()
            .find(|s| s.kind == StageKind::Plan)
            .unwrap();
        assert_eq!(plan_stage.prompt, "custom plan prompt");
    }

    #[test]
    fn validation_commands_propagated_to_planned_stages() {
        let issue = make_issue(42, "feat: add something");
        let policy = make_policy(vec!["cargo test --lib".to_string()]);
        let template = crate::workflow::builtin_templates()
            .into_iter()
            .find(|t| t.name == "feature")
            .unwrap();
        let plan = create_plan(
            &issue,
            &template,
            "automation/42-feat-add-something",
            Some(&policy),
        );
        for stage in &plan.stages {
            assert_eq!(stage.validation, vec!["cargo test --lib"]);
        }
    }
}
