//! Skill definitions — reusable prompt templates.
//!
//! Skills are parameterized templates that define how to approach a specific
//! kind of task. The coordinator discovers them via MCP prompt listing,
//! then references them by name in `pool/run` or `pool/submit`.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::types::WorkerConfig;

/// A reusable skill template.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Skill {
    /// Unique skill name (e.g. "code_review", "write_tests").
    pub name: String,

    /// Human-readable description of what this skill does.
    pub description: String,

    /// Prompt template. Use `{arg_name}` placeholders for arguments.
    pub prompt: String,

    /// Argument definitions (name -> description).
    pub arguments: Vec<SkillArgument>,

    /// Per-skill config overrides (model, effort, etc.).
    pub config: Option<WorkerConfig>,
}

/// An argument accepted by a skill.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillArgument {
    /// Argument name (used as `{name}` in the prompt template).
    pub name: String,

    /// Human-readable description.
    pub description: String,

    /// Whether this argument is required.
    pub required: bool,
}

impl Skill {
    /// Render the prompt template with the given arguments.
    ///
    /// Replaces `{arg_name}` placeholders in the prompt with values
    /// from the arguments map. Missing required arguments return an error.
    pub fn render(&self, args: &HashMap<String, String>) -> crate::Result<String> {
        // Check required arguments.
        for arg in &self.arguments {
            if arg.required && !args.contains_key(&arg.name) {
                return Err(crate::Error::Store(format!(
                    "missing required argument '{}' for skill '{}'",
                    arg.name, self.name
                )));
            }
        }

        let mut rendered = self.prompt.clone();
        for (key, value) in args {
            rendered = rendered.replace(&format!("{{{key}}}"), value);
        }
        Ok(rendered)
    }
}

/// Registry of available skills.
#[derive(Debug, Clone, Default)]
pub struct SkillRegistry {
    skills: HashMap<String, Skill>,
}

impl SkillRegistry {
    /// Create a new empty registry.
    pub fn new() -> Self {
        Self::default()
    }

    /// Create a registry pre-loaded with built-in skills.
    pub fn with_builtins() -> Self {
        let mut registry = Self::new();
        for skill in builtin_skills() {
            registry.register(skill);
        }
        registry
    }

    /// Register a skill.
    pub fn register(&mut self, skill: Skill) {
        self.skills.insert(skill.name.clone(), skill);
    }

    /// Look up a skill by name.
    pub fn get(&self, name: &str) -> Option<&Skill> {
        self.skills.get(name)
    }

    /// List all registered skills.
    pub fn list(&self) -> Vec<&Skill> {
        self.skills.values().collect()
    }

    /// Remove a skill by name.
    pub fn remove(&mut self, name: &str) -> Option<Skill> {
        self.skills.remove(name)
    }
}

/// Built-in skill definitions.
pub fn builtin_skills() -> Vec<Skill> {
    vec![
        Skill {
            name: "code_review".into(),
            description: "Review code for bugs, style issues, and improvements.".into(),
            prompt: "Review the following code or changes for bugs, style issues, \
                     and potential improvements. Be thorough but concise.\n\n{target}"
                .into(),
            arguments: vec![SkillArgument {
                name: "target".into(),
                description: "Code, diff, file path, or PR reference to review.".into(),
                required: true,
            }],
            config: None,
        },
        Skill {
            name: "implement".into(),
            description: "Implement a feature based on a description or issue.".into(),
            prompt:
                "Implement the following feature. Write clean, well-tested code.\n\n{description}"
                    .into(),
            arguments: vec![SkillArgument {
                name: "description".into(),
                description: "Feature description, issue URL, or requirements.".into(),
                required: true,
            }],
            config: None,
        },
        Skill {
            name: "write_tests".into(),
            description: "Generate tests for existing code.".into(),
            prompt: "Write comprehensive tests for the following code. Cover edge cases \
                     and error paths.\n\n{target}"
                .into(),
            arguments: vec![SkillArgument {
                name: "target".into(),
                description: "File path, module, or code to test.".into(),
                required: true,
            }],
            config: None,
        },
        Skill {
            name: "refactor".into(),
            description: "Refactor code toward a specific goal.".into(),
            prompt: "Refactor the following code. Goal: {goal}\n\n{target}".into(),
            arguments: vec![
                SkillArgument {
                    name: "target".into(),
                    description: "Code or file path to refactor.".into(),
                    required: true,
                },
                SkillArgument {
                    name: "goal".into(),
                    description: "What the refactoring should achieve.".into(),
                    required: true,
                },
            ],
            config: None,
        },
        Skill {
            name: "summarize".into(),
            description: "Summarize a codebase, file, or document.".into(),
            prompt: "Provide a clear, structured summary of the following.\n\n{target}".into(),
            arguments: vec![SkillArgument {
                name: "target".into(),
                description: "Codebase path, file, or content to summarize.".into(),
                required: true,
            }],
            config: None,
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn render_skill_template() {
        let skill = Skill {
            name: "greet".into(),
            description: "Greet someone".into(),
            prompt: "Hello, {name}! Welcome to {place}.".into(),
            arguments: vec![
                SkillArgument {
                    name: "name".into(),
                    description: "Name".into(),
                    required: true,
                },
                SkillArgument {
                    name: "place".into(),
                    description: "Place".into(),
                    required: false,
                },
            ],
            config: None,
        };

        let mut args = HashMap::new();
        args.insert("name".into(), "Alice".into());
        args.insert("place".into(), "the pool".into());

        let rendered = skill.render(&args).unwrap();
        assert_eq!(rendered, "Hello, Alice! Welcome to the pool.");
    }

    #[test]
    fn missing_required_argument() {
        let skill = Skill {
            name: "test".into(),
            description: "Test".into(),
            prompt: "{x}".into(),
            arguments: vec![SkillArgument {
                name: "x".into(),
                description: "X".into(),
                required: true,
            }],
            config: None,
        };

        let result = skill.render(&HashMap::new());
        assert!(result.is_err());
    }

    #[test]
    fn registry_crud() {
        let mut registry = SkillRegistry::new();
        assert!(registry.list().is_empty());

        registry.register(Skill {
            name: "test".into(),
            description: "A test skill".into(),
            prompt: "do {thing}".into(),
            arguments: vec![],
            config: None,
        });

        assert_eq!(registry.list().len(), 1);
        assert!(registry.get("test").is_some());
        assert!(registry.get("nope").is_none());

        registry.remove("test");
        assert!(registry.list().is_empty());
    }

    #[test]
    fn builtins_load() {
        let registry = SkillRegistry::with_builtins();
        assert_eq!(registry.list().len(), 5);
        assert!(registry.get("code_review").is_some());
        assert!(registry.get("implement").is_some());
        assert!(registry.get("write_tests").is_some());
        assert!(registry.get("refactor").is_some());
        assert!(registry.get("summarize").is_some());
    }
}
