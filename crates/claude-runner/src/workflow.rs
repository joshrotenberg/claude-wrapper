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
pub fn select_workflow(
    issue: &crate::github::IssueCandidate,
    policy: &crate::policy::RepoPolicy,
) -> WorkflowTemplate {
    // Check policy workflow mappings first.
    for label in &issue.labels {
        if let Some(template_name) = policy.workflows.get(label) {
            // Custom templates take precedence over built-ins.
            if let Some(template) = policy
                .workflow_templates
                .iter()
                .find(|t| t.name == *template_name)
            {
                return template.clone();
            }
            if let Some(template) = builtin_templates()
                .into_iter()
                .find(|t| t.name == *template_name)
            {
                return template;
            }
        }
    }

    // Infer from title prefix.
    let lower_title = issue.title.to_lowercase();
    let inferred = if lower_title.starts_with("fix")
        || lower_title.starts_with("bug")
        || lower_title.contains("bug:")
    {
        "bug"
    } else if lower_title.starts_with("chore")
        || lower_title.starts_with("refactor")
        || lower_title.contains("chore:")
    {
        "chore"
    } else if lower_title.starts_with("research")
        || lower_title.starts_with("discuss")
        || lower_title.contains("research:")
    {
        "research"
    } else {
        "feature"
    };

    builtin_templates()
        .into_iter()
        .find(|t| t.name == inferred)
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

    fn make_policy_with_templates(
        workflows: std::collections::HashMap<String, String>,
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
            workflow_templates,
        }
    }

    fn make_issue(number: u64, title: &str, labels: Vec<String>) -> crate::github::IssueCandidate {
        crate::github::IssueCandidate {
            number,
            repo: "owner/repo".to_string(),
            title: title.to_string(),
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
    fn select_workflow_custom_template_via_label_mapping() {
        let custom = WorkflowTemplate {
            name: "my-custom".to_string(),
            stages: vec![Stage {
                kind: StageKind::Research,
                optional: false,
                max_retries: 1,
                timeout_secs: None,
                condition: None,
            }],
        };
        let mut workflows = std::collections::HashMap::new();
        workflows.insert("my-label".to_string(), "my-custom".to_string());
        let policy = make_policy_with_templates(workflows, vec![custom]);
        let issue = make_issue(1, "some task", vec!["my-label".to_string()]);
        let selected = select_workflow(&issue, &policy);
        assert_eq!(selected.name, "my-custom");
        assert_eq!(selected.stages.len(), 1);
        assert_eq!(selected.stages[0].kind, StageKind::Research);
    }

    #[test]
    fn select_workflow_custom_overrides_builtin_with_same_name() {
        let custom_bug = WorkflowTemplate {
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
        let policy = make_policy_with_templates(workflows, vec![custom_bug]);
        let issue = make_issue(2, "fix something", vec!["bug".to_string()]);
        let selected = select_workflow(&issue, &policy);
        assert_eq!(selected.name, "bug");
        // Custom has only 1 stage (Comment); builtin "bug" has 5 stages.
        assert_eq!(selected.stages.len(), 1);
        assert_eq!(selected.stages[0].kind, StageKind::Comment);
    }

    #[test]
    fn select_workflow_falls_back_to_builtin_when_no_custom() {
        let mut workflows = std::collections::HashMap::new();
        workflows.insert("bug".to_string(), "bug".to_string());
        let policy = make_policy_with_templates(workflows, vec![]);
        let issue = make_issue(3, "fix crash", vec!["bug".to_string()]);
        let selected = select_workflow(&issue, &policy);
        assert_eq!(selected.name, "bug");
        assert!(selected.stages.len() > 1);
    }

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
}
