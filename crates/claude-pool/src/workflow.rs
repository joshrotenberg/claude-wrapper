//! Workflow templates — preset chains for common patterns.
//!
//! Workflows define reusable multi-step pipelines with placeholders for customization.
//! They simplify invoking common patterns like "issue to PR" or "refactor and test"
//! without manually composing individual chain steps.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::chain::{ChainStep, StepAction, StepFailurePolicy};
use crate::types::TaskOverrides;

/// A workflow template — a preset chain with placeholders.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Workflow {
    /// Unique workflow name (e.g. "issue_to_pr", "refactor_and_test").
    pub name: String,

    /// Human-readable description of what this workflow does.
    pub description: String,

    /// Template steps with placeholders.
    pub steps: Vec<WorkflowStep>,

    /// Argument definitions for this workflow.
    pub arguments: Vec<WorkflowArgument>,
}

/// A step in a workflow template.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkflowStep {
    /// Step name (for logging and result tracking).
    pub name: String,

    /// Either an inline prompt or a skill reference (may contain placeholders).
    pub action: StepAction,

    /// Per-step config overrides (model, effort, etc.).
    pub config: Option<TaskOverrides>,

    /// Failure policy for this step.
    #[serde(default)]
    pub failure_policy: StepFailurePolicy,
}

/// An argument accepted by a workflow.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkflowArgument {
    /// Argument name (used as `{name}` in the template).
    pub name: String,

    /// Human-readable description.
    pub description: String,

    /// Whether this argument is required.
    pub required: bool,
}

impl Workflow {
    /// Instantiate this workflow by substituting placeholders with arguments.
    ///
    /// Validates that all required arguments are provided, then replaces
    /// `{placeholder}` in prompts and skill arguments with values from the map.
    pub fn instantiate(&self, args: &HashMap<String, String>) -> crate::Result<Vec<ChainStep>> {
        // Validate required arguments.
        for arg in &self.arguments {
            if arg.required && !args.contains_key(&arg.name) {
                return Err(crate::Error::Store(format!(
                    "missing required argument '{}' for workflow '{}'",
                    arg.name, self.name
                )));
            }
        }

        // Substitute placeholders in steps.
        let mut steps = Vec::new();
        for ws in &self.steps {
            let action = match &ws.action {
                StepAction::Prompt { prompt } => {
                    let mut p = prompt.clone();
                    for (key, value) in args {
                        p = p.replace(&format!("{{{key}}}"), value);
                    }
                    StepAction::Prompt { prompt: p }
                }
                StepAction::Skill { skill, arguments } => {
                    let mut args_substituted = arguments.clone();
                    for value in args_substituted.values_mut() {
                        for (arg_key, arg_value) in args {
                            *value = value.replace(&format!("{{{arg_key}}}"), arg_value);
                        }
                    }
                    StepAction::Skill {
                        skill: skill.clone(),
                        arguments: args_substituted,
                    }
                }
            };

            steps.push(ChainStep {
                name: ws.name.clone(),
                action,
                config: ws.config.clone(),
                failure_policy: ws.failure_policy.clone(),
                output_vars: Default::default(),
            });
        }

        Ok(steps)
    }
}

/// Registry of available workflows.
#[derive(Debug, Clone, Default)]
pub struct WorkflowRegistry {
    workflows: HashMap<String, Workflow>,
}

impl WorkflowRegistry {
    /// Create a new empty registry.
    pub fn new() -> Self {
        Self::default()
    }

    /// Create a registry pre-loaded with built-in workflows.
    pub fn with_builtins() -> Self {
        let mut registry = Self::new();
        for workflow in builtin_workflows() {
            registry.register(workflow);
        }
        registry
    }

    /// Register a workflow.
    pub fn register(&mut self, workflow: Workflow) {
        self.workflows.insert(workflow.name.clone(), workflow);
    }

    /// Look up a workflow by name.
    pub fn get(&self, name: &str) -> Option<&Workflow> {
        self.workflows.get(name)
    }

    /// List all registered workflows.
    pub fn list(&self) -> Vec<&Workflow> {
        self.workflows.values().collect()
    }

    /// Remove a workflow by name.
    pub fn remove(&mut self, name: &str) -> Option<Workflow> {
        self.workflows.remove(name)
    }
}

/// Built-in workflow definitions.
pub fn builtin_workflows() -> Vec<Workflow> {
    vec![
        Workflow {
            name: "issue_to_pr".into(),
            description: "Take an issue description and implement a solution, creating a PR-ready commit."
                .into(),
            steps: vec![
                WorkflowStep {
                    name: "analyze_issue".into(),
                    action: StepAction::Skill {
                        skill: "summarize".into(),
                        arguments: {
                            let mut m = HashMap::new();
                            m.insert("target".into(), "{issue_url}".into());
                            m
                        },
                    },
                    config: None,
                    failure_policy: StepFailurePolicy::default(),
                },
                WorkflowStep {
                    name: "implement_solution".into(),
                    action: StepAction::Skill {
                        skill: "implement".into(),
                        arguments: {
                            let mut m = HashMap::new();
                            m.insert("description".into(), "{issue_url}".into());
                            m
                        },
                    },
                    config: None,
                    failure_policy: StepFailurePolicy {
                        retries: 1,
                        recovery_prompt: Some(
                            "Previous implementation failed. Try a different approach.".into(),
                        ),
                    },
                },
                WorkflowStep {
                    name: "write_tests".into(),
                    action: StepAction::Skill {
                        skill: "write_tests".into(),
                        arguments: {
                            let mut m = HashMap::new();
                            m.insert("target".into(), ".".into());
                            m
                        },
                    },
                    config: None,
                    failure_policy: StepFailurePolicy::default(),
                },
                WorkflowStep {
                    name: "run_checks".into(),
                    action: StepAction::Skill {
                        skill: "pre_push".into(),
                        arguments: HashMap::new(),
                    },
                    config: None,
                    failure_policy: StepFailurePolicy {
                        retries: 2,
                        recovery_prompt: Some("Fix failures and rerun checks.".into()),
                    },
                },
            ],
            arguments: vec![WorkflowArgument {
                name: "issue_url".into(),
                description: "GitHub issue URL or issue description".into(),
                required: true,
            }],
        },
        Workflow {
            name: "refactor_and_test".into(),
            description:
                "Refactor code toward a goal, write/update tests, and verify success.".into(),
            steps: vec![
                WorkflowStep {
                    name: "refactor".into(),
                    action: StepAction::Skill {
                        skill: "refactor".into(),
                        arguments: {
                            let mut m = HashMap::new();
                            m.insert("target".into(), "{target_file}".into());
                            m.insert("goal".into(), "{refactor_goal}".into());
                            m
                        },
                    },
                    config: None,
                    failure_policy: StepFailurePolicy {
                        retries: 1,
                        recovery_prompt: None,
                    },
                },
                WorkflowStep {
                    name: "update_tests".into(),
                    action: StepAction::Skill {
                        skill: "write_tests".into(),
                        arguments: {
                            let mut m = HashMap::new();
                            m.insert("target".into(), "{target_file}".into());
                            m
                        },
                    },
                    config: None,
                    failure_policy: StepFailurePolicy::default(),
                },
                WorkflowStep {
                    name: "verify_quality".into(),
                    action: StepAction::Prompt {
                        prompt: "Run tests on {target_file} and verify all tests pass.".into(),
                    },
                    config: None,
                    failure_policy: StepFailurePolicy::default(),
                },
            ],
            arguments: vec![
                WorkflowArgument {
                    name: "target_file".into(),
                    description: "File or module to refactor".into(),
                    required: true,
                },
                WorkflowArgument {
                    name: "refactor_goal".into(),
                    description: "What the refactoring should achieve".into(),
                    required: true,
                },
            ],
        },
        Workflow {
            name: "review_and_fix".into(),
            description: "Review code or PR, identify issues, and apply fixes.".into(),
            steps: vec![
                WorkflowStep {
                    name: "review".into(),
                    action: StepAction::Skill {
                        skill: "code_review".into(),
                        arguments: {
                            let mut m = HashMap::new();
                            m.insert("target".into(), "{review_target}".into());
                            m
                        },
                    },
                    config: None,
                    failure_policy: StepFailurePolicy::default(),
                },
                WorkflowStep {
                    name: "apply_fixes".into(),
                    action: StepAction::Prompt {
                        prompt:
                            "Based on the review feedback for {review_target}, apply all suggested fixes."
                                .into(),
                    },
                    config: None,
                    failure_policy: StepFailurePolicy {
                        retries: 1,
                        recovery_prompt: Some(
                            "Review failed. Try a different approach to fixing the issues.".into(),
                        ),
                    },
                },
                WorkflowStep {
                    name: "verify_fixes".into(),
                    action: StepAction::Skill {
                        skill: "code_review".into(),
                        arguments: {
                            let mut m = HashMap::new();
                            m.insert("target".into(), "{review_target}".into());
                            m
                        },
                    },
                    config: None,
                    failure_policy: StepFailurePolicy::default(),
                },
            ],
            arguments: vec![WorkflowArgument {
                name: "review_target".into(),
                description: "Code, PR URL, or file path to review".into(),
                required: true,
            }],
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn workflow_instantiation() {
        let mut args = HashMap::new();
        args.insert(
            "issue_url".into(),
            "https://github.com/owner/repo/issues/42".into(),
        );

        let registry = WorkflowRegistry::with_builtins();
        let workflow = registry.get("issue_to_pr").expect("workflow not found");
        let steps = workflow.instantiate(&args).expect("instantiation failed");

        assert!(!steps.is_empty());
        // Check that placeholders were substituted in first step
        if let StepAction::Skill { arguments, .. } = &steps[0].action {
            let target = arguments.get("target").expect("target argument missing");
            assert_eq!(target, "https://github.com/owner/repo/issues/42");
        } else {
            panic!("expected skill action");
        }
    }

    #[test]
    fn missing_required_argument() {
        let registry = WorkflowRegistry::with_builtins();
        let workflow = registry.get("issue_to_pr").expect("workflow not found");
        let result = workflow.instantiate(&HashMap::new());

        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("missing required argument")
        );
    }

    #[test]
    fn multiple_placeholders() {
        let mut args = HashMap::new();
        args.insert("target_file".into(), "src/lib.rs".into());
        args.insert("refactor_goal".into(), "improve readability".into());

        let registry = WorkflowRegistry::with_builtins();
        let workflow = registry
            .get("refactor_and_test")
            .expect("workflow not found");
        let steps = workflow.instantiate(&args).expect("instantiation failed");

        assert!(!steps.is_empty());
        if let StepAction::Skill { arguments, .. } = &steps[0].action {
            assert_eq!(arguments.get("target").unwrap(), "src/lib.rs");
            assert_eq!(arguments.get("goal").unwrap(), "improve readability");
        } else {
            panic!("expected skill action");
        }
    }

    #[test]
    fn builtin_workflows_registered() {
        let registry = WorkflowRegistry::with_builtins();
        assert!(registry.get("issue_to_pr").is_some());
        assert!(registry.get("refactor_and_test").is_some());
        assert!(registry.get("review_and_fix").is_some());
        assert_eq!(registry.list().len(), 3);
    }

    #[test]
    fn workflow_registration() {
        let mut registry = WorkflowRegistry::new();
        assert!(registry.get("test_workflow").is_none());

        let workflow = Workflow {
            name: "test_workflow".into(),
            description: "Test workflow".into(),
            steps: vec![],
            arguments: vec![],
        };
        registry.register(workflow);

        assert!(registry.get("test_workflow").is_some());
        assert_eq!(registry.list().len(), 1);

        let removed = registry.remove("test_workflow");
        assert!(removed.is_some());
        assert!(registry.get("test_workflow").is_none());
    }
}
