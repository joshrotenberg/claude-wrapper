//! Workflow templates — configurable stage chains per issue type.

use serde::{Deserialize, Serialize};

/// A named workflow template defining the stage chain for a type of work.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkflowTemplate {
    /// Template name (e.g., "bug", "feature", "research", "chore").
    pub name: String,
    /// Ordered stages to execute.
    pub stages: Vec<Stage>,
}

/// A stage in a workflow — a bounded unit of work.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Stage {
    /// Stage identifier.
    pub kind: StageKind,
    /// Whether this stage is optional (can be skipped).
    #[serde(default)]
    pub optional: bool,
    /// Maximum retries for this stage.
    #[serde(default = "default_retries")]
    pub max_retries: u32,
    /// Timeout in seconds for this stage.
    pub timeout_secs: Option<u64>,
    /// Shell command that gates execution. Exit 0 = run, non-zero = skip.
    /// Evaluated with RUNNER_ISSUE_NUMBER, RUNNER_ISSUE_TITLE, RUNNER_ISSUE_BODY,
    /// and RUNNER_ISSUE_LABELS env vars available.
    #[serde(default)]
    pub condition: Option<String>,
}

fn default_retries() -> u32 {
    2
}

/// Known stage kinds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StageKind {
    /// Evaluate issue readiness.
    Triage,
    /// Ask for missing information.
    Clarify,
    /// Create an implementation plan.
    Plan,
    /// Write code changes.
    Implement,
    /// Run tests and validation.
    Test,
    /// Review changes for quality.
    Review,
    /// Create or update a pull request.
    OpenPr,
    /// Address PR review feedback.
    RevisePr,
    /// Fix CI failures.
    FixCi,
    /// Merge the PR.
    Merge,
    /// Produce a research report (comment on issue, no PR).
    Research,
    /// Post a summary comment on the issue.
    Comment,
}

/// Select a workflow template for an issue based on labels and policy.
///
/// Resolution order:
/// 1. For each issue label, check `policy.workflows` for a template name; if
///    found, resolve it against custom then built-in templates.
/// 2. Fall back to `policy.default_workflow` (if set) or `"feature"`, again
///    resolved against custom then built-in templates.
pub fn select_workflow(
    issue: &crate::github::IssueCandidate,
    policy: &crate::policy::RepoPolicy,
) -> WorkflowTemplate {
    // Merge custom and built-in templates; custom templates shadow built-ins.
    let all: Vec<WorkflowTemplate> = {
        let mut v = policy.workflow_templates.clone();
        for builtin in builtin_templates() {
            if !v.iter().any(|t| t.name == builtin.name) {
                v.push(builtin);
            }
        }
        v
    };

    // Check policy workflow label mappings first.
    for label in &issue.labels {
        if let Some(template_name) = policy.workflows.get(label)
            && let Some(template) = all.iter().find(|t| t.name == *template_name)
        {
            return template.clone();
        }
    }

    // Fall back to configured default or "feature".
    let default_name = policy.default_workflow.as_deref().unwrap_or("feature");
    all.iter()
        .find(|t| t.name == default_name)
        .cloned()
        .unwrap_or_else(|| {
            builtin_templates()
                .into_iter()
                .find(|t| t.name == "feature")
                .unwrap()
        })
}

/// Built-in workflow templates.
pub fn builtin_templates() -> Vec<WorkflowTemplate> {
    vec![
        WorkflowTemplate {
            name: "bug".into(),
            stages: vec![
                Stage {
                    kind: StageKind::Plan,
                    optional: false,
                    max_retries: 1,
                    timeout_secs: None,
                    condition: None,
                },
                Stage {
                    kind: StageKind::Implement,
                    optional: false,
                    max_retries: 2,
                    timeout_secs: None,
                    condition: None,
                },
                Stage {
                    kind: StageKind::Test,
                    optional: false,
                    max_retries: 2,
                    timeout_secs: None,
                    condition: None,
                },
                Stage {
                    kind: StageKind::Review,
                    optional: true,
                    max_retries: 1,
                    timeout_secs: None,
                    condition: None,
                },
                Stage {
                    kind: StageKind::OpenPr,
                    optional: false,
                    max_retries: 1,
                    timeout_secs: None,
                    condition: None,
                },
            ],
        },
        WorkflowTemplate {
            name: "feature".into(),
            stages: vec![
                Stage {
                    kind: StageKind::Plan,
                    optional: false,
                    max_retries: 1,
                    timeout_secs: None,
                    condition: None,
                },
                Stage {
                    kind: StageKind::Implement,
                    optional: false,
                    max_retries: 2,
                    timeout_secs: None,
                    condition: None,
                },
                Stage {
                    kind: StageKind::Test,
                    optional: false,
                    max_retries: 2,
                    timeout_secs: None,
                    condition: None,
                },
                Stage {
                    kind: StageKind::Review,
                    optional: true,
                    max_retries: 1,
                    timeout_secs: None,
                    condition: None,
                },
                Stage {
                    kind: StageKind::OpenPr,
                    optional: false,
                    max_retries: 1,
                    timeout_secs: None,
                    condition: None,
                },
            ],
        },
        WorkflowTemplate {
            name: "chore".into(),
            stages: vec![
                Stage {
                    kind: StageKind::Implement,
                    optional: false,
                    max_retries: 2,
                    timeout_secs: None,
                    condition: None,
                },
                Stage {
                    kind: StageKind::Test,
                    optional: false,
                    max_retries: 2,
                    timeout_secs: None,
                    condition: None,
                },
                Stage {
                    kind: StageKind::OpenPr,
                    optional: false,
                    max_retries: 1,
                    timeout_secs: None,
                    condition: None,
                },
            ],
        },
        WorkflowTemplate {
            name: "research".into(),
            stages: vec![
                Stage {
                    kind: StageKind::Research,
                    optional: false,
                    max_retries: 1,
                    timeout_secs: None,
                    condition: None,
                },
                Stage {
                    kind: StageKind::Comment,
                    optional: false,
                    max_retries: 1,
                    timeout_secs: None,
                    condition: None,
                },
            ],
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn feature_template_has_no_clarify_stage() {
        let feature = builtin_templates()
            .into_iter()
            .find(|t| t.name == "feature")
            .expect("feature template must exist");
        assert!(
            !feature.stages.iter().any(|s| s.kind == StageKind::Clarify),
            "feature template should not include a Clarify stage by default"
        );
    }

    #[test]
    fn feature_template_starts_with_plan() {
        let feature = builtin_templates()
            .into_iter()
            .find(|t| t.name == "feature")
            .expect("feature template must exist");
        assert_eq!(
            feature.stages[0].kind,
            StageKind::Plan,
            "feature template should start with Plan"
        );
    }

    #[test]
    fn stage_condition_field_defaults_to_none() {
        let stage = Stage {
            kind: StageKind::Review,
            optional: true,
            max_retries: 1,
            timeout_secs: None,
            condition: None,
        };
        assert!(stage.condition.is_none());
    }

    #[test]
    fn stage_condition_round_trips_via_serde() {
        let stage = Stage {
            kind: StageKind::Review,
            optional: true,
            max_retries: 1,
            timeout_secs: None,
            condition: Some("test -n \"$RUNNER_ISSUE_BODY\"".into()),
        };
        let json = serde_json::to_string(&stage).unwrap();
        let restored: Stage = serde_json::from_str(&json).unwrap();
        assert_eq!(restored.condition, stage.condition);
    }

    #[test]
    fn stage_condition_absent_in_json_defaults_to_none() {
        let json = r#"{"kind":"review","optional":true,"max_retries":1}"#;
        let stage: Stage = serde_json::from_str(json).unwrap();
        assert!(stage.condition.is_none());
    }

    #[test]
    fn clarify_injection_before_plan() {
        let mut template = builtin_templates()
            .into_iter()
            .find(|t| t.name == "feature")
            .expect("feature template must exist");

        let clarify_stage = Stage {
            kind: StageKind::Clarify,
            optional: false,
            max_retries: 1,
            timeout_secs: None,
            condition: None,
        };
        let plan_pos = template
            .stages
            .iter()
            .position(|s| s.kind == StageKind::Plan);
        if let Some(pos) = plan_pos {
            template.stages.insert(pos, clarify_stage);
        } else {
            template.stages.insert(0, clarify_stage);
        }

        assert_eq!(template.stages[0].kind, StageKind::Clarify);
        assert_eq!(template.stages[1].kind, StageKind::Plan);
    }

    fn make_policy(
        workflows: std::collections::HashMap<String, String>,
        default_workflow: Option<String>,
        workflow_templates: Vec<WorkflowTemplate>,
    ) -> crate::policy::RepoPolicy {
        crate::policy::RepoPolicy {
            repo: "owner/repo".to_string(),
            eligible_labels: vec![],
            exclude_labels: vec![],
            workflows,
            branch_pattern: "automation/{issue}-{slug}".to_string(),
            max_concurrency: 3,
            concurrency: Default::default(),
            auto_merge: false,
            agent: "claude".to_string(),
            model: None,
            validation_commands: vec![],
            stage_prompts: Default::default(),
            default_workflow,
            workflow_templates,
        }
    }

    fn make_issue(labels: Vec<String>) -> crate::github::IssueCandidate {
        crate::github::IssueCandidate {
            number: 1,
            repo: "owner/repo".to_string(),
            title: "some issue".to_string(),
            body: String::new(),
            labels,
            state: "open".to_string(),
            created_at: String::new(),
            updated_at: String::new(),
            is_assigned: false,
            html_url: String::new(),
        }
    }

    #[test]
    fn label_mapping_selects_bug_workflow() {
        let mut workflows = std::collections::HashMap::new();
        workflows.insert("bug".to_string(), "bug".to_string());
        let policy = make_policy(workflows, None, vec![]);
        let issue = make_issue(vec!["bug".to_string()]);
        let tmpl = select_workflow(&issue, &policy);
        assert_eq!(tmpl.name, "bug");
    }

    #[test]
    fn no_label_match_falls_back_to_default_workflow() {
        let policy = make_policy(Default::default(), Some("chore".to_string()), vec![]);
        let issue = make_issue(vec![]);
        let tmpl = select_workflow(&issue, &policy);
        assert_eq!(tmpl.name, "chore");
    }

    #[test]
    fn no_label_no_default_falls_back_to_feature() {
        let policy = make_policy(Default::default(), None, vec![]);
        let issue = make_issue(vec![]);
        let tmpl = select_workflow(&issue, &policy);
        assert_eq!(tmpl.name, "feature");
    }

    #[test]
    fn title_prefix_does_not_affect_selection() {
        // Issues titled like "fix: ..." should still get "feature" without a label mapping.
        let policy = make_policy(Default::default(), None, vec![]);
        let mut issue = make_issue(vec![]);
        issue.title = "fix: some bug".to_string();
        let tmpl = select_workflow(&issue, &policy);
        assert_eq!(tmpl.name, "feature");
    }

    #[test]
    fn custom_template_shadows_builtin() {
        let custom = WorkflowTemplate {
            name: "bug".to_string(),
            stages: vec![Stage {
                kind: StageKind::Comment,
                optional: false,
                max_retries: 1,
                timeout_secs: None,
                condition: None,
            }],
        };
        let mut workflows = std::collections::HashMap::new();
        workflows.insert("bug".to_string(), "bug".to_string());
        let policy = make_policy(workflows, None, vec![custom]);
        let issue = make_issue(vec!["bug".to_string()]);
        let tmpl = select_workflow(&issue, &policy);
        assert_eq!(tmpl.name, "bug");
        assert_eq!(tmpl.stages.len(), 1);
        assert_eq!(tmpl.stages[0].kind, StageKind::Comment);
    }
}
