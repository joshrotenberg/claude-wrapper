//! Workflow templates — preset chains for common patterns.
//!
//! Workflows define reusable multi-step pipelines with placeholders for customization.
//! They simplify invoking common patterns like "issue to PR" or "refactor and test"
//! without manually composing individual chain steps.

use std::collections::HashMap;
use std::path::Path;

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
    /// Validate this workflow definition.
    ///
    /// Checks that the workflow has a name, at least one step, and that all
    /// steps have names.
    pub fn validate(&self) -> std::result::Result<(), String> {
        if self.name.is_empty() {
            return Err("workflow name cannot be empty".into());
        }
        if self.steps.is_empty() {
            return Err(format!("workflow '{}' has no steps", self.name));
        }
        for (i, step) in self.steps.iter().enumerate() {
            if step.name.is_empty() {
                return Err(format!(
                    "step {} in workflow '{}' has no name",
                    i, self.name
                ));
            }
        }
        Ok(())
    }

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

    /// Load workflow definitions from a directory.
    ///
    /// Supports two formats:
    /// - **YAML files** (`*.yml` / `*.yaml`): Each file defines one workflow.
    /// - **JSON files** (`*.json`): Each file defines one workflow.
    ///
    /// Returns the number of workflows loaded. Silently returns 0 if the
    /// directory does not exist.
    pub fn load_from_dir(&mut self, dir: &Path) -> crate::Result<usize> {
        if !dir.is_dir() {
            return Ok(0);
        }

        let mut entries: Vec<_> = std::fs::read_dir(dir)
            .map_err(|e| crate::Error::Store(format!("failed to read workflow dir: {e}")))?
            .filter_map(|e| e.ok())
            .collect();
        entries.sort_by_key(|e| e.file_name());

        let mut count = 0;
        for entry in entries {
            let path = entry.path();
            if !path.is_file() {
                continue;
            }

            let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
            let contents = std::fs::read_to_string(&path).map_err(|e| {
                crate::Error::Store(format!("failed to read {}: {e}", path.display()))
            })?;

            let workflow: Workflow = match ext {
                "yml" | "yaml" => serde_yaml::from_str(&contents).map_err(|e| {
                    crate::Error::Store(format!(
                        "failed to parse YAML workflow {}: {e}",
                        path.display()
                    ))
                })?,
                "json" => serde_json::from_str(&contents).map_err(|e| {
                    crate::Error::Store(format!(
                        "failed to parse JSON workflow {}: {e}",
                        path.display()
                    ))
                })?,
                _ => continue,
            };

            workflow.validate().map_err(|e| {
                crate::Error::Store(format!("invalid workflow in {}: {e}", path.display()))
            })?;

            self.register(workflow);
            count += 1;
        }

        Ok(count)
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
    use std::path::Path;

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
    fn validate_empty_name() {
        let wf = Workflow {
            name: "".into(),
            description: "test".into(),
            steps: vec![],
            arguments: vec![],
        };
        assert!(wf.validate().is_err());
        assert!(wf.validate().unwrap_err().contains("name cannot be empty"));
    }

    #[test]
    fn validate_no_steps() {
        let wf = Workflow {
            name: "test".into(),
            description: "test".into(),
            steps: vec![],
            arguments: vec![],
        };
        assert!(wf.validate().is_err());
        assert!(wf.validate().unwrap_err().contains("has no steps"));
    }

    #[test]
    fn validate_step_without_name() {
        let wf = Workflow {
            name: "test".into(),
            description: "test".into(),
            steps: vec![WorkflowStep {
                name: "".into(),
                action: StepAction::Prompt {
                    prompt: "do something".into(),
                },
                config: None,
                failure_policy: StepFailurePolicy::default(),
            }],
            arguments: vec![],
        };
        assert!(wf.validate().is_err());
        assert!(wf.validate().unwrap_err().contains("has no name"));
    }

    #[test]
    fn validate_good_workflow() {
        let wf = Workflow {
            name: "test".into(),
            description: "test".into(),
            steps: vec![WorkflowStep {
                name: "step1".into(),
                action: StepAction::Prompt {
                    prompt: "do something".into(),
                },
                config: None,
                failure_policy: StepFailurePolicy::default(),
            }],
            arguments: vec![],
        };
        assert!(wf.validate().is_ok());
    }

    #[test]
    fn load_from_dir_yaml() {
        let dir = tempfile::tempdir().unwrap();
        let yaml = r#"
name: test_yaml
description: A test workflow loaded from YAML
steps:
  - name: step_one
    action:
      type: prompt
      prompt: "do the thing"
    failure_policy:
      retries: 0
arguments:
  - name: target
    description: What to process
    required: true
"#;
        std::fs::write(dir.path().join("test.yml"), yaml).unwrap();

        let mut registry = WorkflowRegistry::new();
        let count = registry.load_from_dir(dir.path()).unwrap();
        assert_eq!(count, 1);
        let wf = registry.get("test_yaml").unwrap();
        assert_eq!(wf.steps.len(), 1);
        assert_eq!(wf.arguments.len(), 1);
    }

    #[test]
    fn load_from_dir_json() {
        let dir = tempfile::tempdir().unwrap();
        let json = r#"{
            "name": "test_json",
            "description": "A test workflow loaded from JSON",
            "steps": [{
                "name": "step_one",
                "action": {"type": "prompt", "prompt": "do the thing"},
                "failure_policy": {"retries": 0}
            }],
            "arguments": []
        }"#;
        std::fs::write(dir.path().join("test.json"), json).unwrap();

        let mut registry = WorkflowRegistry::new();
        let count = registry.load_from_dir(dir.path()).unwrap();
        assert_eq!(count, 1);
        assert!(registry.get("test_json").is_some());
    }

    #[test]
    fn load_from_dir_ignores_non_workflow_files() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("readme.md"), "# not a workflow").unwrap();
        std::fs::write(dir.path().join("notes.txt"), "some notes").unwrap();

        let mut registry = WorkflowRegistry::new();
        let count = registry.load_from_dir(dir.path()).unwrap();
        assert_eq!(count, 0);
    }

    #[test]
    fn load_from_dir_nonexistent_returns_zero() {
        let mut registry = WorkflowRegistry::new();
        let count = registry
            .load_from_dir(Path::new("/nonexistent/path"))
            .unwrap();
        assert_eq!(count, 0);
    }

    #[test]
    fn load_from_dir_rejects_invalid_workflow() {
        let dir = tempfile::tempdir().unwrap();
        let yaml = r#"
name: ""
description: "invalid"
steps: []
arguments: []
"#;
        std::fs::write(dir.path().join("bad.yml"), yaml).unwrap();

        let mut registry = WorkflowRegistry::new();
        let result = registry.load_from_dir(dir.path());
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("invalid workflow"));
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
