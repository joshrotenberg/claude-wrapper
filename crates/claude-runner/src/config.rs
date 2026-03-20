//! Configuration — the route-based config model.
//!
//! Each route maps a type (issue/pr) + label to a workflow template
//! with its own concurrency, polling frequency, and agent settings.

use std::collections::HashMap;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};
use crate::github::IssueCandidate;

/// Top-level runner configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunnerConfig {
    /// Global settings.
    pub global: GlobalConfig,

    /// Security settings.
    #[serde(default)]
    pub security: SecurityConfig,

    /// Global validation commands (run between stages).
    #[serde(default)]
    pub validation: ValidationConfig,

    /// Named routes — the core routing table.
    #[serde(default)]
    pub routes: HashMap<String, Route>,

    /// Custom workflow templates (override or extend built-ins).
    #[serde(default)]
    pub workflow_templates: Vec<crate::workflow::WorkflowTemplate>,

    /// Global agent config (skills, MCP, system prompt).
    #[serde(default)]
    pub agent_config: AgentConfig,

    /// Per-stage hooks.
    #[serde(default)]
    pub stage_hooks: HashMap<String, StageHooks>,

    /// Stage prompt template file overrides.
    #[serde(default)]
    pub prompt_templates: HashMap<String, String>,
}

/// Global settings shared across all routes.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GlobalConfig {
    /// Repository in `owner/name` format.
    pub repo: String,

    /// Local checkout path. Default: current directory.
    pub repo_dir: Option<String>,

    /// Maximum concurrent runs across all routes.
    #[serde(default = "default_max_concurrency")]
    pub max_concurrency: usize,

    /// Maximum cost per single issue (fail if exceeded).
    pub max_cost_per_issue: Option<f64>,

    /// Maximum hourly spend (pause all routes if exceeded).
    pub max_cost_per_hour: Option<f64>,

    /// Default agent.
    #[serde(default = "default_agent")]
    pub agent: String,

    /// Default model.
    pub model: Option<String>,

    /// Gate label — issues/PRs must have this label to be eligible.
    pub gate_label: Option<String>,

    /// Label applied when processing starts.
    #[serde(default = "default_in_progress_label")]
    pub in_progress_label: String,

    /// Label applied on successful completion.
    #[serde(default = "default_complete_label")]
    pub complete_label: String,

    /// Label applied on failure.
    #[serde(default = "default_failed_label")]
    pub failed_label: String,

    /// Branch naming pattern. {issue} and {slug} are replaced.
    #[serde(default = "default_branch_pattern")]
    pub branch_pattern: String,

    /// Stale lease timeout in seconds.
    #[serde(default = "default_stale_lease_timeout")]
    pub stale_lease_timeout: u64,

    /// Default workflow when no route label matches.
    pub default_workflow: Option<String>,
}

/// Security settings.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SecurityConfig {
    /// Authorization level: sandbox, local, contributor, trusted.
    #[serde(default = "default_auth_level")]
    pub authorization_level: String,

    /// Only process issues from these authors. Empty = authenticated user only.
    #[serde(default)]
    pub allowed_authors: Vec<String>,
}

/// Global validation commands.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ValidationConfig {
    /// Commands to run between stages.
    #[serde(default)]
    pub commands: Vec<String>,
}

/// A named route — maps type+label to a workflow.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Route {
    /// "issue" or "pr".
    #[serde(rename = "type")]
    pub route_type: String,

    /// GitHub label that triggers this route.
    pub label: String,

    /// Workflow template name to use.
    pub workflow: String,

    /// Maximum concurrent runs for this route.
    #[serde(default = "default_route_concurrency")]
    pub concurrency: usize,

    /// Poll interval in seconds for this route.
    #[serde(default = "default_poll_interval")]
    pub poll_interval: u64,

    /// Override model for this route.
    pub model: Option<String>,

    /// Override skills for this route.
    pub skills: Option<Vec<String>>,

    /// Override validation commands for this route.
    pub validation_commands: Option<Vec<String>>,

    /// Override MCP config for this route.
    pub mcp_config: Option<String>,
}

/// Agent-specific configuration.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct AgentConfig {
    /// Skill files to inject.
    #[serde(default)]
    pub skills: Vec<String>,

    /// MCP config file path.
    pub mcp_config: Option<String>,

    /// System prompt to append.
    pub append_system_prompt: Option<String>,
}

/// Pre/post hooks for a specific stage.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct StageHooks {
    #[serde(default)]
    pub pre: Vec<String>,
    #[serde(default)]
    pub post: Vec<String>,
}

// ── Defaults ────────────────────────────────────────────────────────

fn default_max_concurrency() -> usize {
    5
}

fn default_agent() -> String {
    "claude".to_string()
}

fn default_in_progress_label() -> String {
    "runner:in-progress".to_string()
}

fn default_complete_label() -> String {
    "runner:complete".to_string()
}

fn default_failed_label() -> String {
    "runner:failed".to_string()
}

fn default_branch_pattern() -> String {
    "automation/{issue}-{slug}".to_string()
}

fn default_stale_lease_timeout() -> u64 {
    3600
}

fn default_auth_level() -> String {
    "contributor".to_string()
}

fn default_route_concurrency() -> usize {
    1
}

fn default_poll_interval() -> u64 {
    300
}

// ── Implementation ──────────────────────────────────────────────────

impl RunnerConfig {
    /// Load config from a TOML file.
    pub fn from_file(path: &Path) -> Result<Self> {
        let content = std::fs::read_to_string(path)?;
        toml::from_str(&content).map_err(|e| Error::Policy(e.to_string()))
    }

    /// Get routes filtered by type ("issue" or "pr").
    pub fn routes_by_type(&self, route_type: &str) -> Vec<(&str, &Route)> {
        self.routes
            .iter()
            .filter(|(_, r)| r.route_type == route_type)
            .map(|(name, route)| (name.as_str(), route))
            .collect()
    }

    /// Find the route that matches an issue's labels.
    pub fn match_route(&self, issue: &IssueCandidate) -> Option<(&str, &Route)> {
        for (name, route) in &self.routes {
            if route.route_type == "issue" && issue.labels.iter().any(|l| l == &route.label) {
                return Some((name.as_str(), route));
            }
        }
        None
    }

    /// Get the effective model for a route (route override > global default).
    pub fn effective_model<'a>(&'a self, route: &'a Route) -> Option<&'a str> {
        route.model.as_deref().or(self.global.model.as_deref())
    }

    /// Get the effective validation commands for a route.
    pub fn effective_validation<'a>(&'a self, route: &'a Route) -> &'a [String] {
        route
            .validation_commands
            .as_deref()
            .unwrap_or(&self.validation.commands)
    }

    /// Generate a branch name for an issue.
    pub fn branch_for_issue(&self, issue: &IssueCandidate) -> String {
        let slug = slugify(&issue.title, 40);
        let branch = self
            .global
            .branch_pattern
            .replace("{issue}", &issue.number.to_string())
            .replace("{slug}", &slug);
        truncate_branch(&branch, 60)
    }

    /// Resolve a workflow template by name (custom overrides built-in).
    pub fn resolve_workflow(&self, name: &str) -> Option<crate::workflow::WorkflowTemplate> {
        // Check custom templates first.
        if let Some(template) = self.workflow_templates.iter().find(|t| t.name == name) {
            return Some(template.clone());
        }
        // Fall back to built-in.
        crate::workflow::builtin_templates()
            .into_iter()
            .find(|t| t.name == name)
    }
}

// ── Slug helpers (moved from policy.rs) ─────────────────────────────

fn slugify(title: &str, max_len: usize) -> String {
    let raw: String = title
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect();

    let collapsed = collapse_hyphens(&raw);
    let trimmed = collapsed.trim_matches('-');

    trimmed
        .chars()
        .take(max_len)
        .collect::<String>()
        .trim_end_matches('-')
        .to_string()
}

fn collapse_hyphens(s: &str) -> String {
    let mut result = String::with_capacity(s.len());
    let mut prev_hyphen = false;
    for c in s.chars() {
        if c == '-' {
            if !prev_hyphen {
                result.push(c);
            }
            prev_hyphen = true;
        } else {
            result.push(c);
            prev_hyphen = false;
        }
    }
    result
}

fn truncate_branch(branch: &str, max_len: usize) -> String {
    if branch.chars().count() <= max_len {
        return branch.to_string();
    }
    branch
        .chars()
        .take(max_len)
        .collect::<String>()
        .trim_end_matches('-')
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_example_config() {
        let content = r#"
[global]
repo = "owner/repo"
max_concurrency = 3
model = "claude-sonnet-4-6"
gate_label = "runner:ready"

[routes.bugfix]
type = "issue"
label = "bug"
workflow = "bug"
concurrency = 1
poll_interval = 60

[routes.features]
type = "issue"
label = "enhancement"
workflow = "feature"
concurrency = 2
poll_interval = 300
"#;
        let config: RunnerConfig = toml::from_str(content).unwrap();
        assert_eq!(config.global.repo, "owner/repo");
        assert_eq!(config.routes.len(), 2);
        assert_eq!(config.routes["bugfix"].workflow, "bug");
        assert_eq!(config.routes["features"].concurrency, 2);
    }

    #[test]
    fn match_route_by_label() {
        let config: RunnerConfig = toml::from_str(
            r#"
[global]
repo = "owner/repo"

[routes.bugfix]
type = "issue"
label = "bug"
workflow = "bug"
"#,
        )
        .unwrap();

        let issue = IssueCandidate {
            number: 1,
            repo: "owner/repo".into(),
            title: "fix something".into(),
            body: "details here".into(),
            labels: vec!["bug".into()],
            state: "open".into(),
            created_at: String::new(),
            updated_at: String::new(),
            is_assigned: false,
            html_url: String::new(),
        };

        let (name, route) = config.match_route(&issue).unwrap();
        assert_eq!(name, "bugfix");
        assert_eq!(route.workflow, "bug");
    }

    #[test]
    fn no_route_match_returns_none() {
        let config: RunnerConfig = toml::from_str(
            r#"
[global]
repo = "owner/repo"

[routes.bugfix]
type = "issue"
label = "bug"
workflow = "bug"
"#,
        )
        .unwrap();

        let issue = IssueCandidate {
            number: 1,
            repo: "owner/repo".into(),
            title: "add feature".into(),
            body: "details".into(),
            labels: vec!["enhancement".into()],
            state: "open".into(),
            created_at: String::new(),
            updated_at: String::new(),
            is_assigned: false,
            html_url: String::new(),
        };

        assert!(config.match_route(&issue).is_none());
    }

    #[test]
    fn effective_model_route_overrides_global() {
        let config: RunnerConfig = toml::from_str(
            r#"
[global]
repo = "owner/repo"
model = "sonnet"

[routes.bugfix]
type = "issue"
label = "bug"
workflow = "bug"
model = "opus"
"#,
        )
        .unwrap();

        assert_eq!(
            config.effective_model(&config.routes["bugfix"]),
            Some("opus")
        );
    }

    #[test]
    fn slug_generation() {
        assert_eq!(slugify("fix: handle the thing", 40), "fix-handle-the-thing");
        assert_eq!(
            slugify("fix(scope): do something", 40),
            "fix-scope-do-something"
        );
        assert!(!slugify("  leading  ", 40).starts_with('-'));
        assert!(!slugify("  trailing  ", 40).ends_with('-'));
    }
}
