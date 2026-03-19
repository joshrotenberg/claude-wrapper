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
    /// Profile name to apply to all generated tasks.
    pub profile: Option<String>,
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

/// Builder for [`PlanOptions`].
///
/// `prompt()` is the repeatable entry point — call it once per task.
/// All other setters apply to every generated task as overrides.
///
/// # Example
///
/// ```
/// use claudes::planner::PlanOptionsBuilder;
///
/// let opts = PlanOptionsBuilder::new()
///     .prompt("Fix the pagination bug")
///     .prompt("Add export endpoint")
///     .model("claude-opus-4-5")
///     .max_turns(20)
///     .build();
/// assert_eq!(opts.prompts.len(), 2);
/// ```
#[derive(Debug, Default)]
pub struct PlanOptionsBuilder {
    prompts: Vec<String>,
    model: Option<String>,
    fallback_model: Option<String>,
    max_turns: Option<u32>,
    timeout_secs: Option<u64>,
    max_budget_usd: Option<f64>,
    effort: Option<String>,
    permission_mode: Option<String>,
    allowed_tools: Option<Vec<String>>,
    disallowed_tools: Option<Vec<String>>,
    append_system_prompt: Option<String>,
    mcp_config: Option<String>,
    strict_mcp_config: Option<bool>,
    no_session_persistence: Option<bool>,
    isolation: Option<String>,
    isolation_base_dir: Option<String>,
    profile: Option<String>,
}

impl PlanOptionsBuilder {
    /// Create a new builder.
    pub fn new() -> Self {
        Self::default()
    }

    /// Add a prompt (one task per call).
    pub fn prompt(mut self, prompt: impl Into<String>) -> Self {
        self.prompts.push(prompt.into());
        self
    }

    /// Model override.
    pub fn model(mut self, model: impl Into<String>) -> Self {
        self.model = Some(model.into());
        self
    }

    /// Fallback model override.
    pub fn fallback_model(mut self, fallback_model: impl Into<String>) -> Self {
        self.fallback_model = Some(fallback_model.into());
        self
    }

    /// Max turns override.
    pub fn max_turns(mut self, max_turns: u32) -> Self {
        self.max_turns = Some(max_turns);
        self
    }

    /// Timeout override (in seconds).
    pub fn timeout_secs(mut self, timeout_secs: u64) -> Self {
        self.timeout_secs = Some(timeout_secs);
        self
    }

    /// Budget override.
    pub fn max_budget_usd(mut self, max_budget_usd: f64) -> Self {
        self.max_budget_usd = Some(max_budget_usd);
        self
    }

    /// Effort override (`"low"`, `"medium"`, or `"high"`).
    pub fn effort(mut self, effort: impl Into<String>) -> Self {
        self.effort = Some(effort.into());
        self
    }

    /// Permission mode override.
    pub fn permission_mode(mut self, permission_mode: impl Into<String>) -> Self {
        self.permission_mode = Some(permission_mode.into());
        self
    }

    /// Allowed tools override.
    pub fn allowed_tools(mut self, allowed_tools: Vec<String>) -> Self {
        self.allowed_tools = Some(allowed_tools);
        self
    }

    /// Disallowed tools override.
    pub fn disallowed_tools(mut self, disallowed_tools: Vec<String>) -> Self {
        self.disallowed_tools = Some(disallowed_tools);
        self
    }

    /// Append system prompt override.
    pub fn append_system_prompt(mut self, append_system_prompt: impl Into<String>) -> Self {
        self.append_system_prompt = Some(append_system_prompt.into());
        self
    }

    /// MCP config override.
    pub fn mcp_config(mut self, mcp_config: impl Into<String>) -> Self {
        self.mcp_config = Some(mcp_config.into());
        self
    }

    /// Strict MCP config override.
    pub fn strict_mcp_config(mut self, strict_mcp_config: bool) -> Self {
        self.strict_mcp_config = Some(strict_mcp_config);
        self
    }

    /// No session persistence override.
    pub fn no_session_persistence(mut self, no_session_persistence: bool) -> Self {
        self.no_session_persistence = Some(no_session_persistence);
        self
    }

    /// Isolation type override (`"worktree"`, `"clone"`, or `"none"`).
    pub fn isolation(mut self, isolation: impl Into<String>) -> Self {
        self.isolation = Some(isolation.into());
        self
    }

    /// Isolation base directory override.
    pub fn isolation_base_dir(mut self, isolation_base_dir: impl Into<String>) -> Self {
        self.isolation_base_dir = Some(isolation_base_dir.into());
        self
    }

    /// Profile name to apply to all generated tasks.
    pub fn profile(mut self, profile: impl Into<String>) -> Self {
        self.profile = Some(profile.into());
        self
    }

    /// Build the [`PlanOptions`].
    pub fn build(self) -> PlanOptions {
        PlanOptions {
            prompts: self.prompts,
            model: self.model,
            fallback_model: self.fallback_model,
            max_turns: self.max_turns,
            timeout_secs: self.timeout_secs,
            max_budget_usd: self.max_budget_usd,
            effort: self.effort,
            permission_mode: self.permission_mode,
            allowed_tools: self.allowed_tools,
            disallowed_tools: self.disallowed_tools,
            append_system_prompt: self.append_system_prompt,
            mcp_config: self.mcp_config,
            strict_mcp_config: self.strict_mcp_config,
            no_session_persistence: self.no_session_persistence,
            isolation: self.isolation,
            isolation_base_dir: self.isolation_base_dir,
            profile: self.profile,
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
                profile: options.profile.clone(),
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
                prompt_file: None,
                append_system_prompt_file: None,
                effort: options.effort.clone(),
                no_session_persistence: options.no_session_persistence,
                mcp_config: options.mcp_config.clone(),
                strict_mcp_config: options.strict_mcp_config,
                add_dirs: None,
                isolation,
                branch: Some(branch),
                env: None,
                pre_hooks: None,
                post_hooks: None,
                finally_hooks: None,
                depends_on: None,
                skills: None,
                settings: None,
                setting_sources: None,
            }
        })
        .collect();

    Manifest::new(tasks)
}

/// Generate a task name from a prompt.
///
/// Filters out English noise words and file path tokens, caps the slug at 25
/// chars, and appends a 4-char hash for uniqueness: `fix-pagination-bug-a3b2`
fn generate_task_name(prompt: &str) -> String {
    const NOISE_WORDS: &[&str] = &[
        "in", "the", "a", "for", "to", "of", "and", "all", "from", "with",
    ];
    const FILE_EXTENSIONS: &[&str] = &[
        ".rs", ".ts", ".py", ".js", ".go", ".java", ".cpp", ".c", ".h", ".tsx", ".jsx", ".toml",
        ".json", ".yaml", ".yml", ".md",
    ];

    // Hash the full original prompt for uniqueness.
    let mut hasher = DefaultHasher::new();
    prompt.hash(&mut hasher);
    let hash = format!("{:04x}", hasher.finish() & 0xFFFF);

    let lower = prompt.to_lowercase();
    let slug_words: Vec<String> = lower
        .split_whitespace()
        .filter(|word| {
            // Skip noise words.
            if NOISE_WORDS.contains(word) {
                return false;
            }
            // Skip file paths: tokens containing '/' or ending in a known extension.
            let is_path =
                word.contains('/') || FILE_EXTENSIONS.iter().any(|ext| word.ends_with(ext));
            !is_path
        })
        .map(|word| {
            word.chars()
                .filter(|c| c.is_alphanumeric() || *c == '-')
                .collect::<String>()
        })
        .filter(|w| !w.is_empty())
        .collect();

    // Build slug capped at 25 chars, adding only complete words.
    let mut slug = String::new();
    for word in &slug_words {
        if slug.is_empty() {
            // First word: take up to 25 chars.
            slug.extend(word.chars().take(25));
        } else if slug.len() + 1 + word.len() <= 25 {
            slug.push('-');
            slug.push_str(word);
        } else {
            break;
        }
    }

    let slug = slug.trim_matches('-').to_string();
    let slug = if slug.is_empty() {
        "task".to_string()
    } else {
        slug
    };

    format!("{slug}-{hash}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generate_name_from_prompt() {
        // "the" and "in" are noise words; "list.rs" is a file path — all stripped.
        let name = generate_task_name("Fix the pagination bug in list.rs");
        assert!(name.starts_with("fix-pagination-bug"));
        assert!(name.len() > 10);
        // Should end with a 4-char hex hash.
        let parts: Vec<&str> = name.rsplitn(2, '-').collect();
        assert_eq!(parts[0].len(), 4);
    }

    #[test]
    fn generate_name_strips_file_paths() {
        // Path token "crates/claudes/src/planner.rs" contains '/' — stripped.
        let name = generate_task_name("Fix the bug in crates/claudes/src/planner.rs");
        assert!(name.starts_with("fix-bug-"));
        let parts: Vec<&str> = name.rsplitn(2, '-').collect();
        assert_eq!(parts[0].len(), 4);
    }

    #[test]
    fn generate_name_long_prompt_slug_capped() {
        // A long prompt should produce a slug no longer than 25 chars.
        let long =
            "Implement comprehensive authentication system for enterprise production deployment";
        let name = generate_task_name(long);
        let parts: Vec<&str> = name.rsplitn(2, '-').collect();
        assert_eq!(parts[0].len(), 4, "hash should be 4 chars");
        assert!(parts[1].len() <= 25, "slug should be at most 25 chars");
    }

    #[test]
    fn generate_name_short_prompt() {
        let name = generate_task_name("Fix bug");
        assert!(name.starts_with("fix-bug-"));
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

    #[test]
    fn builder_single_prompt() {
        let opts = PlanOptionsBuilder::new().prompt("Fix the bug").build();
        assert_eq!(opts.prompts, vec!["Fix the bug"]);
    }

    #[test]
    fn builder_multiple_prompts() {
        let opts = PlanOptionsBuilder::new()
            .prompt("Fix A")
            .prompt("Fix B")
            .prompt("Fix C")
            .build();
        assert_eq!(opts.prompts.len(), 3);
        assert_eq!(opts.prompts[1], "Fix B");
    }

    #[test]
    fn builder_defaults_are_none() {
        let opts = PlanOptionsBuilder::new().prompt("task").build();
        assert!(opts.model.is_none());
        assert!(opts.fallback_model.is_none());
        assert!(opts.max_turns.is_none());
        assert!(opts.timeout_secs.is_none());
        assert!(opts.max_budget_usd.is_none());
        assert!(opts.effort.is_none());
        assert!(opts.permission_mode.is_none());
        assert!(opts.allowed_tools.is_none());
        assert!(opts.disallowed_tools.is_none());
        assert!(opts.append_system_prompt.is_none());
        assert!(opts.mcp_config.is_none());
        assert!(opts.strict_mcp_config.is_none());
        assert!(opts.no_session_persistence.is_none());
        assert!(opts.isolation.is_none());
        assert!(opts.isolation_base_dir.is_none());
        assert!(opts.profile.is_none());
    }

    #[test]
    fn builder_overrides() {
        let opts = PlanOptionsBuilder::new()
            .prompt("task")
            .model("opus")
            .fallback_model("sonnet")
            .max_turns(10)
            .timeout_secs(120)
            .max_budget_usd(5.0)
            .effort("high")
            .permission_mode("bypassPermissions")
            .allowed_tools(vec!["Edit".into()])
            .disallowed_tools(vec!["Bash".into()])
            .append_system_prompt("Be concise.")
            .mcp_config("/path/to/mcp.json")
            .strict_mcp_config(true)
            .no_session_persistence(true)
            .isolation("clone")
            .isolation_base_dir(".clones")
            .build();

        assert_eq!(opts.model.as_deref(), Some("opus"));
        assert_eq!(opts.fallback_model.as_deref(), Some("sonnet"));
        assert_eq!(opts.max_turns, Some(10));
        assert_eq!(opts.timeout_secs, Some(120));
        assert_eq!(opts.max_budget_usd, Some(5.0));
        assert_eq!(opts.effort.as_deref(), Some("high"));
        assert_eq!(opts.permission_mode.as_deref(), Some("bypassPermissions"));
        assert_eq!(
            opts.allowed_tools.as_deref(),
            Some(&["Edit".to_string()][..])
        );
        assert_eq!(
            opts.disallowed_tools.as_deref(),
            Some(&["Bash".to_string()][..])
        );
        assert_eq!(opts.append_system_prompt.as_deref(), Some("Be concise."));
        assert_eq!(opts.mcp_config.as_deref(), Some("/path/to/mcp.json"));
        assert_eq!(opts.strict_mcp_config, Some(true));
        assert_eq!(opts.no_session_persistence, Some(true));
        assert_eq!(opts.isolation.as_deref(), Some("clone"));
        assert_eq!(opts.isolation_base_dir.as_deref(), Some(".clones"));
    }

    #[test]
    fn builder_produces_valid_manifest() {
        let opts = PlanOptionsBuilder::new()
            .prompt("Fix the pagination bug")
            .prompt("Add export endpoint")
            .model("opus")
            .max_turns(20)
            .build();
        let manifest = plan(&opts);
        assert_eq!(manifest.tasks.len(), 2);
        assert_eq!(manifest.tasks[0].prompt, "Fix the pagination bug");
        assert_eq!(manifest.tasks[1].prompt, "Add export endpoint");
        assert_eq!(manifest.tasks[0].model.as_deref(), Some("opus"));
        assert_eq!(manifest.tasks[0].max_turns, Some(20));
    }
}
