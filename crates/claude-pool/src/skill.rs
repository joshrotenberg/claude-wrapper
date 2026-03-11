//! Skill definitions — reusable prompt templates.
//!
//! Skills are parameterized templates that define how to approach a specific
//! kind of task. The coordinator discovers them via MCP prompt listing,
//! then references them by name in `pool/run` or `pool/submit`.

use std::collections::HashMap;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::types::SlotConfig;

/// How a skill was registered in the registry.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SkillSource {
    /// Ships with the pool binary.
    Builtin,
    /// Loaded from `~/.claude-pool/skills/` (user global).
    Global,
    /// Loaded from `.claude-pool/skills/` (project).
    Project,
    /// Added at runtime via `pool_skill_add`.
    Runtime,
}

impl std::fmt::Display for SkillSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Builtin => write!(f, "builtin"),
            Self::Global => write!(f, "global"),
            Self::Project => write!(f, "project"),
            Self::Runtime => write!(f, "runtime"),
        }
    }
}

/// A skill paired with its registration source.
#[derive(Debug, Clone)]
pub struct RegisteredSkill {
    /// The skill definition.
    pub skill: Skill,
    /// How this skill was registered.
    pub source: SkillSource,
}

/// Where a skill is intended to run.
///
/// Advisory only — the pool does not enforce scope. Coordinators and agents
/// use it to decide whether a skill makes sense in a given context.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum SkillScope {
    /// Single unit of work, any slot can run it.
    #[default]
    Task,

    /// Needs MCP access, human interaction, or cross-cutting visibility.
    /// Should run at the coordinator level, not in a pool slot.
    Coordinator,

    /// Multi-step workflow template. Used as a chain definition, not a
    /// single task.
    Chain,
}

impl std::fmt::Display for SkillScope {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Task => write!(f, "task"),
            Self::Coordinator => write!(f, "coordinator"),
            Self::Chain => write!(f, "chain"),
        }
    }
}

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
    pub config: Option<SlotConfig>,

    /// Where this skill is intended to run (advisory).
    #[serde(default)]
    pub scope: SkillScope,
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

/// YAML frontmatter from a SKILL.md file.
///
/// Follows the [Agent Skills standard](https://agentskills.io/specification):
/// name, description, and pool-specific extensions under `metadata`.
#[derive(Debug, Deserialize)]
struct SkillFrontmatter {
    name: String,
    description: String,
    #[serde(default)]
    metadata: SkillMetadata,
}

/// Pool-specific metadata extensions in SKILL.md frontmatter.
#[derive(Debug, Default, Deserialize)]
struct SkillMetadata {
    #[serde(default)]
    scope: Option<SkillScope>,
    #[serde(default)]
    arguments: Vec<SkillArgument>,
    #[serde(default)]
    config: Option<SlotConfig>,
}

/// Parse a SKILL.md file into a [`Skill`].
///
/// The file format is YAML frontmatter between `---` delimiters, followed
/// by a markdown body that becomes the prompt template.
fn parse_skill_md(content: &str) -> crate::Result<Skill> {
    let content = content.trim();
    if !content.starts_with("---") {
        return Err(crate::Error::Store(
            "SKILL.md must start with YAML frontmatter (---)".into(),
        ));
    }

    let after_first = &content[3..];
    let end = after_first.find("---").ok_or_else(|| {
        crate::Error::Store("SKILL.md missing closing frontmatter delimiter (---)".into())
    })?;

    let yaml = &after_first[..end];
    let body = after_first[end + 3..].trim();

    let fm: SkillFrontmatter = serde_yaml::from_str(yaml)
        .map_err(|e| crate::Error::Store(format!("SKILL.md YAML parse error: {e}")))?;

    // Infer scope from name prefix if not set explicitly.
    let scope = fm.metadata.scope.unwrap_or_else(|| infer_scope(&fm.name));

    Ok(Skill {
        name: fm.name,
        description: fm.description,
        prompt: body.to_string(),
        arguments: fm.metadata.arguments,
        config: fm.metadata.config,
        scope,
    })
}

/// Infer skill scope from name prefix convention.
fn infer_scope(name: &str) -> SkillScope {
    if name.starts_with("cps-coordinator") {
        SkillScope::Coordinator
    } else if name.starts_with("cps-chain") {
        SkillScope::Chain
    } else {
        SkillScope::Task
    }
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

        // Legacy {key} substitution (our original format).
        for (key, value) in args {
            rendered = rendered.replace(&format!("{{{key}}}"), value);
        }

        // Standard $ARGUMENTS substitution (Agent Skills / Claude Code format).
        // Build positional args list from argument definitions order.
        let positional: Vec<&str> = self
            .arguments
            .iter()
            .filter_map(|a| args.get(&a.name).map(|v| v.as_str()))
            .collect();

        let all_args = positional.join(" ");
        let has_arguments_var = rendered.contains("$ARGUMENTS") || rendered.contains("$0");

        // $ARGUMENTS[N] and $N positional substitution.
        for (i, val) in positional.iter().enumerate() {
            rendered = rendered.replace(&format!("$ARGUMENTS[{i}]"), val);
            rendered = rendered.replace(&format!("${i}"), val);
        }

        // $ARGUMENTS (all args as a single string).
        rendered = rendered.replace("$ARGUMENTS", &all_args);

        // If neither $ARGUMENTS/$N nor {key} placeholders were used and there
        // are args, append them (matches Claude Code behavior).
        let had_legacy_placeholders = self
            .arguments
            .iter()
            .any(|a| self.prompt.contains(&format!("{{{}}}", a.name)));
        if !has_arguments_var && !had_legacy_placeholders && !all_args.is_empty() {
            rendered.push_str(&format!("\n\nARGUMENTS: {all_args}"));
        }

        Ok(rendered)
    }
}

/// Registry of available skills.
#[derive(Debug, Clone, Default)]
pub struct SkillRegistry {
    skills: HashMap<String, RegisteredSkill>,
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
            registry.register(skill, SkillSource::Builtin);
        }
        registry
    }

    /// Register a skill with a given source.
    pub fn register(&mut self, skill: Skill, source: SkillSource) {
        self.skills
            .insert(skill.name.clone(), RegisteredSkill { skill, source });
    }

    /// Look up a skill by name.
    pub fn get(&self, name: &str) -> Option<&Skill> {
        self.skills.get(name).map(|rs| &rs.skill)
    }

    /// Look up a registered skill (with source metadata) by name.
    pub fn get_registered(&self, name: &str) -> Option<&RegisteredSkill> {
        self.skills.get(name)
    }

    /// List all registered skills.
    pub fn list(&self) -> Vec<&Skill> {
        self.skills.values().map(|rs| &rs.skill).collect()
    }

    /// List all registered skills with source metadata.
    pub fn list_registered(&self) -> Vec<&RegisteredSkill> {
        self.skills.values().collect()
    }

    /// Remove a skill by name. Returns the removed skill if found.
    pub fn remove(&mut self, name: &str) -> Option<Skill> {
        self.skills.remove(name).map(|rs| rs.skill)
    }

    /// Remove multiple skills by name.
    pub fn remove_many(&mut self, names: &[&str]) {
        for name in names {
            self.skills.remove(*name);
        }
    }

    /// List skills filtered by scope.
    pub fn list_by_scope(&self, scope: SkillScope) -> Vec<&Skill> {
        self.skills
            .values()
            .filter(|rs| rs.skill.scope == scope)
            .map(|rs| &rs.skill)
            .collect()
    }

    /// Load skill definitions from a directory.
    ///
    /// Discovers skills in two formats, in sorted order:
    /// - **SKILL.md folders**: `skill-name/SKILL.md` (Agent Skills standard)
    /// - **JSON files**: `skill_name.json` (legacy, with deprecation warning)
    ///
    /// Skills are registered with the given `source`. Skills loaded this way
    /// override any existing skill with the same name. Returns the number of
    /// skills loaded. If the directory does not exist, returns `Ok(0)`.
    pub fn load_from_dir(&mut self, dir: &Path) -> crate::Result<usize> {
        self.load_from_dir_with_source(dir, SkillSource::Project)
    }

    /// Load skill definitions from a directory with the specified source.
    pub fn load_from_dir_with_source(
        &mut self,
        dir: &Path,
        source: SkillSource,
    ) -> crate::Result<usize> {
        if !dir.is_dir() {
            return Ok(0);
        }

        let mut entries: Vec<_> = std::fs::read_dir(dir)?.filter_map(|e| e.ok()).collect();
        entries.sort_by_key(|e| e.file_name());

        let mut count = 0;
        for entry in entries {
            let path = entry.path();

            // SKILL.md folder format (preferred).
            if path.is_dir() {
                let skill_md = path.join("SKILL.md");
                if skill_md.is_file() {
                    let contents = std::fs::read_to_string(&skill_md)?;
                    let skill = parse_skill_md(&contents)?;
                    self.register(skill, source);
                    count += 1;
                }
                continue;
            }

            // Legacy JSON format (deprecated).
            if path.extension().is_some_and(|ext| ext == "json") {
                tracing::warn!(
                    path = %path.display(),
                    "loading skill from JSON format (deprecated — migrate to SKILL.md folder)"
                );
                let contents = std::fs::read_to_string(&path)?;
                let skill: Skill = serde_json::from_str(&contents)?;
                self.register(skill, source);
                count += 1;
            }
        }

        Ok(count)
    }
}

/// Built-in skill definitions.
///
/// Each skill is defined as a SKILL.md file under `skills/` and embedded
/// at compile time via `include_str!`. This keeps prompts readable and
/// editable as standard markdown while ensuring they ship with the binary.
pub fn builtin_skills() -> Vec<Skill> {
    const SKILL_SOURCES: &[&str] = &[
        include_str!("../skills/code_review/SKILL.md"),
        include_str!("../skills/implement/SKILL.md"),
        include_str!("../skills/write_tests/SKILL.md"),
        include_str!("../skills/refactor/SKILL.md"),
        include_str!("../skills/summarize/SKILL.md"),
        include_str!("../skills/pre_push/SKILL.md"),
        include_str!("../skills/create_pr/SKILL.md"),
        include_str!("../skills/issue_watcher/SKILL.md"),
        include_str!("../skills/loop_monitor/SKILL.md"),
        include_str!("../skills/pool_dashboard/SKILL.md"),
        include_str!("../skills/chain_watcher/SKILL.md"),
        include_str!("../skills/plan_then_execute/SKILL.md"),
    ];

    SKILL_SOURCES
        .iter()
        .map(|src| parse_skill_md(src).expect("builtin SKILL.md should be valid"))
        .collect()
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
            scope: SkillScope::Task,
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
            scope: SkillScope::Task,
        };

        let result = skill.render(&HashMap::new());
        assert!(result.is_err());
    }

    #[test]
    fn render_dollar_arguments_all() {
        let skill = Skill {
            name: "fix".into(),
            description: "Fix issue".into(),
            prompt: "Fix issue $ARGUMENTS following conventions.".into(),
            arguments: vec![SkillArgument {
                name: "issue".into(),
                description: "Issue number".into(),
                required: true,
            }],
            config: None,
            scope: SkillScope::Task,
        };

        let mut args = HashMap::new();
        args.insert("issue".into(), "123".into());
        let rendered = skill.render(&args).unwrap();
        assert_eq!(rendered, "Fix issue 123 following conventions.");
    }

    #[test]
    fn render_dollar_positional() {
        let skill = Skill {
            name: "migrate".into(),
            description: "Migrate component".into(),
            prompt: "Migrate $0 from $1 to $2.".into(),
            arguments: vec![
                SkillArgument {
                    name: "component".into(),
                    description: "Component name".into(),
                    required: true,
                },
                SkillArgument {
                    name: "from".into(),
                    description: "Source framework".into(),
                    required: true,
                },
                SkillArgument {
                    name: "to".into(),
                    description: "Target framework".into(),
                    required: true,
                },
            ],
            config: None,
            scope: SkillScope::Task,
        };

        let mut args = HashMap::new();
        args.insert("component".into(), "SearchBar".into());
        args.insert("from".into(), "React".into());
        args.insert("to".into(), "Vue".into());
        let rendered = skill.render(&args).unwrap();
        assert_eq!(rendered, "Migrate SearchBar from React to Vue.");
    }

    #[test]
    fn render_dollar_arguments_n_bracket() {
        let skill = Skill {
            name: "test".into(),
            description: "Test".into(),
            prompt: "Process $ARGUMENTS[0] then $ARGUMENTS[1].".into(),
            arguments: vec![
                SkillArgument {
                    name: "a".into(),
                    description: "A".into(),
                    required: true,
                },
                SkillArgument {
                    name: "b".into(),
                    description: "B".into(),
                    required: true,
                },
            ],
            config: None,
            scope: SkillScope::Task,
        };

        let mut args = HashMap::new();
        args.insert("a".into(), "foo".into());
        args.insert("b".into(), "bar".into());
        let rendered = skill.render(&args).unwrap();
        assert_eq!(rendered, "Process foo then bar.");
    }

    #[test]
    fn render_no_placeholder_appends_arguments() {
        let skill = Skill {
            name: "test".into(),
            description: "Test".into(),
            prompt: "Do the thing.".into(),
            arguments: vec![SkillArgument {
                name: "target".into(),
                description: "Target".into(),
                required: true,
            }],
            config: None,
            scope: SkillScope::Task,
        };

        let mut args = HashMap::new();
        args.insert("target".into(), "src/main.rs".into());
        let rendered = skill.render(&args).unwrap();
        assert_eq!(rendered, "Do the thing.\n\nARGUMENTS: src/main.rs");
    }

    #[test]
    fn render_legacy_placeholder_no_append() {
        let skill = Skill {
            name: "test".into(),
            description: "Test".into(),
            prompt: "Review {target} carefully.".into(),
            arguments: vec![SkillArgument {
                name: "target".into(),
                description: "Target".into(),
                required: true,
            }],
            config: None,
            scope: SkillScope::Task,
        };

        let mut args = HashMap::new();
        args.insert("target".into(), "src/main.rs".into());
        let rendered = skill.render(&args).unwrap();
        // Legacy placeholder consumed the arg, no ARGUMENTS append.
        assert_eq!(rendered, "Review src/main.rs carefully.");
    }

    #[test]
    fn registry_crud() {
        let mut registry = SkillRegistry::new();
        assert!(registry.list().is_empty());

        registry.register(
            Skill {
                name: "test".into(),
                description: "A test skill".into(),
                prompt: "do {thing}".into(),
                arguments: vec![],
                config: None,
                scope: SkillScope::Task,
            },
            SkillSource::Runtime,
        );

        assert_eq!(registry.list().len(), 1);
        assert!(registry.get("test").is_some());
        assert!(registry.get("nope").is_none());

        registry.remove("test");
        assert!(registry.list().is_empty());
    }

    #[test]
    fn load_from_nonexistent_dir() {
        let mut registry = SkillRegistry::new();
        let count = registry
            .load_from_dir(Path::new("/tmp/does-not-exist-claude-pool-test"))
            .unwrap();
        assert_eq!(count, 0);
    }

    #[test]
    fn load_from_dir_with_json_files() {
        let dir = tempfile::tempdir().unwrap();

        let skill_json = serde_json::json!({
            "name": "my_skill",
            "description": "A test skill",
            "prompt": "Do {thing}",
            "arguments": [
                { "name": "thing", "description": "What to do", "required": true }
            ],
            "config": null
        });
        std::fs::write(
            dir.path().join("my_skill.json"),
            serde_json::to_string_pretty(&skill_json).unwrap(),
        )
        .unwrap();

        // Non-json file should be ignored.
        std::fs::write(dir.path().join("readme.txt"), "not a skill").unwrap();

        let mut registry = SkillRegistry::new();
        let count = registry.load_from_dir(dir.path()).unwrap();
        assert_eq!(count, 1);

        let skill = registry.get("my_skill").unwrap();
        assert_eq!(skill.description, "A test skill");
        assert_eq!(skill.arguments.len(), 1);
        assert!(skill.arguments[0].required);
    }

    #[test]
    fn project_skills_override_builtins() {
        let dir = tempfile::tempdir().unwrap();

        let override_json = serde_json::json!({
            "name": "code_review",
            "description": "Custom project review",
            "prompt": "Review with custom rules: {target}",
            "arguments": [
                { "name": "target", "description": "What to review", "required": true }
            ],
            "config": null
        });
        std::fs::write(
            dir.path().join("code_review.json"),
            serde_json::to_string_pretty(&override_json).unwrap(),
        )
        .unwrap();

        let mut registry = SkillRegistry::with_builtins();
        assert_eq!(
            registry.get("code_review").unwrap().description,
            "Review code for bugs, style issues, and improvements."
        );

        let count = registry.load_from_dir(dir.path()).unwrap();
        assert_eq!(count, 1);
        assert_eq!(
            registry.get("code_review").unwrap().description,
            "Custom project review"
        );
    }

    #[test]
    fn builtins_load() {
        let registry = SkillRegistry::with_builtins();
        // 7 task + 4 coordinator + 1 chain = 12 builtins
        assert_eq!(registry.list().len(), 12);
        // Task-scoped
        assert!(registry.get("code_review").is_some());
        assert!(registry.get("implement").is_some());
        assert!(registry.get("write_tests").is_some());
        assert!(registry.get("refactor").is_some());
        assert!(registry.get("summarize").is_some());
        assert!(registry.get("pre_push").is_some());
        assert!(registry.get("create_pr").is_some());
        // Coordinator-scoped
        assert!(registry.get("issue_watcher").is_some());
        assert!(registry.get("loop_monitor").is_some());
        assert!(registry.get("pool_dashboard").is_some());
        assert!(registry.get("chain_watcher").is_some());
    }

    #[test]
    fn list_by_scope() {
        let registry = SkillRegistry::with_builtins();
        let tasks = registry.list_by_scope(SkillScope::Task);
        let coordinators = registry.list_by_scope(SkillScope::Coordinator);
        let chains = registry.list_by_scope(SkillScope::Chain);

        assert_eq!(tasks.len(), 7);
        assert_eq!(coordinators.len(), 4);
        assert_eq!(chains.len(), 1);
    }

    #[test]
    fn remove_many_skills() {
        let mut registry = SkillRegistry::with_builtins();
        let before = registry.list().len();
        registry.remove_many(&["create_pr", "issue_watcher"]);
        assert_eq!(registry.list().len(), before - 2);
        assert!(registry.get("create_pr").is_none());
        assert!(registry.get("issue_watcher").is_none());
    }

    #[test]
    fn scope_default_is_task() {
        assert_eq!(SkillScope::default(), SkillScope::Task);
    }

    #[test]
    fn scope_serde_roundtrip() {
        let json = serde_json::json!("coordinator");
        let scope: SkillScope = serde_json::from_value(json).unwrap();
        assert_eq!(scope, SkillScope::Coordinator);

        let serialized = serde_json::to_value(scope).unwrap();
        assert_eq!(serialized, "coordinator");
    }

    #[test]
    fn source_tracking() {
        let registry = SkillRegistry::with_builtins();
        let rs = registry.get_registered("code_review").unwrap();
        assert_eq!(rs.source, SkillSource::Builtin);
    }

    #[test]
    fn list_registered_includes_source() {
        let mut registry = SkillRegistry::new();
        registry.register(
            Skill {
                name: "a".into(),
                description: "A".into(),
                prompt: "do a".into(),
                arguments: vec![],
                config: None,
                scope: SkillScope::Task,
            },
            SkillSource::Builtin,
        );
        registry.register(
            Skill {
                name: "b".into(),
                description: "B".into(),
                prompt: "do b".into(),
                arguments: vec![],
                config: None,
                scope: SkillScope::Task,
            },
            SkillSource::Runtime,
        );

        let all = registry.list_registered();
        assert_eq!(all.len(), 2);

        let builtin = registry.get_registered("a").unwrap();
        assert_eq!(builtin.source, SkillSource::Builtin);

        let runtime = registry.get_registered("b").unwrap();
        assert_eq!(runtime.source, SkillSource::Runtime);
    }

    #[test]
    fn project_skills_have_project_source() {
        let dir = tempfile::tempdir().unwrap();
        let skill_json = serde_json::json!({
            "name": "proj_skill",
            "description": "Project skill",
            "prompt": "do {thing}",
            "arguments": [
                { "name": "thing", "description": "What", "required": true }
            ]
        });
        std::fs::write(
            dir.path().join("proj_skill.json"),
            serde_json::to_string_pretty(&skill_json).unwrap(),
        )
        .unwrap();

        let mut registry = SkillRegistry::new();
        registry.load_from_dir(dir.path()).unwrap();

        let rs = registry.get_registered("proj_skill").unwrap();
        assert_eq!(rs.source, SkillSource::Project);
    }

    #[test]
    fn source_serde_roundtrip() {
        let json = serde_json::json!("runtime");
        let source: SkillSource = serde_json::from_value(json).unwrap();
        assert_eq!(source, SkillSource::Runtime);

        let serialized = serde_json::to_value(source).unwrap();
        assert_eq!(serialized, "runtime");
    }

    #[test]
    fn source_display() {
        assert_eq!(SkillSource::Builtin.to_string(), "builtin");
        assert_eq!(SkillSource::Global.to_string(), "global");
        assert_eq!(SkillSource::Project.to_string(), "project");
        assert_eq!(SkillSource::Runtime.to_string(), "runtime");
    }

    #[test]
    fn source_global_serde_roundtrip() {
        let json = serde_json::json!("global");
        let source: SkillSource = serde_json::from_value(json).unwrap();
        assert_eq!(source, SkillSource::Global);

        let serialized = serde_json::to_value(source).unwrap();
        assert_eq!(serialized, "global");
    }

    #[test]
    fn parse_skill_md_basic() {
        let content = "\
---
name: test-skill
description: A test skill for parsing.
metadata:
  arguments:
    - name: target
      description: What to test
      required: true
---

Run tests on {target}.

Report results.
";

        let skill = parse_skill_md(content).unwrap();
        assert_eq!(skill.name, "test-skill");
        assert_eq!(skill.description, "A test skill for parsing.");
        assert_eq!(skill.prompt, "Run tests on {target}.\n\nReport results.");
        assert_eq!(skill.arguments.len(), 1);
        assert_eq!(skill.arguments[0].name, "target");
        assert!(skill.arguments[0].required);
        assert_eq!(skill.scope, SkillScope::Task);
    }

    #[test]
    fn parse_skill_md_with_scope() {
        let content = "\
---
name: cps-coordinator-watcher
description: Watches things.
metadata:
  scope: coordinator
---

Watch stuff.
";
        let skill = parse_skill_md(content).unwrap();
        assert_eq!(skill.scope, SkillScope::Coordinator);
    }

    #[test]
    fn parse_skill_md_infers_scope_from_prefix() {
        let content = "\
---
name: cps-chain-deploy
description: Deploy chain.
---

Deploy stuff.
";
        let skill = parse_skill_md(content).unwrap();
        assert_eq!(skill.scope, SkillScope::Chain);
    }

    #[test]
    fn parse_skill_md_no_metadata() {
        let content = "\
---
name: simple
description: Simple skill.
---

Just do it.
";
        let skill = parse_skill_md(content).unwrap();
        assert_eq!(skill.name, "simple");
        assert!(skill.arguments.is_empty());
        assert_eq!(skill.scope, SkillScope::Task);
    }

    #[test]
    fn parse_skill_md_missing_frontmatter() {
        let result = parse_skill_md("no frontmatter here");
        assert!(result.is_err());
    }

    #[test]
    fn parse_skill_md_missing_closing_delimiter() {
        let result = parse_skill_md("---\nname: broken\n");
        assert!(result.is_err());
    }

    #[test]
    fn load_from_dir_with_skill_md_folders() {
        let dir = tempfile::tempdir().unwrap();

        // Create a SKILL.md folder.
        let skill_dir = dir.path().join("my-skill");
        std::fs::create_dir(&skill_dir).unwrap();
        std::fs::write(
            skill_dir.join("SKILL.md"),
            "\
---
name: my-skill
description: A folder-based skill.
metadata:
  arguments:
    - name: input
      description: The input
      required: true
---

Process {input}.
",
        )
        .unwrap();

        let mut registry = SkillRegistry::new();
        let count = registry.load_from_dir(dir.path()).unwrap();
        assert_eq!(count, 1);

        let skill = registry.get("my-skill").unwrap();
        assert_eq!(skill.description, "A folder-based skill.");
        assert_eq!(skill.prompt, "Process {input}.");
        assert_eq!(skill.arguments.len(), 1);
    }

    #[test]
    fn load_from_dir_mixed_formats() {
        let dir = tempfile::tempdir().unwrap();

        // SKILL.md folder.
        let skill_dir = dir.path().join("new-skill");
        std::fs::create_dir(&skill_dir).unwrap();
        std::fs::write(
            skill_dir.join("SKILL.md"),
            "---\nname: new-skill\ndescription: New format.\n---\n\nNew prompt.\n",
        )
        .unwrap();

        // Legacy JSON.
        let skill_json = serde_json::json!({
            "name": "old_skill",
            "description": "Legacy format",
            "prompt": "Old prompt",
            "arguments": []
        });
        std::fs::write(
            dir.path().join("old_skill.json"),
            serde_json::to_string_pretty(&skill_json).unwrap(),
        )
        .unwrap();

        let mut registry = SkillRegistry::new();
        let count = registry.load_from_dir(dir.path()).unwrap();
        assert_eq!(count, 2);
        assert!(registry.get("new-skill").is_some());
        assert!(registry.get("old_skill").is_some());
    }

    #[test]
    fn load_from_dir_with_source() {
        let dir = tempfile::tempdir().unwrap();
        let skill_dir = dir.path().join("global-skill");
        std::fs::create_dir(&skill_dir).unwrap();
        std::fs::write(
            skill_dir.join("SKILL.md"),
            "---\nname: global-skill\ndescription: Global.\n---\n\nDo global things.\n",
        )
        .unwrap();

        let mut registry = SkillRegistry::new();
        let count = registry
            .load_from_dir_with_source(dir.path(), SkillSource::Global)
            .unwrap();
        assert_eq!(count, 1);

        let rs = registry.get_registered("global-skill").unwrap();
        assert_eq!(rs.source, SkillSource::Global);
    }

    #[test]
    fn skill_md_folder_overrides_builtin() {
        let dir = tempfile::tempdir().unwrap();

        let skill_dir = dir.path().join("code_review");
        std::fs::create_dir(&skill_dir).unwrap();
        std::fs::write(
            skill_dir.join("SKILL.md"),
            "\
---
name: code_review
description: Custom review via SKILL.md.
metadata:
  arguments:
    - name: target
      description: What to review
      required: true
---

Custom review: {target}
",
        )
        .unwrap();

        let mut registry = SkillRegistry::with_builtins();
        assert_eq!(
            registry.get("code_review").unwrap().description,
            "Review code for bugs, style issues, and improvements."
        );

        registry.load_from_dir(dir.path()).unwrap();
        assert_eq!(
            registry.get("code_review").unwrap().description,
            "Custom review via SKILL.md."
        );
        assert_eq!(
            registry.get_registered("code_review").unwrap().source,
            SkillSource::Project
        );
    }
}
