//! Repository policy — per-repo configuration controlling automation behavior.

use std::path::Path;

use serde::{Deserialize, Serialize};

/// Per-repository configuration that controls automation behavior.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RepoPolicy {
    /// Repository in `owner/name` format.
    pub repo: String,

    /// Issue labels that make an issue eligible for automation.
    #[serde(default)]
    pub eligible_labels: Vec<String>,

    /// Labels that exclude an issue from automation.
    #[serde(default)]
    pub exclude_labels: Vec<String>,

    /// Workflow template to use by issue type.
    #[serde(default)]
    pub workflows: std::collections::HashMap<String, String>,

    /// Branch naming pattern. `{issue}` is replaced with issue number.
    #[serde(default = "default_branch_pattern")]
    pub branch_pattern: String,

    /// Maximum concurrent runs for this repo.
    #[serde(default = "default_concurrency")]
    pub max_concurrency: usize,

    /// Whether to auto-merge approved PRs.
    #[serde(default)]
    pub auto_merge: bool,

    /// Agent to use for execution (e.g., "claude", "codex").
    #[serde(default = "default_agent")]
    pub agent: String,

    /// Model override for the agent.
    pub model: Option<String>,

    /// Post-stage validation commands.
    #[serde(default)]
    pub validation_commands: Vec<String>,
}

fn default_branch_pattern() -> String {
    "automation/{issue}-{slug}".to_string()
}

fn default_concurrency() -> usize {
    3
}

fn default_agent() -> String {
    "claude".to_string()
}

impl RepoPolicy {
    /// Load policy from a TOML file.
    pub fn from_file(path: &Path) -> crate::error::Result<Self> {
        let content = std::fs::read_to_string(path)?;
        toml::from_str(&content).map_err(|e| crate::error::Error::Policy(e.to_string()))
    }
}
