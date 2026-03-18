//! Manifest schema — the core abstraction.
//!
//! A manifest is a fully resolved JSON document describing exactly what to execute.
//! Every field is explicit. No inheritance, no defaults, no references to profiles.
//! What you see is what executes.

use std::collections::HashMap;
use std::path::Path;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// The manifest — a fully resolved, self-contained execution plan.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Manifest {
    /// Schema version (currently 1).
    pub version: u32,

    /// When this manifest was created.
    pub created_at: DateTime<Utc>,

    /// Manifest-level defaults applied to all tasks.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub shared: Option<Shared>,

    /// One or more tasks to execute.
    pub tasks: Vec<Task>,
}

impl Manifest {
    /// Create a new manifest with the given tasks.
    pub fn new(tasks: Vec<Task>) -> Self {
        Self {
            version: 1,
            created_at: Utc::now(),
            shared: None,
            tasks,
        }
    }

    /// Return a new `Manifest` where each task has been merged with `shared` defaults.
    ///
    /// Task-level fields take precedence. `None` task fields inherit from `shared`.
    /// `post_hooks` are special: shared hooks are prepended to task-level hooks.
    pub fn resolve(&self) -> Manifest {
        let tasks = self
            .tasks
            .iter()
            .map(|task| {
                let Some(shared) = &self.shared else {
                    return task.clone();
                };
                Task {
                    name: task.name.clone(),
                    prompt: task.prompt.clone(),
                    model: task.model.clone().or_else(|| shared.model.clone()),
                    fallback_model: task.fallback_model.clone(),
                    max_turns: task.max_turns.or(shared.max_turns),
                    timeout_secs: task.timeout_secs.or(shared.timeout_secs),
                    max_budget_usd: task.max_budget_usd.or(shared.max_budget_usd),
                    permission_mode: task
                        .permission_mode
                        .clone()
                        .or_else(|| shared.permission_mode.clone()),
                    allowed_tools: task
                        .allowed_tools
                        .clone()
                        .or_else(|| shared.allowed_tools.clone()),
                    disallowed_tools: task
                        .disallowed_tools
                        .clone()
                        .or_else(|| shared.disallowed_tools.clone()),
                    system_prompt: task
                        .system_prompt
                        .clone()
                        .or_else(|| shared.system_prompt.clone()),
                    append_system_prompt: task
                        .append_system_prompt
                        .clone()
                        .or_else(|| shared.append_system_prompt.clone()),
                    effort: task.effort.clone().or_else(|| shared.effort.clone()),
                    no_session_persistence: task
                        .no_session_persistence
                        .or(shared.no_session_persistence),
                    mcp_config: task
                        .mcp_config
                        .clone()
                        .or_else(|| shared.mcp_config.clone()),
                    strict_mcp_config: task.strict_mcp_config.or(shared.strict_mcp_config),
                    add_dirs: task.add_dirs.clone().or_else(|| shared.add_dirs.clone()),
                    isolation: task.isolation.clone().or_else(|| shared.isolation.clone()),
                    branch: task.branch.clone().or_else(|| shared.branch.clone()),
                    env: task.env.clone().or_else(|| shared.env.clone()),
                    pre_hooks: match (&shared.pre_hooks, &task.pre_hooks) {
                        (Some(s), Some(t)) => {
                            let mut merged = s.clone();
                            merged.extend(t.iter().cloned());
                            Some(merged)
                        }
                        (Some(s), None) => Some(s.clone()),
                        (None, t) => t.clone(),
                    },
                    post_hooks: match (&shared.post_hooks, &task.post_hooks) {
                        (Some(s), Some(t)) => {
                            let mut merged = s.clone();
                            merged.extend(t.iter().cloned());
                            Some(merged)
                        }
                        (Some(s), None) => Some(s.clone()),
                        (None, t) => t.clone(),
                    },
                }
            })
            .collect();

        Manifest {
            version: self.version,
            created_at: self.created_at,
            shared: self.shared.clone(),
            tasks,
        }
    }

    /// Parse a manifest from a TOML string.
    pub fn from_toml(s: &str) -> Result<Manifest, crate::Error> {
        Ok(toml::from_str(s)?)
    }

    /// Parse a manifest from a file, detecting format by extension (`.json` or `.toml`).
    pub fn from_file(path: &Path) -> Result<Manifest, crate::Error> {
        let contents = std::fs::read_to_string(path)?;
        match path.extension().and_then(|e| e.to_str()) {
            Some("json") => Ok(serde_json::from_str(&contents)?),
            Some("toml") => Ok(toml::from_str(&contents)?),
            _ => Err(crate::Error::InvalidManifest(format!(
                "unknown file extension: {}",
                path.display()
            ))),
        }
    }

    /// Validate the manifest, returning errors for any problems.
    pub fn validate(&self) -> Result<(), Vec<String>> {
        let mut errors = Vec::new();

        if self.version != 1 {
            errors.push(format!("unsupported manifest version: {}", self.version));
        }

        if self.tasks.is_empty() {
            errors.push("manifest must contain at least one task".into());
        }

        // Check for duplicate task names.
        let mut seen = std::collections::HashSet::new();
        for task in &self.tasks {
            if !seen.insert(&task.name) {
                errors.push(format!("duplicate task name: {}", task.name));
            }
        }

        for task in &self.tasks {
            if let Err(task_errors) = task.validate() {
                for e in task_errors {
                    errors.push(format!("task '{}': {}", task.name, e));
                }
            }
        }

        if errors.is_empty() {
            Ok(())
        } else {
            Err(errors)
        }
    }
}

/// Manifest-level defaults applied to all tasks.
///
/// All fields are optional. Task-level fields take precedence; if a task field
/// is `None`, the value from `Shared` is used. `pre_hooks` and `post_hooks` are
/// exceptions: shared hooks are **prepended** to any task-level hooks.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Shared {
    /// Model alias or full ID.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,

    /// Conversation turn limit.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_turns: Option<u32>,

    /// Process timeout in seconds.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timeout_secs: Option<u64>,

    /// Spending cap in USD.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_budget_usd: Option<f64>,

    /// Permission mode.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub permission_mode: Option<String>,

    /// Tool allow list.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub allowed_tools: Option<Vec<String>>,

    /// Tool deny list.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub disallowed_tools: Option<Vec<String>>,

    /// Replace the default system prompt entirely.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub system_prompt: Option<String>,

    /// Append to the default system prompt.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub append_system_prompt: Option<String>,

    /// Effort level: low, medium, high.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub effort: Option<String>,

    /// Don't save session state.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub no_session_persistence: Option<bool>,

    /// Path to MCP config file.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mcp_config: Option<String>,

    /// Only use MCP servers from config.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub strict_mcp_config: Option<bool>,

    /// Additional accessible directories.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub add_dirs: Option<Vec<String>>,

    /// Isolation strategy.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub isolation: Option<Isolation>,

    /// Git branch name.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub branch: Option<String>,

    /// Environment variables.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub env: Option<HashMap<String, String>>,

    /// Shell commands to run before each task starts.
    /// These are **prepended** to any task-level pre_hooks.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pre_hooks: Option<Vec<String>>,

    /// Shell commands to run after each task completes successfully.
    /// These are **prepended** to any task-level post_hooks.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub post_hooks: Option<Vec<String>>,
}

/// A fully resolved task. Every field is explicit.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Task {
    /// Task identifier (used for worktree/branch naming, logs).
    pub name: String,

    /// The task prompt.
    pub prompt: String,

    /// Model alias or full ID.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,

    /// Fallback model if primary is unavailable.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fallback_model: Option<String>,

    /// Conversation turn limit.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_turns: Option<u32>,

    /// Process timeout in seconds.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timeout_secs: Option<u64>,

    /// Spending cap in USD.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_budget_usd: Option<f64>,

    /// Permission mode.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub permission_mode: Option<String>,

    /// Tool allow list.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub allowed_tools: Option<Vec<String>>,

    /// Tool deny list.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub disallowed_tools: Option<Vec<String>>,

    /// Replace the default system prompt entirely.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub system_prompt: Option<String>,

    /// Append to the default system prompt.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub append_system_prompt: Option<String>,

    /// Effort level: low, medium, high.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub effort: Option<String>,

    /// Don't save session state.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub no_session_persistence: Option<bool>,

    /// Path to MCP config file.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mcp_config: Option<String>,

    /// Only use MCP servers from config.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub strict_mcp_config: Option<bool>,

    /// Additional accessible directories.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub add_dirs: Option<Vec<String>>,

    /// Isolation strategy.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub isolation: Option<Isolation>,

    /// Git branch name for this task.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub branch: Option<String>,

    /// Environment variables.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub env: Option<HashMap<String, String>>,

    /// Shell commands to run before the task starts.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pre_hooks: Option<Vec<String>>,

    /// Shell commands to run after the task completes successfully.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub post_hooks: Option<Vec<String>>,
}

impl Task {
    /// Create a new task with the given name and prompt.
    pub fn new(name: impl Into<String>, prompt: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            prompt: prompt.into(),
            model: None,
            fallback_model: None,
            max_turns: None,
            timeout_secs: None,
            max_budget_usd: None,
            permission_mode: None,
            allowed_tools: None,
            disallowed_tools: None,
            system_prompt: None,
            append_system_prompt: None,
            effort: None,
            no_session_persistence: None,
            mcp_config: None,
            strict_mcp_config: None,
            add_dirs: None,
            isolation: None,
            branch: None,
            env: None,
            pre_hooks: None,
            post_hooks: None,
        }
    }

    /// Validate this task.
    fn validate(&self) -> Result<(), Vec<String>> {
        let mut errors = Vec::new();

        if self.name.is_empty() {
            errors.push("name must not be empty".into());
        }

        if self.prompt.is_empty() {
            errors.push("prompt must not be empty".into());
        }

        if let Some(effort) = &self.effort {
            match effort.as_str() {
                "low" | "medium" | "high" => {}
                other => errors.push(format!("invalid effort level: {other}")),
            }
        }

        if let Some(mode) = &self.permission_mode {
            match mode.as_str() {
                "default" | "acceptEdits" | "bypassPermissions" | "dontAsk" | "plan" | "auto" => {}
                other => errors.push(format!("invalid permission mode: {other}")),
            }
        }

        if let Some(budget) = self.max_budget_usd
            && budget <= 0.0
        {
            errors.push("max_budget_usd must be positive".into());
        }

        if let Some(hooks) = &self.pre_hooks {
            for (i, hook) in hooks.iter().enumerate() {
                if hook.is_empty() {
                    errors.push(format!("pre_hooks[{i}] must not be empty"));
                }
            }
        }

        if let Some(hooks) = &self.post_hooks {
            for (i, hook) in hooks.iter().enumerate() {
                if hook.is_empty() {
                    errors.push(format!("post_hooks[{i}] must not be empty"));
                }
            }
        }

        if errors.is_empty() {
            Ok(())
        } else {
            Err(errors)
        }
    }
}

/// Builder for [`Task`].
///
/// Required fields (`name` and `prompt`) are passed to [`TaskBuilder::new`].
/// All other fields are optional and set via chained setter methods.
///
/// # Example
///
/// ```
/// use claudes::manifest::TaskBuilder;
///
/// let task = TaskBuilder::new("fix-bug", "Fix the bug in main.rs")
///     .model("claude-opus-4-6")
///     .max_turns(10)
///     .build();
///
/// assert_eq!(task.name, "fix-bug");
/// assert_eq!(task.model.as_deref(), Some("claude-opus-4-6"));
/// assert_eq!(task.max_turns, Some(10));
/// ```
pub struct TaskBuilder {
    task: Task,
}

impl TaskBuilder {
    /// Create a new builder with the required `name` and `prompt`.
    pub fn new(name: impl Into<String>, prompt: impl Into<String>) -> Self {
        Self {
            task: Task::new(name, prompt),
        }
    }

    /// Set the model alias or full model ID.
    pub fn model(mut self, model: impl Into<String>) -> Self {
        self.task.model = Some(model.into());
        self
    }

    /// Set the fallback model.
    pub fn fallback_model(mut self, fallback_model: impl Into<String>) -> Self {
        self.task.fallback_model = Some(fallback_model.into());
        self
    }

    /// Set the conversation turn limit.
    pub fn max_turns(mut self, max_turns: u32) -> Self {
        self.task.max_turns = Some(max_turns);
        self
    }

    /// Set the process timeout in seconds.
    pub fn timeout_secs(mut self, timeout_secs: u64) -> Self {
        self.task.timeout_secs = Some(timeout_secs);
        self
    }

    /// Set the spending cap in USD.
    pub fn max_budget_usd(mut self, max_budget_usd: f64) -> Self {
        self.task.max_budget_usd = Some(max_budget_usd);
        self
    }

    /// Set the permission mode.
    pub fn permission_mode(mut self, permission_mode: impl Into<String>) -> Self {
        self.task.permission_mode = Some(permission_mode.into());
        self
    }

    /// Set the tool allow list.
    pub fn allowed_tools(mut self, allowed_tools: Vec<String>) -> Self {
        self.task.allowed_tools = Some(allowed_tools);
        self
    }

    /// Set the tool deny list.
    pub fn disallowed_tools(mut self, disallowed_tools: Vec<String>) -> Self {
        self.task.disallowed_tools = Some(disallowed_tools);
        self
    }

    /// Replace the default system prompt entirely.
    pub fn system_prompt(mut self, system_prompt: impl Into<String>) -> Self {
        self.task.system_prompt = Some(system_prompt.into());
        self
    }

    /// Append to the default system prompt.
    pub fn append_system_prompt(mut self, append_system_prompt: impl Into<String>) -> Self {
        self.task.append_system_prompt = Some(append_system_prompt.into());
        self
    }

    /// Set the effort level (`"low"`, `"medium"`, or `"high"`).
    pub fn effort(mut self, effort: impl Into<String>) -> Self {
        self.task.effort = Some(effort.into());
        self
    }

    /// Disable session state persistence.
    pub fn no_session_persistence(mut self, no_session_persistence: bool) -> Self {
        self.task.no_session_persistence = Some(no_session_persistence);
        self
    }

    /// Set the path to the MCP config file.
    pub fn mcp_config(mut self, mcp_config: impl Into<String>) -> Self {
        self.task.mcp_config = Some(mcp_config.into());
        self
    }

    /// Only use MCP servers from the config file.
    pub fn strict_mcp_config(mut self, strict_mcp_config: bool) -> Self {
        self.task.strict_mcp_config = Some(strict_mcp_config);
        self
    }

    /// Set additional accessible directories.
    pub fn add_dirs(mut self, add_dirs: Vec<String>) -> Self {
        self.task.add_dirs = Some(add_dirs);
        self
    }

    /// Set the isolation strategy.
    pub fn isolation(mut self, isolation: Isolation) -> Self {
        self.task.isolation = Some(isolation);
        self
    }

    /// Set the git branch name for this task.
    pub fn branch(mut self, branch: impl Into<String>) -> Self {
        self.task.branch = Some(branch.into());
        self
    }

    /// Set environment variables.
    pub fn env(mut self, env: HashMap<String, String>) -> Self {
        self.task.env = Some(env);
        self
    }

    /// Set shell commands to run before the task starts.
    pub fn pre_hooks(mut self, pre_hooks: Vec<String>) -> Self {
        self.task.pre_hooks = Some(pre_hooks);
        self
    }

    /// Set shell commands to run after the task completes successfully.
    pub fn post_hooks(mut self, post_hooks: Vec<String>) -> Self {
        self.task.post_hooks = Some(post_hooks);
        self
    }

    /// Build the [`Task`].
    pub fn build(self) -> Task {
        self.task
    }
}

/// Isolation strategy for task execution.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum Isolation {
    /// Run in a git worktree.
    #[serde(rename = "worktree")]
    Worktree {
        /// Directory for worktrees.
        base_dir: String,
    },

    /// Run in a full clone.
    #[serde(rename = "clone")]
    Clone {
        /// Directory for clones.
        base_dir: String,
    },

    /// No isolation — run in the current directory.
    #[serde(rename = "none")]
    None,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_manifest() {
        let manifest = Manifest::new(vec![
            Task::new("fix-bug", "Fix the bug in main.rs"),
            Task::new("add-tests", "Add unit tests"),
        ]);

        let json = serde_json::to_string_pretty(&manifest).unwrap();
        let parsed: Manifest = serde_json::from_str(&json).unwrap();

        assert_eq!(parsed.version, 1);
        assert_eq!(parsed.tasks.len(), 2);
        assert_eq!(parsed.tasks[0].name, "fix-bug");
        assert_eq!(parsed.tasks[1].name, "add-tests");
    }

    #[test]
    fn roundtrip_isolation_variants() {
        let task_wt = Task {
            isolation: Some(Isolation::Worktree {
                base_dir: ".worktrees".into(),
            }),
            ..Task::new("wt", "test")
        };
        let json = serde_json::to_value(&task_wt).unwrap();
        assert_eq!(json["isolation"]["type"], "worktree");
        assert_eq!(json["isolation"]["base_dir"], ".worktrees");

        let task_none = Task {
            isolation: Some(Isolation::None),
            ..Task::new("no-iso", "test")
        };
        let json = serde_json::to_value(&task_none).unwrap();
        assert_eq!(json["isolation"]["type"], "none");
    }

    #[test]
    fn validate_good_manifest() {
        let manifest = Manifest::new(vec![Task::new("t1", "do something")]);
        assert!(manifest.validate().is_ok());
    }

    #[test]
    fn validate_empty_tasks() {
        let manifest = Manifest::new(vec![]);
        let errs = manifest.validate().unwrap_err();
        assert!(errs.iter().any(|e| e.contains("at least one task")));
    }

    #[test]
    fn validate_duplicate_names() {
        let manifest = Manifest::new(vec![
            Task::new("same", "first"),
            Task::new("same", "second"),
        ]);
        let errs = manifest.validate().unwrap_err();
        assert!(errs.iter().any(|e| e.contains("duplicate task name")));
    }

    #[test]
    fn validate_bad_effort() {
        let mut task = Task::new("t", "prompt");
        task.effort = Some("max".into());
        let manifest = Manifest::new(vec![task]);
        let errs = manifest.validate().unwrap_err();
        assert!(errs.iter().any(|e| e.contains("invalid effort")));
    }

    #[test]
    fn validate_bad_permission_mode() {
        let mut task = Task::new("t", "prompt");
        task.permission_mode = Some("yolo".into());
        let manifest = Manifest::new(vec![task]);
        let errs = manifest.validate().unwrap_err();
        assert!(errs.iter().any(|e| e.contains("invalid permission mode")));
    }

    #[test]
    fn skip_serializing_none_fields() {
        let task = Task::new("minimal", "just a prompt");
        let json = serde_json::to_value(&task).unwrap();
        let obj = json.as_object().unwrap();
        // Only name and prompt should be present.
        assert!(obj.contains_key("name"));
        assert!(obj.contains_key("prompt"));
        assert!(!obj.contains_key("model"));
        assert!(!obj.contains_key("isolation"));
        assert!(!obj.contains_key("env"));
    }

    #[test]
    fn task_builder_required_fields() {
        let task = TaskBuilder::new("my-task", "do something").build();
        assert_eq!(task.name, "my-task");
        assert_eq!(task.prompt, "do something");
        assert!(task.model.is_none());
        assert!(task.max_turns.is_none());
    }

    #[test]
    fn task_builder_all_optional_fields() {
        let env: HashMap<String, String> = [("KEY".into(), "val".into())].into();
        let task = TaskBuilder::new("t", "p")
            .model("claude-opus-4-6")
            .fallback_model("claude-haiku-4-5-20251001")
            .max_turns(5)
            .timeout_secs(120)
            .max_budget_usd(1.5)
            .permission_mode("bypassPermissions")
            .allowed_tools(vec!["Bash".into()])
            .disallowed_tools(vec!["Write".into()])
            .system_prompt("sys")
            .append_system_prompt("append")
            .effort("high")
            .no_session_persistence(true)
            .mcp_config("/etc/mcp.json")
            .strict_mcp_config(true)
            .add_dirs(vec!["/tmp".into()])
            .isolation(Isolation::None)
            .branch("feat/t")
            .env(env.clone())
            .pre_hooks(vec!["echo ready".into()])
            .post_hooks(vec!["echo done".into()])
            .build();

        assert_eq!(task.model.as_deref(), Some("claude-opus-4-6"));
        assert_eq!(
            task.fallback_model.as_deref(),
            Some("claude-haiku-4-5-20251001")
        );
        assert_eq!(task.max_turns, Some(5));
        assert_eq!(task.timeout_secs, Some(120));
        assert_eq!(task.max_budget_usd, Some(1.5));
        assert_eq!(task.permission_mode.as_deref(), Some("bypassPermissions"));
        assert_eq!(
            task.allowed_tools.as_deref(),
            Some(["Bash".to_string()].as_slice())
        );
        assert_eq!(
            task.disallowed_tools.as_deref(),
            Some(["Write".to_string()].as_slice())
        );
        assert_eq!(task.system_prompt.as_deref(), Some("sys"));
        assert_eq!(task.append_system_prompt.as_deref(), Some("append"));
        assert_eq!(task.effort.as_deref(), Some("high"));
        assert_eq!(task.no_session_persistence, Some(true));
        assert_eq!(task.mcp_config.as_deref(), Some("/etc/mcp.json"));
        assert_eq!(task.strict_mcp_config, Some(true));
        assert_eq!(
            task.add_dirs.as_deref(),
            Some(["/tmp".to_string()].as_slice())
        );
        assert!(matches!(task.isolation, Some(Isolation::None)));
        assert_eq!(task.branch.as_deref(), Some("feat/t"));
        assert_eq!(task.env, Some(env));
        assert_eq!(
            task.pre_hooks.as_deref(),
            Some(["echo ready".to_string()].as_slice())
        );
        assert_eq!(
            task.post_hooks.as_deref(),
            Some(["echo done".to_string()].as_slice())
        );
    }

    #[test]
    fn task_builder_produces_valid_task() {
        let task = TaskBuilder::new("valid", "do the thing")
            .effort("low")
            .permission_mode("default")
            .max_budget_usd(5.0)
            .build();
        let manifest = Manifest::new(vec![task]);
        assert!(manifest.validate().is_ok());
    }

    #[test]
    fn task_builder_invalid_effort_fails_validation() {
        let task = TaskBuilder::new("t", "p").effort("turbo").build();
        let manifest = Manifest::new(vec![task]);
        let errs = manifest.validate().unwrap_err();
        assert!(errs.iter().any(|e| e.contains("invalid effort")));
    }

    #[test]
    fn validate_empty_pre_hook_entry() {
        let mut task = Task::new("t", "prompt");
        task.pre_hooks = Some(vec!["echo ok".into(), "".into()]);
        let manifest = Manifest::new(vec![task]);
        let errs = manifest.validate().unwrap_err();
        assert!(errs.iter().any(|e| e.contains("pre_hooks[1]")));
    }

    #[test]
    fn validate_valid_pre_hooks() {
        let mut task = Task::new("t", "prompt");
        task.pre_hooks = Some(vec!["echo ok".into(), "cargo fmt".into()]);
        let manifest = Manifest::new(vec![task]);
        assert!(manifest.validate().is_ok());
    }

    #[test]
    fn validate_empty_post_hook_entry() {
        let mut task = Task::new("t", "prompt");
        task.post_hooks = Some(vec!["echo ok".into(), "".into()]);
        let manifest = Manifest::new(vec![task]);
        let errs = manifest.validate().unwrap_err();
        assert!(errs.iter().any(|e| e.contains("post_hooks[1]")));
    }

    #[test]
    fn validate_valid_post_hooks() {
        let mut task = Task::new("t", "prompt");
        task.post_hooks = Some(vec!["echo ok".into(), "cargo fmt".into()]);
        let manifest = Manifest::new(vec![task]);
        assert!(manifest.validate().is_ok());
    }

    #[test]
    fn task_builder_pre_hooks() {
        let task = TaskBuilder::new("t", "p")
            .pre_hooks(vec!["echo ready".into()])
            .build();
        assert_eq!(
            task.pre_hooks.as_deref(),
            Some(["echo ready".to_string()].as_slice())
        );
    }

    #[test]
    fn skip_serializing_pre_hooks_when_none() {
        let task = Task::new("minimal", "just a prompt");
        let json = serde_json::to_value(&task).unwrap();
        assert!(!json.as_object().unwrap().contains_key("pre_hooks"));
    }

    #[test]
    fn task_builder_post_hooks() {
        let task = TaskBuilder::new("t", "p")
            .post_hooks(vec!["echo done".into()])
            .build();
        assert_eq!(
            task.post_hooks.as_deref(),
            Some(["echo done".to_string()].as_slice())
        );
    }

    #[test]
    fn skip_serializing_post_hooks_when_none() {
        let task = Task::new("minimal", "just a prompt");
        let json = serde_json::to_value(&task).unwrap();
        assert!(!json.as_object().unwrap().contains_key("post_hooks"));
    }

    #[test]
    fn deserialize_from_json_with_extras_ignored() {
        let json = r#"{
            "version": 1,
            "created_at": "2026-03-18T10:30:00Z",
            "tasks": [{
                "name": "t1",
                "prompt": "do it",
                "model": "opus",
                "unknown_field": true
            }]
        }"#;
        // Unknown fields should not cause an error (we don't deny_unknown_fields).
        let manifest: Manifest = serde_json::from_str(json).unwrap();
        assert_eq!(manifest.tasks[0].model.as_deref(), Some("opus"));
    }

    #[test]
    fn resolve_shared_fills_missing_task_fields() {
        let mut manifest = Manifest::new(vec![Task::new("t1", "do it")]);
        manifest.shared = Some(Shared {
            model: Some("claude-opus-4-6".into()),
            max_turns: Some(5),
            effort: Some("high".into()),
            ..Default::default()
        });

        let resolved = manifest.resolve();
        let task = &resolved.tasks[0];
        assert_eq!(task.model.as_deref(), Some("claude-opus-4-6"));
        assert_eq!(task.max_turns, Some(5));
        assert_eq!(task.effort.as_deref(), Some("high"));
    }

    #[test]
    fn resolve_task_fields_override_shared() {
        let mut task = Task::new("t1", "do it");
        task.model = Some("claude-haiku-4-5-20251001".into());
        task.max_turns = Some(10);

        let mut manifest = Manifest::new(vec![task]);
        manifest.shared = Some(Shared {
            model: Some("claude-opus-4-6".into()),
            max_turns: Some(5),
            ..Default::default()
        });

        let resolved = manifest.resolve();
        let t = &resolved.tasks[0];
        assert_eq!(t.model.as_deref(), Some("claude-haiku-4-5-20251001"));
        assert_eq!(t.max_turns, Some(10));
    }

    #[test]
    fn resolve_no_shared_is_noop() {
        let mut task = Task::new("t1", "do it");
        task.model = Some("opus".into());
        let manifest = Manifest::new(vec![task]);

        let resolved = manifest.resolve();
        assert_eq!(resolved.tasks[0].model.as_deref(), Some("opus"));
        assert_eq!(resolved.tasks[0].max_turns, None);
    }

    #[test]
    fn resolve_shared_pre_hooks_prepended_to_task_hooks() {
        let mut task = Task::new("t1", "do it");
        task.pre_hooks = Some(vec!["echo task".into()]);

        let mut manifest = Manifest::new(vec![task]);
        manifest.shared = Some(Shared {
            pre_hooks: Some(vec!["echo shared".into()]),
            ..Default::default()
        });

        let resolved = manifest.resolve();
        assert_eq!(
            resolved.tasks[0].pre_hooks.as_deref(),
            Some(["echo shared".to_string(), "echo task".to_string()].as_slice())
        );
    }

    #[test]
    fn resolve_shared_pre_hooks_only_when_task_has_none() {
        let mut manifest = Manifest::new(vec![Task::new("t1", "do it")]);
        manifest.shared = Some(Shared {
            pre_hooks: Some(vec!["echo shared".into()]),
            ..Default::default()
        });

        let resolved = manifest.resolve();
        assert_eq!(
            resolved.tasks[0].pre_hooks.as_deref(),
            Some(["echo shared".to_string()].as_slice())
        );
    }

    #[test]
    fn resolve_shared_post_hooks_prepended_to_task_hooks() {
        let mut task = Task::new("t1", "do it");
        task.post_hooks = Some(vec!["echo task".into()]);

        let mut manifest = Manifest::new(vec![task]);
        manifest.shared = Some(Shared {
            post_hooks: Some(vec!["echo shared".into()]),
            ..Default::default()
        });

        let resolved = manifest.resolve();
        assert_eq!(
            resolved.tasks[0].post_hooks.as_deref(),
            Some(["echo shared".to_string(), "echo task".to_string()].as_slice())
        );
    }

    #[test]
    fn resolve_shared_post_hooks_only_when_task_has_none() {
        let mut manifest = Manifest::new(vec![Task::new("t1", "do it")]);
        manifest.shared = Some(Shared {
            post_hooks: Some(vec!["echo shared".into()]),
            ..Default::default()
        });

        let resolved = manifest.resolve();
        assert_eq!(
            resolved.tasks[0].post_hooks.as_deref(),
            Some(["echo shared".to_string()].as_slice())
        );
    }

    #[test]
    fn from_toml_with_tasks() {
        let toml = r#"
version = 1
created_at = "2026-03-18T10:30:00Z"

[[tasks]]
name = "fix-bug"
prompt = "Fix the bug in main.rs"

[[tasks]]
name = "add-tests"
prompt = "Add unit tests"
"#;
        let manifest = Manifest::from_toml(toml).unwrap();
        assert_eq!(manifest.version, 1);
        assert_eq!(manifest.tasks.len(), 2);
        assert_eq!(manifest.tasks[0].name, "fix-bug");
        assert_eq!(manifest.tasks[1].name, "add-tests");
        assert!(manifest.shared.is_none());
    }

    #[test]
    fn from_toml_with_shared_block() {
        let toml = r#"
version = 1
created_at = "2026-03-18T10:30:00Z"

[shared]
model = "claude-opus-4-6"
max_turns = 5

[[tasks]]
name = "t1"
prompt = "do it"
"#;
        let manifest = Manifest::from_toml(toml).unwrap();
        assert_eq!(manifest.tasks.len(), 1);
        let shared = manifest.shared.as_ref().unwrap();
        assert_eq!(shared.model.as_deref(), Some("claude-opus-4-6"));
        assert_eq!(shared.max_turns, Some(5));
    }

    #[test]
    fn from_toml_invalid_returns_error() {
        let result = Manifest::from_toml("not valid toml [[[");
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("toml error"));
    }

    #[test]
    fn from_file_toml() {
        use std::io::Write;
        let mut f = tempfile::Builder::new().suffix(".toml").tempfile().unwrap();
        write!(
            f,
            r#"version = 1
created_at = "2026-03-18T10:30:00Z"

[[tasks]]
name = "t1"
prompt = "do it"
"#
        )
        .unwrap();
        let manifest = Manifest::from_file(f.path()).unwrap();
        assert_eq!(manifest.tasks[0].name, "t1");
    }

    #[test]
    fn from_file_json() {
        use std::io::Write;
        let mut f = tempfile::Builder::new().suffix(".json").tempfile().unwrap();
        write!(
            f,
            r#"{{"version":1,"created_at":"2026-03-18T10:30:00Z","tasks":[{{"name":"t1","prompt":"do it"}}]}}"#
        )
        .unwrap();
        let manifest = Manifest::from_file(f.path()).unwrap();
        assert_eq!(manifest.tasks[0].name, "t1");
    }

    #[test]
    fn from_file_unknown_extension_errors() {
        use std::io::Write;
        let mut f = tempfile::Builder::new().suffix(".yaml").tempfile().unwrap();
        write!(f, "").unwrap();
        let err = Manifest::from_file(f.path()).unwrap_err().to_string();
        assert!(err.contains("unknown file extension"));
    }

    #[test]
    fn shared_not_serialized_when_none() {
        let manifest = Manifest::new(vec![Task::new("t", "p")]);
        let json = serde_json::to_value(&manifest).unwrap();
        assert!(!json.as_object().unwrap().contains_key("shared"));
    }

    #[test]
    fn shared_roundtrip() {
        let json = r#"{
            "version": 1,
            "created_at": "2026-03-18T10:30:00Z",
            "shared": { "model": "claude-opus-4-6", "max_turns": 5 },
            "tasks": [{ "name": "t1", "prompt": "do it" }]
        }"#;
        let manifest: Manifest = serde_json::from_str(json).unwrap();
        assert_eq!(
            manifest.shared.as_ref().unwrap().model.as_deref(),
            Some("claude-opus-4-6")
        );
        let resolved = manifest.resolve();
        assert_eq!(resolved.tasks[0].model.as_deref(), Some("claude-opus-4-6"));
        assert_eq!(resolved.tasks[0].max_turns, Some(5));
    }
}
