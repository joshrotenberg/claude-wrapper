//! Repository policy — per-repo configuration controlling automation behavior.

use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::workflow::WorkflowTemplate;

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

    /// Maximum concurrent runs across all workflows (global cap).
    #[serde(default = "default_concurrency")]
    pub max_concurrency: usize,

    /// Per-workflow concurrency limits. Key is the workflow name (e.g., "bug",
    /// "feature"). A value of `0` means unlimited (up to the global cap).
    /// Workflows not listed here default to unlimited (up to the global cap).
    #[serde(default)]
    pub concurrency: std::collections::HashMap<String, usize>,

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

    /// Per-stage prompt overrides. Keys are stage names (e.g., "plan", "implement").
    /// If set, the value replaces the default generated prompt for that stage.
    #[serde(default)]
    pub stage_prompts: std::collections::HashMap<String, String>,

    /// Default workflow to use when no label matches `workflows`. Falls back to
    /// `"feature"` if not set.
    #[serde(default)]
    pub default_workflow: Option<String>,

    /// Custom workflow templates. These are merged with the built-in templates;
    /// custom templates shadow built-ins with the same name.
    #[serde(default)]
    pub workflow_templates: Vec<WorkflowTemplate>,
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

    /// Generate a branch name for an issue based on the pattern.
    pub fn branch_for_issue(&self, issue: &crate::github::IssueCandidate) -> String {
        let slug = slugify(&issue.title, 40);
        let branch = self
            .branch_pattern
            .replace("{issue}", &issue.number.to_string())
            .replace("{slug}", &slug);
        truncate_branch(&branch, 60)
    }
}

/// Convert a title into a git-safe slug.
///
/// - ASCII alphanumeric characters are lowercased and kept as-is.
/// - Non-ASCII unicode letters/digits are transliterated to their ASCII base
///   character where possible; otherwise dropped.
/// - All other characters become a single `-`.
/// - Consecutive hyphens are collapsed.
/// - Leading and trailing hyphens are stripped.
/// - The result is limited to `max_len` characters before stripping.
fn slugify(title: &str, max_len: usize) -> String {
    let raw: String = title
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_ascii_lowercase()
            } else if c.is_alphabetic() || c.is_numeric() {
                // Non-ASCII unicode: attempt to fold to ASCII equivalent.
                // We do a simple decomposition: keep the first ASCII char, drop the rest.
                let lower = c.to_lowercase().next().unwrap_or(c);
                let escaped = lower.escape_unicode().to_string();
                // If it encodes to a single ASCII char, use it; otherwise use '-'.
                if escaped.len() == 1
                    && escaped
                        .chars()
                        .next()
                        .is_some_and(|ch| ch.is_ascii_alphanumeric())
                {
                    escaped.chars().next().unwrap()
                } else {
                    '-'
                }
            } else {
                '-'
            }
        })
        .collect();

    // Collapse consecutive hyphens and trim.
    let collapsed = collapse_hyphens(&raw);
    let trimmed = collapsed.trim_matches('-');

    // Limit length then re-trim in case the cut point lands on a hyphen.
    trimmed
        .chars()
        .take(max_len)
        .collect::<String>()
        .trim_end_matches('-')
        .to_string()
}

/// Collapse runs of two or more hyphens into a single hyphen.
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

/// Truncate a branch name to `max_len` characters, trimming any trailing
/// hyphens that result from the cut.
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

    fn make_issue(number: u64, title: &str) -> crate::github::IssueCandidate {
        crate::github::IssueCandidate {
            number,
            repo: "owner/repo".to_string(),
            title: title.to_string(),
            body: String::new(),
            labels: vec![],
            state: "open".to_string(),
            created_at: String::new(),
            updated_at: String::new(),
            is_assigned: false,
            html_url: String::new(),
        }
    }

    #[test]
    fn test_basic_slug() {
        let policy = RepoPolicy {
            repo: "owner/repo".to_string(),
            eligible_labels: vec![],
            exclude_labels: vec![],
            workflows: Default::default(),
            branch_pattern: default_branch_pattern(),
            max_concurrency: default_concurrency(),
            concurrency: Default::default(),
            auto_merge: false,
            agent: default_agent(),
            model: None,
            validation_commands: vec![],
            stage_prompts: Default::default(),
            default_workflow: None,
            workflow_templates: vec![],
        };
        let issue = make_issue(42, "fix: handle the thing");
        let branch = policy.branch_for_issue(&issue);
        assert_eq!(branch, "automation/42-fix-handle-the-thing");
    }

    #[test]
    fn test_consecutive_hyphens_collapsed() {
        assert_eq!(
            slugify("fix(scope): do something", 60),
            "fix-scope-do-something"
        );
    }

    #[test]
    fn test_leading_trailing_hyphens_stripped() {
        assert_eq!(
            slugify("  leading and trailing  ", 60),
            "leading-and-trailing"
        );
    }

    #[test]
    fn test_length_limit() {
        let long = "a".repeat(100);
        let slug = slugify(&long, 40);
        assert!(slug.len() <= 40);
    }

    #[test]
    fn test_length_limit_no_trailing_hyphen() {
        // Ensure truncation doesn't leave a trailing hyphen.
        let s = "abc-def-ghi-jkl-mno-pqr-stu-vwx-yz";
        let slug = slugify(s, 10);
        assert!(!slug.ends_with('-'));
        assert!(slug.len() <= 10);
    }

    #[test]
    fn test_branch_total_length_limit() {
        let policy = RepoPolicy {
            repo: "owner/repo".to_string(),
            eligible_labels: vec![],
            exclude_labels: vec![],
            workflows: Default::default(),
            branch_pattern: default_branch_pattern(),
            max_concurrency: default_concurrency(),
            concurrency: Default::default(),
            auto_merge: false,
            agent: default_agent(),
            model: None,
            validation_commands: vec![],
            stage_prompts: Default::default(),
            default_workflow: None,
            workflow_templates: vec![],
        };
        let issue = make_issue(471, &"a".repeat(200));
        let branch = policy.branch_for_issue(&issue);
        assert!(branch.len() <= 60, "branch too long: {branch}");
    }

    #[test]
    fn test_unicode_handled() {
        // Non-ASCII chars become hyphens, consecutive hyphens collapse.
        let slug = slugify("café résumé", 60);
        // 'c', 'a', 'f' are ASCII; 'é' → '-'; ' ' → '-'; etc.
        assert!(!slug.contains("--"));
        assert!(!slug.starts_with('-'));
        assert!(!slug.ends_with('-'));
    }

    #[test]
    fn test_collapse_hyphens() {
        assert_eq!(collapse_hyphens("a--b---c"), "a-b-c");
        assert_eq!(collapse_hyphens("---"), "-");
    }
}
