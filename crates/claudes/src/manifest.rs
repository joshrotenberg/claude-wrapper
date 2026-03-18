//! Manifest schema — the core abstraction.
//!
//! A manifest is a fully resolved JSON document describing exactly what to execute.
//! Every field is explicit. No inheritance, no defaults, no references to profiles.
//! What you see is what executes.

use std::collections::HashMap;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// The manifest — a fully resolved, self-contained execution plan.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Manifest {
    /// Schema version (currently 1).
    pub version: u32,

    /// When this manifest was created.
    pub created_at: DateTime<Utc>,

    /// One or more tasks to execute.
    pub tasks: Vec<Task>,
}

impl Manifest {
    /// Create a new manifest with the given tasks.
    pub fn new(tasks: Vec<Task>) -> Self {
        Self {
            version: 1,
            created_at: Utc::now(),
            tasks,
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

        if errors.is_empty() {
            Ok(())
        } else {
            Err(errors)
        }
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
}
