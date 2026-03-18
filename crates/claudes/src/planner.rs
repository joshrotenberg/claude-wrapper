//! Planner — generates manifests from CLI arguments and (eventually) config files.
//!
//! The planner is the bridge between human-friendly inputs (CLI flags, TOML config)
//! and the fully resolved manifest that the runner understands.

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

use crate::manifest::{Isolation, Manifest, Task};

/// Options for generating a manifest from CLI inputs.
#[derive(Debug, Clone, Default)]
pub struct PlanOptions {
    /// Task prompts (one per task).
    pub prompts: Vec<String>,

    /// Model override.
    pub model: Option<String>,
    /// Fallback model override.
    pub fallback_model: Option<String>,
    /// Max turns override.
    pub max_turns: Option<u32>,
    /// Timeout override (in seconds).
    pub timeout_secs: Option<u64>,
    /// Budget override.
    pub max_budget_usd: Option<f64>,
    /// Effort override.
    pub effort: Option<String>,
    /// Permission mode override.
    pub permission_mode: Option<String>,
    /// Allowed tools override.
    pub allowed_tools: Option<Vec<String>>,
    /// Disallowed tools override.
    pub disallowed_tools: Option<Vec<String>>,
    /// Append system prompt override.
    pub append_system_prompt: Option<String>,
    /// MCP config override.
    pub mcp_config: Option<String>,
    /// Strict MCP config override.
    pub strict_mcp_config: Option<bool>,
    /// No session persistence override.
    pub no_session_persistence: Option<bool>,
    /// Isolation type override.
    pub isolation: Option<String>,
    /// Isolation base dir.
    pub isolation_base_dir: Option<String>,
}

impl PlanOptions {
    /// Create options from a single prompt.
    pub fn single(prompt: impl Into<String>) -> Self {
        Self {
            prompts: vec![prompt.into()],
            ..Default::default()
        }
    }
}

/// Generate a manifest from plan options.
pub fn plan(options: &PlanOptions) -> Manifest {
    let tasks: Vec<Task> = options
        .prompts
        .iter()
        .map(|prompt| {
            let name = generate_task_name(prompt);
            let branch = format!("claudes/{name}");

            let isolation = match options.isolation.as_deref() {
                Some("none") => Some(Isolation::None),
                Some("clone") => Some(Isolation::Clone {
                    base_dir: options
                        .isolation_base_dir
                        .clone()
                        .unwrap_or_else(|| ".worktrees".into()),
                }),
                // Default to worktree.
                _ => Some(Isolation::Worktree {
                    base_dir: options
                        .isolation_base_dir
                        .clone()
                        .unwrap_or_else(|| ".worktrees".into()),
                }),
            };

            Task {
                name,
                prompt: prompt.clone(),
                model: options.model.clone(),
                fallback_model: options.fallback_model.clone(),
                max_turns: options.max_turns,
                timeout_secs: options.timeout_secs,
                max_budget_usd: options.max_budget_usd,
                permission_mode: options.permission_mode.clone(),
                allowed_tools: options.allowed_tools.clone(),
                disallowed_tools: options.disallowed_tools.clone(),
                system_prompt: None,
                append_system_prompt: options.append_system_prompt.clone(),
                effort: options.effort.clone(),
                no_session_persistence: options.no_session_persistence,
                mcp_config: options.mcp_config.clone(),
                strict_mcp_config: options.strict_mcp_config,
                add_dirs: None,
                isolation,
                branch: Some(branch),
                env: None,
            }
        })
        .collect();

    Manifest::new(tasks)
}

/// Generate a task name from a prompt.
///
/// Takes the first few words of the prompt and appends a short hash
/// for uniqueness: `fix-the-pagination-bug-a3b2`
fn generate_task_name(prompt: &str) -> String {
    let slug: String = prompt
        .to_lowercase()
        .split_whitespace()
        .take(5)
        .collect::<Vec<_>>()
        .join("-")
        .chars()
        .filter(|c| c.is_alphanumeric() || *c == '-')
        .collect();

    let slug = slug.trim_matches('-').to_string();
    let slug = if slug.is_empty() {
        "task".to_string()
    } else {
        slug
    };

    let mut hasher = DefaultHasher::new();
    prompt.hash(&mut hasher);
    let hash = format!("{:04x}", hasher.finish() & 0xFFFF);

    format!("{slug}-{hash}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generate_name_from_prompt() {
        let name = generate_task_name("Fix the pagination bug in list.rs");
        assert!(name.starts_with("fix-the-pagination-bug-in"));
        assert!(name.len() > 10);
        // Should end with a 4-char hex hash.
        let parts: Vec<&str> = name.rsplitn(2, '-').collect();
        assert_eq!(parts[0].len(), 4);
    }

    #[test]
    fn generate_name_deterministic() {
        let a = generate_task_name("Fix the bug");
        let b = generate_task_name("Fix the bug");
        assert_eq!(a, b);
    }

    #[test]
    fn generate_name_different_prompts() {
        let a = generate_task_name("Fix the bug");
        let b = generate_task_name("Add the feature");
        assert_ne!(a, b);
    }

    #[test]
    fn plan_single_prompt() {
        let opts = PlanOptions::single("Fix the bug");
        let manifest = plan(&opts);
        assert_eq!(manifest.tasks.len(), 1);
        assert_eq!(manifest.tasks[0].prompt, "Fix the bug");
        assert!(
            manifest.tasks[0]
                .branch
                .as_ref()
                .unwrap()
                .starts_with("claudes/")
        );
    }

    #[test]
    fn plan_multiple_prompts() {
        let opts = PlanOptions {
            prompts: vec!["Fix A".into(), "Fix B".into(), "Fix C".into()],
            ..Default::default()
        };
        let manifest = plan(&opts);
        assert_eq!(manifest.tasks.len(), 3);
    }

    #[test]
    fn plan_applies_overrides() {
        let opts = PlanOptions {
            prompts: vec!["task".into()],
            model: Some("opus".into()),
            max_turns: Some(50),
            effort: Some("high".into()),
            ..Default::default()
        };
        let manifest = plan(&opts);
        let task = &manifest.tasks[0];
        assert_eq!(task.model.as_deref(), Some("opus"));
        assert_eq!(task.max_turns, Some(50));
        assert_eq!(task.effort.as_deref(), Some("high"));
    }

    #[test]
    fn plan_default_isolation_is_worktree() {
        let opts = PlanOptions::single("task");
        let manifest = plan(&opts);
        match &manifest.tasks[0].isolation {
            Some(Isolation::Worktree { base_dir }) => {
                assert_eq!(base_dir, ".worktrees");
            }
            other => panic!("expected worktree isolation, got {other:?}"),
        }
    }

    #[test]
    fn plan_no_isolation() {
        let opts = PlanOptions {
            prompts: vec!["task".into()],
            isolation: Some("none".into()),
            ..Default::default()
        };
        let manifest = plan(&opts);
        assert!(matches!(manifest.tasks[0].isolation, Some(Isolation::None)));
    }
}
