//! Manifest schema — the core abstraction.
//!
//! A manifest is a fully resolved JSON document describing exactly what to execute.
//! Every field is explicit. No inheritance, no defaults, no references to profiles.
//! What you see is what executes.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Load global defaults from a given config home directory.
///
/// Looks for `claudes/defaults.toml` then `claudes/defaults.json` under `config_home`.
/// Returns `None` if neither file exists or cannot be parsed.
fn load_global_defaults_from(config_home: &Path) -> Option<Shared> {
    let base = config_home.join("claudes");
    for name in &["defaults.toml", "defaults.json"] {
        let path = base.join(name);
        if path.exists() {
            let contents = std::fs::read_to_string(&path).ok()?;
            return match path.extension().and_then(|e| e.to_str()) {
                Some("toml") => toml::from_str::<Shared>(&contents).ok(),
                Some("json") => serde_json::from_str::<Shared>(&contents).ok(),
                _ => None,
            };
        }
    }
    None
}

/// Load global defaults from `~/.config/claudes/defaults.toml` or `defaults.json`.
///
/// Returns the parsed [`Shared`] block, or `None` if no file exists or the file
/// cannot be parsed. A missing file is not an error.
pub fn load_global_defaults() -> Option<Shared> {
    let home = std::env::var("HOME").ok()?;
    load_global_defaults_from(&PathBuf::from(home).join(".config"))
}

fn default_version() -> u32 {
    1
}

/// The manifest — a fully resolved, self-contained execution plan.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Manifest {
    /// Schema version (currently 1).
    #[serde(default = "default_version")]
    pub version: u32,

    /// When this manifest was created.
    #[serde(default = "Utc::now")]
    pub created_at: DateTime<Utc>,

    /// Manifest-level defaults applied to all tasks.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub shared: Option<Shared>,

    /// Named Shared presets that tasks can reference via their `profile` field.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub profiles: Option<HashMap<String, Shared>>,

    /// One or more tasks to execute.
    pub tasks: Vec<Task>,

    /// Dependency chains — sugar for declaring sequential/fan-out dependencies.
    ///
    /// Each chain is an array of steps. A step is either a task name (string) or
    /// an array of task names (parallel group). Adjacent steps become dependencies:
    ///
    /// ```json
    /// "chains": [
    ///   ["a", "b", "c"],                    // linear: a -> b -> c
    ///   ["a", ["b1", "b2"], "c"]            // fan-out: a -> (b1,b2) -> c
    /// ]
    /// ```
    ///
    /// Desugars into `depends_on` on the referenced tasks. Multiple chains that
    /// reference the same task merge their dependencies (union).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub chains: Option<Vec<Vec<ChainStep>>>,
}

/// A single step in a dependency chain — either one task or a parallel group.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ChainStep {
    /// A single task name.
    Single(String),
    /// Multiple tasks that run in parallel at this step.
    Group(Vec<String>),
}

impl Manifest {
    /// Create a new manifest with the given tasks.
    pub fn new(tasks: Vec<Task>) -> Self {
        Self {
            version: 1,
            created_at: Utc::now(),
            shared: None,
            profiles: None,
            tasks,
            chains: None,
        }
    }

    /// Desugar `chains` into `depends_on` on the referenced tasks.
    ///
    /// Each chain is processed step by step. Adjacent steps become dependencies:
    /// the tasks in each step depend on all tasks in the previous step.
    /// Dependencies are merged (unioned) across multiple chains.
    ///
    /// Call this before `validate()` and `resolve()`.
    pub fn desugar_chains(&mut self) {
        let Some(chains) = self.chains.take() else {
            return;
        };

        // Build a map of task name -> accumulated dependencies.
        let mut deps_map: HashMap<String, std::collections::HashSet<String>> = HashMap::new();

        for chain in &chains {
            let mut prev_names: Vec<String> = Vec::new();

            for step in chain {
                let current_names: Vec<String> = match step {
                    ChainStep::Single(name) => vec![name.clone()],
                    ChainStep::Group(names) => names.clone(),
                };

                // Each task in the current step depends on all tasks in the previous step.
                if !prev_names.is_empty() {
                    for name in &current_names {
                        deps_map
                            .entry(name.clone())
                            .or_default()
                            .extend(prev_names.iter().cloned());
                    }
                }

                prev_names = current_names;
            }
        }

        // Apply accumulated dependencies to tasks.
        for task in &mut self.tasks {
            if let Some(chain_deps) = deps_map.remove(&task.name) {
                let existing = task.depends_on.get_or_insert_with(Vec::new);
                for dep in chain_deps {
                    if !existing.contains(&dep) {
                        existing.push(dep);
                    }
                }
            }
        }
    }

    /// Return a new `Manifest` where each task has been merged with profile and shared defaults.
    ///
    /// Merge order: task fields > profile fields > shared fields.
    /// `pre_hooks` and `post_hooks` are concatenated: shared hooks, then profile hooks, then task hooks.
    ///
    /// # Panics
    ///
    /// Panics if a task references a profile that does not exist in `self.profiles`.
    /// Always call [`Manifest::validate`] before `resolve` to catch this early.
    pub fn resolve(&self) -> Manifest {
        fn merge_hooks(
            s: Option<&Vec<String>>,
            p: Option<&Vec<String>>,
            t: Option<&Vec<String>>,
        ) -> Option<Vec<String>> {
            let mut merged: Vec<String> = Vec::new();
            if let Some(hooks) = s {
                merged.extend(hooks.iter().cloned());
            }
            if let Some(hooks) = p {
                merged.extend(hooks.iter().cloned());
            }
            if let Some(hooks) = t {
                merged.extend(hooks.iter().cloned());
            }
            if merged.is_empty() {
                None
            } else {
                Some(merged)
            }
        }

        let tasks = self
            .tasks
            .iter()
            .map(|task| {
                let profile: Option<&Shared> = task.profile.as_ref().map(|name| {
                    self.profiles
                        .as_ref()
                        .and_then(|m| m.get(name.as_str()))
                        .unwrap_or_else(|| {
                            panic!(
                                "task '{}' references unknown profile '{name}'; \
                                 call validate() before resolve()",
                                task.name
                            )
                        })
                });
                let shared = self.shared.as_ref();

                Task {
                    name: task.name.clone(),
                    prompt: task.prompt.clone(),
                    prompt_file: task.prompt_file.clone(),
                    profile: task.profile.clone(),
                    model: task
                        .model
                        .clone()
                        .or_else(|| profile.and_then(|p| p.model.clone()))
                        .or_else(|| shared.and_then(|s| s.model.clone())),
                    fallback_model: task.fallback_model.clone(),
                    max_turns: task
                        .max_turns
                        .or_else(|| profile.and_then(|p| p.max_turns))
                        .or_else(|| shared.and_then(|s| s.max_turns)),
                    timeout_secs: task
                        .timeout_secs
                        .or_else(|| profile.and_then(|p| p.timeout_secs))
                        .or_else(|| shared.and_then(|s| s.timeout_secs)),
                    max_budget_usd: task
                        .max_budget_usd
                        .or_else(|| profile.and_then(|p| p.max_budget_usd))
                        .or_else(|| shared.and_then(|s| s.max_budget_usd)),
                    permission_mode: task
                        .permission_mode
                        .clone()
                        .or_else(|| profile.and_then(|p| p.permission_mode.clone()))
                        .or_else(|| shared.and_then(|s| s.permission_mode.clone())),
                    allowed_tools: task
                        .allowed_tools
                        .clone()
                        .or_else(|| profile.and_then(|p| p.allowed_tools.clone()))
                        .or_else(|| shared.and_then(|s| s.allowed_tools.clone())),
                    disallowed_tools: task
                        .disallowed_tools
                        .clone()
                        .or_else(|| profile.and_then(|p| p.disallowed_tools.clone()))
                        .or_else(|| shared.and_then(|s| s.disallowed_tools.clone())),
                    system_prompt: task
                        .system_prompt
                        .clone()
                        .or_else(|| profile.and_then(|p| p.system_prompt.clone()))
                        .or_else(|| shared.and_then(|s| s.system_prompt.clone())),
                    append_system_prompt: task
                        .append_system_prompt
                        .clone()
                        .or_else(|| profile.and_then(|p| p.append_system_prompt.clone()))
                        .or_else(|| shared.and_then(|s| s.append_system_prompt.clone())),
                    append_system_prompt_file: task
                        .append_system_prompt_file
                        .clone()
                        .or_else(|| shared.and_then(|s| s.append_system_prompt_file.clone())),
                    effort: task
                        .effort
                        .clone()
                        .or_else(|| profile.and_then(|p| p.effort.clone()))
                        .or_else(|| shared.and_then(|s| s.effort.clone())),
                    no_session_persistence: task
                        .no_session_persistence
                        .or_else(|| profile.and_then(|p| p.no_session_persistence))
                        .or_else(|| shared.and_then(|s| s.no_session_persistence)),
                    mcp_config: task
                        .mcp_config
                        .clone()
                        .or_else(|| profile.and_then(|p| p.mcp_config.clone()))
                        .or_else(|| shared.and_then(|s| s.mcp_config.clone())),
                    strict_mcp_config: task
                        .strict_mcp_config
                        .or_else(|| profile.and_then(|p| p.strict_mcp_config))
                        .or_else(|| shared.and_then(|s| s.strict_mcp_config)),
                    add_dirs: task
                        .add_dirs
                        .clone()
                        .or_else(|| profile.and_then(|p| p.add_dirs.clone()))
                        .or_else(|| shared.and_then(|s| s.add_dirs.clone())),
                    isolation: task
                        .isolation
                        .clone()
                        .or_else(|| profile.and_then(|p| p.isolation.clone()))
                        .or_else(|| shared.and_then(|s| s.isolation.clone())),
                    branch: task
                        .branch
                        .clone()
                        .or_else(|| profile.and_then(|p| p.branch.clone()))
                        .or_else(|| shared.and_then(|s| s.branch.clone())),
                    env: task
                        .env
                        .clone()
                        .or_else(|| profile.and_then(|p| p.env.clone()))
                        .or_else(|| shared.and_then(|s| s.env.clone())),
                    pre_hooks: merge_hooks(
                        shared.and_then(|s| s.pre_hooks.as_ref()),
                        profile.and_then(|p| p.pre_hooks.as_ref()),
                        task.pre_hooks.as_ref(),
                    ),
                    post_hooks: merge_hooks(
                        shared.and_then(|s| s.post_hooks.as_ref()),
                        profile.and_then(|p| p.post_hooks.as_ref()),
                        task.post_hooks.as_ref(),
                    ),
                    finally_hooks: merge_hooks(
                        shared.and_then(|s| s.finally_hooks.as_ref()),
                        profile.and_then(|p| p.finally_hooks.as_ref()),
                        task.finally_hooks.as_ref(),
                    ),
                    depends_on: task.depends_on.clone(),
                    condition: task.condition.clone(),
                    skills: merge_hooks(
                        shared.and_then(|s| s.skills.as_ref()),
                        profile.and_then(|p| p.skills.as_ref()),
                        task.skills.as_ref(),
                    ),
                    settings: task
                        .settings
                        .clone()
                        .or_else(|| profile.and_then(|p| p.settings.clone()))
                        .or_else(|| shared.and_then(|s| s.settings.clone())),
                    setting_sources: task
                        .setting_sources
                        .clone()
                        .or_else(|| profile.and_then(|p| p.setting_sources.clone()))
                        .or_else(|| shared.and_then(|s| s.setting_sources.clone())),
                }
            })
            .collect();

        Manifest {
            version: self.version,
            created_at: self.created_at,
            shared: self.shared.clone(),
            profiles: self.profiles.clone(),
            tasks,
            chains: self.chains.clone(),
        }
    }

    /// Resolve file-based fields by reading their contents from disk.
    ///
    /// For each task, if `prompt_file` is set (and `prompt` is empty), reads the file relative
    /// to `base_dir` and sets `prompt`. If `append_system_prompt_file` is set, reads the file and
    /// sets `append_system_prompt`. The same applies to the `shared` block.
    ///
    /// Errors if both inline and `_file` variants are set for the same field, or if a referenced
    /// file does not exist.
    pub fn resolve_files(&mut self, base_dir: &Path) -> Result<(), crate::Error> {
        // Resolve shared.append_system_prompt_file.
        if let Some(shared) = self.shared.as_mut() {
            if shared.append_system_prompt_file.is_some() && shared.append_system_prompt.is_some() {
                return Err(crate::Error::InvalidManifest(
                    "shared: cannot set both append_system_prompt and append_system_prompt_file"
                        .into(),
                ));
            }
            if let Some(file_path) = shared.append_system_prompt_file.take() {
                let path = base_dir.join(&file_path);
                if !path.exists() {
                    return Err(crate::Error::InvalidManifest(format!(
                        "shared: append_system_prompt_file '{}' not found",
                        path.display()
                    )));
                }
                shared.append_system_prompt = Some(std::fs::read_to_string(&path)?);
            }
        }

        // Resolve per-task file fields.
        for task in &mut self.tasks {
            if task.prompt_file.is_some() && !task.prompt.is_empty() {
                return Err(crate::Error::InvalidManifest(format!(
                    "task '{}': cannot set both prompt and prompt_file",
                    task.name
                )));
            }
            if let Some(file_path) = task.prompt_file.take() {
                let path = base_dir.join(&file_path);
                if !path.exists() {
                    return Err(crate::Error::InvalidManifest(format!(
                        "task '{}': prompt_file '{}' not found",
                        task.name,
                        path.display()
                    )));
                }
                task.prompt = std::fs::read_to_string(&path)?;
            }

            if task.append_system_prompt_file.is_some() && task.append_system_prompt.is_some() {
                return Err(crate::Error::InvalidManifest(format!(
                    "task '{}': cannot set both append_system_prompt and append_system_prompt_file",
                    task.name
                )));
            }
            if let Some(file_path) = task.append_system_prompt_file.take() {
                let path = base_dir.join(&file_path);
                if !path.exists() {
                    return Err(crate::Error::InvalidManifest(format!(
                        "task '{}': append_system_prompt_file '{}' not found",
                        task.name,
                        path.display()
                    )));
                }
                task.append_system_prompt = Some(std::fs::read_to_string(&path)?);
            }
        }

        self.resolve_skills(base_dir)?;

        Ok(())
    }

    /// Load skill file contents and append them to `append_system_prompt` for each task.
    ///
    /// Skill paths are resolved relative to `base_dir`. Skills from the shared block and
    /// the task's referenced profile are prepended to task-level skills (same order as hooks).
    /// The loaded content is appended to any existing `append_system_prompt`.
    ///
    /// This is called automatically by [`Manifest::resolve_files`].
    pub fn resolve_skills(&mut self, base_dir: &Path) -> Result<(), crate::Error> {
        let shared_skills = self
            .shared
            .as_ref()
            .and_then(|s| s.skills.as_ref())
            .cloned()
            .unwrap_or_default();
        let profiles = self.profiles.clone();

        for task in &mut self.tasks {
            let profile_skills: Vec<String> = task
                .profile
                .as_ref()
                .and_then(|name| {
                    profiles
                        .as_ref()
                        .and_then(|m| m.get(name.as_str()))
                        .and_then(|p| p.skills.as_ref())
                })
                .cloned()
                .unwrap_or_default();
            let task_skills = task.skills.as_ref().cloned().unwrap_or_default();

            let all_skills: Vec<&String> = shared_skills
                .iter()
                .chain(profile_skills.iter())
                .chain(task_skills.iter())
                .collect();

            if all_skills.is_empty() {
                continue;
            }

            let mut skill_content = String::new();
            for skill_path in &all_skills {
                let path = base_dir.join(skill_path.as_str());
                if !path.exists() {
                    return Err(crate::Error::InvalidManifest(format!(
                        "task '{}': skill file '{}' not found",
                        task.name,
                        path.display()
                    )));
                }
                if !skill_content.is_empty() {
                    skill_content.push('\n');
                }
                skill_content.push_str(&std::fs::read_to_string(&path)?);
            }

            let existing = task.append_system_prompt.take().unwrap_or_default();
            task.append_system_prompt = Some(if existing.is_empty() {
                skill_content
            } else {
                format!("{existing}\n{skill_content}")
            });
        }

        Ok(())
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

    /// Merge `global` defaults into this manifest's `shared` block at the lowest priority.
    ///
    /// Fields already set in `self.shared` are not overwritten. Pre/post hooks from
    /// `global` are **prepended** to any existing shared hooks so the final resolution
    /// order is: task > manifest shared > global defaults.
    pub fn apply_global_defaults(&mut self, global: &Shared) {
        let shared = self.shared.get_or_insert_with(Shared::default);

        if shared.model.is_none() {
            shared.model = global.model.clone();
        }
        if shared.max_turns.is_none() {
            shared.max_turns = global.max_turns;
        }
        if shared.timeout_secs.is_none() {
            shared.timeout_secs = global.timeout_secs;
        }
        if shared.max_budget_usd.is_none() {
            shared.max_budget_usd = global.max_budget_usd;
        }
        if shared.permission_mode.is_none() {
            shared.permission_mode = global.permission_mode.clone();
        }
        if shared.allowed_tools.is_none() {
            shared.allowed_tools = global.allowed_tools.clone();
        }
        if shared.disallowed_tools.is_none() {
            shared.disallowed_tools = global.disallowed_tools.clone();
        }
        if shared.system_prompt.is_none() {
            shared.system_prompt = global.system_prompt.clone();
        }
        if shared.append_system_prompt.is_none() {
            shared.append_system_prompt = global.append_system_prompt.clone();
        }
        if shared.effort.is_none() {
            shared.effort = global.effort.clone();
        }
        if shared.no_session_persistence.is_none() {
            shared.no_session_persistence = global.no_session_persistence;
        }
        if shared.mcp_config.is_none() {
            shared.mcp_config = global.mcp_config.clone();
        }
        if shared.strict_mcp_config.is_none() {
            shared.strict_mcp_config = global.strict_mcp_config;
        }
        if shared.add_dirs.is_none() {
            shared.add_dirs = global.add_dirs.clone();
        }
        if shared.isolation.is_none() {
            shared.isolation = global.isolation.clone();
        }
        if shared.branch.is_none() {
            shared.branch = global.branch.clone();
        }
        if shared.env.is_none() {
            shared.env = global.env.clone();
        }

        // Hooks: global hooks are prepended to shared hooks.
        let pre_hooks = match (&global.pre_hooks, &shared.pre_hooks) {
            (Some(g), Some(s)) => {
                let mut merged = g.clone();
                merged.extend(s.iter().cloned());
                Some(merged)
            }
            (Some(g), None) => Some(g.clone()),
            (None, _) => shared.pre_hooks.clone(),
        };
        shared.pre_hooks = pre_hooks;

        let post_hooks = match (&global.post_hooks, &shared.post_hooks) {
            (Some(g), Some(s)) => {
                let mut merged = g.clone();
                merged.extend(s.iter().cloned());
                Some(merged)
            }
            (Some(g), None) => Some(g.clone()),
            (None, _) => shared.post_hooks.clone(),
        };
        shared.post_hooks = post_hooks;
    }

    /// Search for a manifest file in `dir`, returning the path to the first one found.
    ///
    /// Searched in order: `claudes.toml`, `.claudes.toml`, `claudes.json`, `.claudes.json`.
    pub fn discover(dir: &Path) -> Option<PathBuf> {
        const CANDIDATES: &[&str] = &[
            "claudes.toml",
            ".claudes.toml",
            "claudes.json",
            ".claudes.json",
        ];
        for name in CANDIDATES {
            let path = dir.join(name);
            if path.exists() {
                return Some(path);
            }
        }
        None
    }

    /// Scan task prompts for file path references and warn if multiple tasks mention the same path.
    ///
    /// Returns one warning string per overlapping path. A token is treated as a file path if it
    /// contains `'/'` or ends with `.rs`, `.ts`, `.py`, `.js`, `.toml`, `.json`, `.yaml`,
    /// `.yml`, or `.md`. This is a best-effort heuristic — it may produce false positives for
    /// URLs and similar patterns.
    pub fn check_file_overlaps(&self) -> Vec<String> {
        fn extract_paths(text: &str) -> std::collections::HashSet<String> {
            const EXTENSIONS: &[&str] = &[
                ".rs", ".ts", ".py", ".js", ".toml", ".json", ".yaml", ".yml", ".md",
            ];
            let mut paths = std::collections::HashSet::new();
            for word in text.split_whitespace() {
                let token = word.trim_matches(|c: char| {
                    matches!(
                        c,
                        '`' | '"' | '\'' | '(' | ')' | '[' | ']' | ',' | ':' | ';'
                    )
                });
                if token.is_empty() {
                    continue;
                }
                if token.contains('/') || EXTENSIONS.iter().any(|ext| token.ends_with(ext)) {
                    paths.insert(token.to_string());
                }
            }
            paths
        }

        // Build a set of (a, b) pairs where a and b are sequenced via depends_on
        // (directly or transitively). For any such pair, an overlap is intentional.
        let mut sequenced: std::collections::HashSet<(String, String)> =
            std::collections::HashSet::new();

        // Build adjacency map: name -> list of names it depends on.
        let mut dep_map: HashMap<&str, Vec<&str>> = HashMap::new();
        for task in &self.tasks {
            let deps: Vec<&str> = task
                .depends_on
                .as_deref()
                .unwrap_or(&[])
                .iter()
                .map(|s| s.as_str())
                .collect();
            dep_map.insert(&task.name, deps);
        }

        // For each task, do a BFS/DFS to collect all transitive dependencies.
        for task in &self.tasks {
            let mut visited: std::collections::HashSet<&str> = std::collections::HashSet::new();
            let mut stack: Vec<&str> = dep_map.get(task.name.as_str()).cloned().unwrap_or_default();
            while let Some(dep) = stack.pop() {
                if visited.insert(dep)
                    && let Some(transitive) = dep_map.get(dep)
                {
                    stack.extend(transitive.iter().copied());
                }
            }
            for ancestor in visited {
                // task depends on ancestor (directly or transitively) — they are sequenced.
                let a = task.name.clone();
                let b = ancestor.to_string();
                sequenced.insert((a.clone(), b.clone()));
                sequenced.insert((b, a));
            }
        }

        let mut path_to_tasks: HashMap<String, Vec<&str>> = HashMap::new();
        for task in &self.tasks {
            for path in extract_paths(&task.prompt) {
                path_to_tasks.entry(path).or_default().push(&task.name);
            }
        }

        let mut warnings: Vec<String> = path_to_tasks
            .into_iter()
            .filter_map(|(path, mut tasks)| {
                if tasks.len() < 2 {
                    return None;
                }
                tasks.sort();
                // Collect the subset of tasks that are NOT sequenced with all others.
                // Warn only when at least one pair of tasks is unsequenced.
                let unsequenced_pairs: Vec<(&str, &str)> = tasks
                    .iter()
                    .enumerate()
                    .flat_map(|(i, a)| tasks[i + 1..].iter().map(move |b| (*a, *b)))
                    .filter(|(a, b)| !sequenced.contains(&(a.to_string(), b.to_string())))
                    .collect();
                if unsequenced_pairs.is_empty() {
                    return None;
                }
                let names = tasks
                    .iter()
                    .map(|n| format!("'{n}'"))
                    .collect::<Vec<_>>()
                    .join(" and ");
                Some(format!(
                    "tasks {names} both reference '{path}' — consider sequencing them"
                ))
            })
            .collect();
        warnings.sort();
        warnings
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
            if !seen.insert(task.name.clone()) {
                errors.push(format!("duplicate task name: {}", task.name));
            }
        }

        for task in &self.tasks {
            if let Some(profile_name) = &task.profile {
                let exists = self
                    .profiles
                    .as_ref()
                    .map(|m| m.contains_key(profile_name.as_str()))
                    .unwrap_or(false);
                if !exists {
                    errors.push(format!(
                        "task '{}' references unknown profile '{profile_name}'",
                        task.name
                    ));
                }
            }

            if let Err(task_errors) = task.validate() {
                for e in task_errors {
                    errors.push(format!("task '{}': {}", task.name, e));
                }
            }

            // Validate depends_on references exist.
            if let Some(deps) = &task.depends_on {
                for dep in deps {
                    if !seen.contains(dep) {
                        errors.push(format!(
                            "task '{}': depends_on references unknown task '{dep}'",
                            task.name
                        ));
                    }
                }
            }
        }

        // Check for dependency cycles.
        if errors.is_empty()
            && let Err(cycle_errors) = self.check_dependency_cycles()
        {
            errors.extend(cycle_errors);
        }

        if errors.is_empty() {
            Ok(())
        } else {
            Err(errors)
        }
    }

    /// Check for dependency cycles using DFS.
    fn check_dependency_cycles(&self) -> Result<(), Vec<String>> {
        use std::collections::{HashMap, HashSet};

        let deps: HashMap<&str, Vec<&str>> = self
            .tasks
            .iter()
            .map(|t| {
                let d = t
                    .depends_on
                    .as_ref()
                    .map(|v| v.iter().map(|s| s.as_str()).collect())
                    .unwrap_or_default();
                (t.name.as_str(), d)
            })
            .collect();

        let mut visited = HashSet::new();
        let mut in_stack = HashSet::new();
        let mut errors = Vec::new();

        for task in &self.tasks {
            if !visited.contains(task.name.as_str())
                && let Some(cycle) = Self::dfs_cycle(&task.name, &deps, &mut visited, &mut in_stack)
            {
                errors.push(format!("dependency cycle detected: {}", cycle.join(" -> ")));
            }
        }

        if errors.is_empty() {
            Ok(())
        } else {
            Err(errors)
        }
    }

    fn dfs_cycle<'a>(
        node: &'a str,
        deps: &HashMap<&'a str, Vec<&'a str>>,
        visited: &mut std::collections::HashSet<&'a str>,
        in_stack: &mut std::collections::HashSet<&'a str>,
    ) -> Option<Vec<String>> {
        visited.insert(node);
        in_stack.insert(node);

        if let Some(neighbors) = deps.get(node) {
            for &neighbor in neighbors {
                if !visited.contains(neighbor) {
                    if let Some(mut cycle) = Self::dfs_cycle(neighbor, deps, visited, in_stack) {
                        cycle.insert(0, node.to_string());
                        return Some(cycle);
                    }
                } else if in_stack.contains(neighbor) {
                    return Some(vec![node.to_string(), neighbor.to_string()]);
                }
            }
        }

        in_stack.remove(node);
        None
    }

    /// Return tasks in topological order (dependencies first).
    ///
    /// Tasks with no dependencies come first. Tasks are grouped into "layers"
    /// where each layer can run in parallel. Returns an error if cycles exist.
    pub fn topological_order(&self) -> Result<Vec<Vec<&Task>>, String> {
        use std::collections::{HashMap, HashSet, VecDeque};

        let task_map: HashMap<&str, &Task> =
            self.tasks.iter().map(|t| (t.name.as_str(), t)).collect();

        // Build in-degree map.
        let mut in_degree: HashMap<&str, usize> = HashMap::new();
        let mut dependents: HashMap<&str, Vec<&str>> = HashMap::new();
        for task in &self.tasks {
            in_degree.entry(task.name.as_str()).or_insert(0);
            if let Some(deps) = &task.depends_on {
                *in_degree.entry(task.name.as_str()).or_insert(0) += deps.len();
                for dep in deps {
                    dependents
                        .entry(dep.as_str())
                        .or_default()
                        .push(task.name.as_str());
                }
            }
        }

        // Kahn's algorithm — layer by layer.
        let mut layers: Vec<Vec<&Task>> = Vec::new();
        let mut queue: VecDeque<&str> = in_degree
            .iter()
            .filter(|(_, deg)| **deg == 0)
            .map(|(name, _)| *name)
            .collect();
        let mut processed: HashSet<&str> = HashSet::new();

        while !queue.is_empty() {
            let layer: Vec<&str> = queue.drain(..).collect();
            let mut task_layer: Vec<&Task> = Vec::new();

            for name in &layer {
                processed.insert(name);
                if let Some(task) = task_map.get(name) {
                    task_layer.push(task);
                }
                if let Some(deps) = dependents.get(name) {
                    for &dep in deps {
                        if let Some(deg) = in_degree.get_mut(dep) {
                            *deg -= 1;
                            if *deg == 0 {
                                queue.push_back(dep);
                            }
                        }
                    }
                }
            }

            if !task_layer.is_empty() {
                layers.push(task_layer);
            }
        }

        if processed.len() != self.tasks.len() {
            return Err("dependency cycle detected".to_string());
        }

        Ok(layers)
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

    /// Load append_system_prompt from a file (mutually exclusive with `append_system_prompt`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub append_system_prompt_file: Option<String>,

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

    /// Shell commands that always run after task completion, regardless of outcome.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub finally_hooks: Option<Vec<String>>,

    /// Skill files (markdown/text) whose contents are appended to the system prompt.
    /// Shared skills are prepended to any task-level skills.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub skills: Option<Vec<String>>,

    /// Path to a Claude CLI settings JSON file (maps to `--settings`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub settings: Option<String>,

    /// Comma-separated list of setting sources to load, e.g. `"project"` or `"user,project"`
    /// (maps to `--setting-sources`). Useful in CI to skip user-level settings.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub setting_sources: Option<String>,
}

/// A fully resolved task. Every field is explicit.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Task {
    /// Task identifier (used for worktree/branch naming, logs).
    pub name: String,

    /// The task prompt.
    #[serde(default)]
    pub prompt: String,

    /// Load the prompt from a file (mutually exclusive with `prompt`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub prompt_file: Option<String>,

    /// Named profile to apply to this task (resolved before execution).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub profile: Option<String>,

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

    /// Load append_system_prompt from a file (mutually exclusive with `append_system_prompt`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub append_system_prompt_file: Option<String>,

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

    /// Shell commands that always run after task completion, regardless of outcome.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub finally_hooks: Option<Vec<String>>,

    /// Task names that must complete successfully before this task starts.
    /// If any dependency fails, this task is skipped.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub depends_on: Option<Vec<String>>,

    /// Optional shell condition command.
    ///
    /// If set, this command is executed in the project root before isolation setup.
    /// Exit code 0 means the task should be **skipped** (condition is met, no work needed).
    /// Any non-zero exit code (or a spawn failure) means the task runs normally.
    /// Condition-skipped tasks do not block downstream dependents.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub condition: Option<String>,

    /// Skill files (markdown/text) whose contents are appended to the system prompt.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub skills: Option<Vec<String>>,

    /// Path to a Claude CLI settings JSON file (maps to `--settings`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub settings: Option<String>,

    /// Comma-separated list of setting sources to load, e.g. `"project"` or `"user,project"`
    /// (maps to `--setting-sources`). Useful in CI to skip user-level settings.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub setting_sources: Option<String>,
}

impl Task {
    /// Create a new task with the given name and prompt.
    pub fn new(name: impl Into<String>, prompt: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            prompt: prompt.into(),
            profile: None,
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
            prompt_file: None,
            append_system_prompt_file: None,
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
            finally_hooks: None,
            depends_on: None,
            condition: None,
            skills: None,
            settings: None,
            setting_sources: None,
        }
    }

    /// Validate this task.
    fn validate(&self) -> Result<(), Vec<String>> {
        let mut errors = Vec::new();

        if self.name.is_empty() {
            errors.push("name must not be empty".into());
        }

        if self.prompt.is_empty() && self.prompt_file.is_none() {
            errors.push("prompt must not be empty".into());
        }

        if !self.prompt.is_empty() && self.prompt_file.is_some() {
            errors.push("cannot set both prompt and prompt_file".into());
        }

        if self.append_system_prompt.is_some() && self.append_system_prompt_file.is_some() {
            errors
                .push("cannot set both append_system_prompt and append_system_prompt_file".into());
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

    /// Set the named profile for this task.
    pub fn profile(mut self, profile: impl Into<String>) -> Self {
        self.task.profile = Some(profile.into());
        self
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

    /// Set the finally hooks (always run, regardless of outcome).
    pub fn finally_hooks(mut self, finally_hooks: Vec<String>) -> Self {
        self.task.finally_hooks = Some(finally_hooks);
        self
    }

    /// Set skill files to inject into the system prompt.
    pub fn skills(mut self, skills: Vec<String>) -> Self {
        self.task.skills = Some(skills);
        self
    }

    /// Set the Claude CLI settings file path.
    pub fn settings(mut self, settings: impl Into<String>) -> Self {
        self.task.settings = Some(settings.into());
        self
    }

    /// Set the Claude CLI setting sources.
    pub fn setting_sources(mut self, setting_sources: impl Into<String>) -> Self {
        self.task.setting_sources = Some(setting_sources.into());
        self
    }

    /// Set a shell condition for this task.
    ///
    /// The command runs in the project root before isolation setup.
    /// Exit 0 = skip the task; non-zero (or spawn failure) = run the task.
    pub fn condition(mut self, condition: impl Into<String>) -> Self {
        self.task.condition = Some(condition.into());
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
    fn load_global_defaults_missing_file_returns_none() {
        let dir = tempfile::tempdir().unwrap();
        assert!(load_global_defaults_from(dir.path()).is_none());
    }

    #[test]
    fn load_global_defaults_reads_toml() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("claudes")).unwrap();
        std::fs::write(
            dir.path().join("claudes/defaults.toml"),
            r#"model = "claude-opus-4-6"
max_turns = 10
"#,
        )
        .unwrap();
        let shared = load_global_defaults_from(dir.path()).unwrap();
        assert_eq!(shared.model.as_deref(), Some("claude-opus-4-6"));
        assert_eq!(shared.max_turns, Some(10));
    }

    #[test]
    fn load_global_defaults_reads_json() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("claudes")).unwrap();
        std::fs::write(
            dir.path().join("claudes/defaults.json"),
            r#"{"model": "claude-haiku-4-5-20251001", "timeout_secs": 300}"#,
        )
        .unwrap();
        let shared = load_global_defaults_from(dir.path()).unwrap();
        assert_eq!(shared.model.as_deref(), Some("claude-haiku-4-5-20251001"));
        assert_eq!(shared.timeout_secs, Some(300));
    }

    #[test]
    fn load_global_defaults_prefers_toml_over_json() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("claudes")).unwrap();
        std::fs::write(
            dir.path().join("claudes/defaults.toml"),
            r#"model = "from-toml""#,
        )
        .unwrap();
        std::fs::write(
            dir.path().join("claudes/defaults.json"),
            r#"{"model": "from-json"}"#,
        )
        .unwrap();
        let shared = load_global_defaults_from(dir.path()).unwrap();
        assert_eq!(shared.model.as_deref(), Some("from-toml"));
    }

    #[test]
    fn apply_global_defaults_fills_missing_fields() {
        let mut manifest = Manifest::new(vec![Task::new("t", "p")]);
        let global = Shared {
            model: Some("claude-opus-4-6".into()),
            max_turns: Some(5),
            effort: Some("high".into()),
            ..Default::default()
        };
        manifest.apply_global_defaults(&global);
        let shared = manifest.shared.as_ref().unwrap();
        assert_eq!(shared.model.as_deref(), Some("claude-opus-4-6"));
        assert_eq!(shared.max_turns, Some(5));
        assert_eq!(shared.effort.as_deref(), Some("high"));
    }

    #[test]
    fn apply_global_defaults_does_not_override_manifest_shared() {
        let mut manifest = Manifest::new(vec![Task::new("t", "p")]);
        manifest.shared = Some(Shared {
            model: Some("manifest-model".into()),
            max_turns: Some(10),
            ..Default::default()
        });
        let global = Shared {
            model: Some("global-model".into()),
            max_turns: Some(1),
            timeout_secs: Some(60),
            ..Default::default()
        };
        manifest.apply_global_defaults(&global);
        let shared = manifest.shared.as_ref().unwrap();
        assert_eq!(shared.model.as_deref(), Some("manifest-model"));
        assert_eq!(shared.max_turns, Some(10));
        assert_eq!(shared.timeout_secs, Some(60));
    }

    #[test]
    fn apply_global_defaults_prepends_pre_hooks() {
        let mut manifest = Manifest::new(vec![Task::new("t", "p")]);
        manifest.shared = Some(Shared {
            pre_hooks: Some(vec!["echo shared".into()]),
            ..Default::default()
        });
        let global = Shared {
            pre_hooks: Some(vec!["echo global".into()]),
            ..Default::default()
        };
        manifest.apply_global_defaults(&global);
        assert_eq!(
            manifest.shared.as_ref().unwrap().pre_hooks.as_deref(),
            Some(["echo global".to_string(), "echo shared".to_string()].as_slice())
        );
    }

    #[test]
    fn apply_global_defaults_prepends_post_hooks() {
        let mut manifest = Manifest::new(vec![Task::new("t", "p")]);
        manifest.shared = Some(Shared {
            post_hooks: Some(vec!["echo shared".into()]),
            ..Default::default()
        });
        let global = Shared {
            post_hooks: Some(vec!["echo global".into()]),
            ..Default::default()
        };
        manifest.apply_global_defaults(&global);
        assert_eq!(
            manifest.shared.as_ref().unwrap().post_hooks.as_deref(),
            Some(["echo global".to_string(), "echo shared".to_string()].as_slice())
        );
    }

    #[test]
    fn global_defaults_are_lowest_priority_in_full_resolution() {
        // task > manifest shared > global defaults
        let mut task = Task::new("t", "p");
        task.model = Some("task-model".into());

        let mut manifest = Manifest::new(vec![task]);
        manifest.shared = Some(Shared {
            model: Some("shared-model".into()),
            max_turns: Some(7),
            ..Default::default()
        });

        let global = Shared {
            model: Some("global-model".into()),
            max_turns: Some(1),
            timeout_secs: Some(120),
            ..Default::default()
        };
        manifest.apply_global_defaults(&global);

        let resolved = manifest.resolve();
        let t = &resolved.tasks[0];
        // task wins over shared and global
        assert_eq!(t.model.as_deref(), Some("task-model"));
        // shared wins over global
        assert_eq!(t.max_turns, Some(7));
        // global fills in what neither task nor shared set
        assert_eq!(t.timeout_secs, Some(120));
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
    fn discover_finds_files_in_order() {
        let dir = tempfile::tempdir().unwrap();
        let base = dir.path();

        // Nothing present — returns None.
        assert!(Manifest::discover(base).is_none());

        // .claudes.json present — found.
        let json_hidden = base.join(".claudes.json");
        std::fs::write(&json_hidden, "{}").unwrap();
        assert_eq!(Manifest::discover(base).unwrap(), json_hidden);

        // claudes.json present — takes priority over .claudes.json.
        let json = base.join("claudes.json");
        std::fs::write(&json, "{}").unwrap();
        assert_eq!(Manifest::discover(base).unwrap(), json);

        // .claudes.toml present — takes priority over claudes.json.
        let toml_hidden = base.join(".claudes.toml");
        std::fs::write(&toml_hidden, "").unwrap();
        assert_eq!(Manifest::discover(base).unwrap(), toml_hidden);

        // claudes.toml present — highest priority.
        let toml = base.join("claudes.toml");
        std::fs::write(&toml, "").unwrap();
        assert_eq!(Manifest::discover(base).unwrap(), toml);
    }

    #[test]
    fn validate_prompt_file_without_prompt_is_valid() {
        let mut task = Task::new("t", "");
        task.prompt_file = Some("prompt.txt".into());
        let manifest = Manifest::new(vec![task]);
        assert!(manifest.validate().is_ok());
    }

    #[test]
    fn validate_both_prompt_and_prompt_file_is_error() {
        let mut task = Task::new("t", "inline");
        task.prompt_file = Some("file.txt".into());
        let manifest = Manifest::new(vec![task]);
        let errs = manifest.validate().unwrap_err();
        assert!(
            errs.iter()
                .any(|e| e.contains("cannot set both prompt and prompt_file"))
        );
    }

    #[test]
    fn validate_both_append_system_prompt_variants_is_error() {
        let mut task = Task::new("t", "prompt");
        task.append_system_prompt = Some("inline".into());
        task.append_system_prompt_file = Some("file.txt".into());
        let manifest = Manifest::new(vec![task]);
        let errs = manifest.validate().unwrap_err();
        assert!(errs.iter().any(|e| {
            e.contains("cannot set both append_system_prompt and append_system_prompt_file")
        }));
    }

    #[test]
    fn resolve_files_loads_prompt_from_file() {
        use std::io::Write;
        let dir = tempfile::tempdir().unwrap();
        let mut f = std::fs::File::create(dir.path().join("task.txt")).unwrap();
        write!(f, "do the thing").unwrap();

        let mut task = Task::new("t", "");
        task.prompt_file = Some("task.txt".into());
        let mut manifest = Manifest::new(vec![task]);
        manifest.resolve_files(dir.path()).unwrap();

        assert_eq!(manifest.tasks[0].prompt, "do the thing");
        assert!(manifest.tasks[0].prompt_file.is_none());
    }

    #[test]
    fn resolve_files_errors_if_both_prompt_and_file_set() {
        use std::io::Write;
        let dir = tempfile::tempdir().unwrap();
        let mut f = std::fs::File::create(dir.path().join("task.txt")).unwrap();
        write!(f, "content").unwrap();

        let mut task = Task::new("t", "inline");
        task.prompt_file = Some("task.txt".into());
        let mut manifest = Manifest::new(vec![task]);
        let err = manifest.resolve_files(dir.path()).unwrap_err().to_string();
        assert!(err.contains("cannot set both prompt and prompt_file"));
    }

    #[test]
    fn resolve_files_errors_if_prompt_file_missing() {
        let dir = tempfile::tempdir().unwrap();
        let mut task = Task::new("t", "");
        task.prompt_file = Some("nonexistent.txt".into());
        let mut manifest = Manifest::new(vec![task]);
        let err = manifest.resolve_files(dir.path()).unwrap_err().to_string();
        assert!(err.contains("not found"));
    }

    #[test]
    fn resolve_files_loads_append_system_prompt_from_file() {
        use std::io::Write;
        let dir = tempfile::tempdir().unwrap();
        let mut f = std::fs::File::create(dir.path().join("sys.txt")).unwrap();
        write!(f, "extra context").unwrap();

        let mut task = Task::new("t", "prompt");
        task.append_system_prompt_file = Some("sys.txt".into());
        let mut manifest = Manifest::new(vec![task]);
        manifest.resolve_files(dir.path()).unwrap();

        assert_eq!(
            manifest.tasks[0].append_system_prompt.as_deref(),
            Some("extra context")
        );
        assert!(manifest.tasks[0].append_system_prompt_file.is_none());
    }

    #[test]
    fn resolve_files_errors_if_both_append_system_prompts_set() {
        use std::io::Write;
        let dir = tempfile::tempdir().unwrap();
        let mut f = std::fs::File::create(dir.path().join("sys.txt")).unwrap();
        write!(f, "extra").unwrap();

        let mut task = Task::new("t", "prompt");
        task.append_system_prompt = Some("inline".into());
        task.append_system_prompt_file = Some("sys.txt".into());
        let mut manifest = Manifest::new(vec![task]);
        let err = manifest.resolve_files(dir.path()).unwrap_err().to_string();
        assert!(err.contains("cannot set both append_system_prompt and append_system_prompt_file"));
    }

    #[test]
    fn resolve_files_shared_append_system_prompt_file() {
        use std::io::Write;
        let dir = tempfile::tempdir().unwrap();
        let mut f = std::fs::File::create(dir.path().join("shared_sys.txt")).unwrap();
        write!(f, "shared context").unwrap();

        let mut manifest = Manifest::new(vec![Task::new("t", "prompt")]);
        manifest.shared = Some(Shared {
            append_system_prompt_file: Some("shared_sys.txt".into()),
            ..Default::default()
        });
        manifest.resolve_files(dir.path()).unwrap();

        assert_eq!(
            manifest
                .shared
                .as_ref()
                .unwrap()
                .append_system_prompt
                .as_deref(),
            Some("shared context")
        );
        assert!(
            manifest
                .shared
                .as_ref()
                .unwrap()
                .append_system_prompt_file
                .is_none()
        );
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

    #[test]
    fn resolve_task_with_profile() {
        let mut task = Task::new("t1", "do it");
        task.profile = Some("fast".into());
        let mut manifest = Manifest::new(vec![task]);
        manifest.shared = Some(Shared {
            model: Some("opus".into()),
            max_turns: Some(10),
            ..Default::default()
        });
        manifest.profiles = Some({
            let mut m = HashMap::new();
            m.insert(
                "fast".into(),
                Shared {
                    max_turns: Some(3),
                    effort: Some("low".into()),
                    ..Default::default()
                },
            );
            m
        });

        let resolved = manifest.resolve();
        let task = &resolved.tasks[0];
        // Profile max_turns overrides shared max_turns.
        assert_eq!(task.max_turns, Some(3));
        // Profile effort is applied.
        assert_eq!(task.effort.as_deref(), Some("low"));
        // Model falls through to shared.
        assert_eq!(task.model.as_deref(), Some("opus"));
    }

    #[test]
    fn resolve_profile_overrides_shared() {
        let mut task = Task::new("t1", "do it");
        task.profile = Some("fast".into());
        let mut manifest = Manifest::new(vec![task]);
        manifest.shared = Some(Shared {
            max_turns: Some(10),
            ..Default::default()
        });
        manifest.profiles = Some({
            let mut m = HashMap::new();
            m.insert(
                "fast".into(),
                Shared {
                    max_turns: Some(3),
                    ..Default::default()
                },
            );
            m
        });

        let resolved = manifest.resolve();
        assert_eq!(resolved.tasks[0].max_turns, Some(3));
    }

    #[test]
    fn resolve_task_overrides_profile() {
        let mut task = Task::new("t1", "do it");
        task.profile = Some("fast".into());
        task.max_turns = Some(20);
        let mut manifest = Manifest::new(vec![task]);
        manifest.profiles = Some({
            let mut m = HashMap::new();
            m.insert(
                "fast".into(),
                Shared {
                    max_turns: Some(3),
                    ..Default::default()
                },
            );
            m
        });

        let resolved = manifest.resolve();
        assert_eq!(resolved.tasks[0].max_turns, Some(20));
    }

    #[test]
    fn validate_missing_profile_error() {
        let mut task = Task::new("t1", "do it");
        task.profile = Some("nonexistent".into());
        let manifest = Manifest::new(vec![task]);
        let errs = manifest.validate().unwrap_err();
        assert!(errs.iter().any(|e| e.contains("unknown profile")));
    }

    #[test]
    fn validate_no_profiles_is_fine() {
        let manifest = Manifest::new(vec![Task::new("t1", "do it")]);
        assert!(manifest.validate().is_ok());
    }

    #[test]
    fn check_file_overlaps_detects_overlap() {
        let manifest = Manifest::new(vec![
            Task::new("task-a", "Fix the bug in src/lib.rs and update Cargo.toml"),
            Task::new("task-b", "Add tests in src/lib.rs"),
        ]);
        let warnings = manifest.check_file_overlaps();
        assert!(
            warnings
                .iter()
                .any(|w| w.contains("src/lib.rs") && w.contains("task-a") && w.contains("task-b")),
            "expected overlap warning for src/lib.rs, got: {warnings:?}"
        );
    }

    #[test]
    fn check_file_overlaps_no_overlap_returns_empty() {
        let manifest = Manifest::new(vec![
            Task::new("task-a", "Edit src/main.rs"),
            Task::new("task-b", "Edit src/lib.rs"),
        ]);
        let warnings = manifest.check_file_overlaps();
        assert!(
            warnings.is_empty(),
            "expected no warnings, got: {warnings:?}"
        );
    }

    #[test]
    fn check_file_overlaps_path_in_only_one_task_no_warning() {
        let manifest = Manifest::new(vec![
            Task::new("task-a", "Update Cargo.toml with new dependencies"),
            Task::new("task-b", "Write documentation in README.md"),
        ]);
        let warnings = manifest.check_file_overlaps();
        assert!(
            warnings.is_empty(),
            "expected no warnings, got: {warnings:?}"
        );
    }

    #[test]
    fn check_file_overlaps_extension_only_match() {
        let manifest = Manifest::new(vec![
            Task::new("task-a", "Update Cargo.toml"),
            Task::new("task-b", "Also update Cargo.toml"),
        ]);
        let warnings = manifest.check_file_overlaps();
        assert!(
            warnings.iter().any(|w| w.contains("Cargo.toml")),
            "expected overlap warning for Cargo.toml, got: {warnings:?}"
        );
    }

    #[test]
    fn check_file_overlaps_single_task_returns_empty() {
        let manifest = Manifest::new(vec![Task::new("only", "Edit src/lib.rs and Cargo.toml")]);
        let warnings = manifest.check_file_overlaps();
        assert!(
            warnings.is_empty(),
            "expected no warnings, got: {warnings:?}"
        );
    }

    #[test]
    fn check_file_overlaps_suppressed_when_direct_depends_on() {
        let manifest = Manifest::new(vec![Task::new("task-a", "Fix the bug in src/lib.rs"), {
            let mut t = Task::new("task-b", "Add tests in src/lib.rs");
            t.depends_on = Some(vec!["task-a".into()]);
            t
        }]);
        let warnings = manifest.check_file_overlaps();
        assert!(
            warnings.is_empty(),
            "expected no warnings for sequenced tasks, got: {warnings:?}"
        );
    }

    #[test]
    fn check_file_overlaps_suppressed_when_transitive_depends_on() {
        // task-c -> task-b -> task-a: all pairs are sequenced
        let manifest = Manifest::new(vec![
            Task::new("task-a", "Edit src/lib.rs"),
            {
                let mut t = Task::new("task-b", "Edit src/lib.rs");
                t.depends_on = Some(vec!["task-a".into()]);
                t
            },
            {
                let mut t = Task::new("task-c", "Edit src/lib.rs");
                t.depends_on = Some(vec!["task-b".into()]);
                t
            },
        ]);
        let warnings = manifest.check_file_overlaps();
        assert!(
            warnings.is_empty(),
            "expected no warnings for transitively sequenced tasks, got: {warnings:?}"
        );
    }

    #[test]
    fn check_file_overlaps_warns_for_unsequenced_pair_among_sequenced() {
        // task-b depends on task-a, but task-c is independent — task-a/task-c and task-b/task-c
        // pairs should still warn.
        let manifest = Manifest::new(vec![
            Task::new("task-a", "Edit src/lib.rs"),
            {
                let mut t = Task::new("task-b", "Edit src/lib.rs");
                t.depends_on = Some(vec!["task-a".into()]);
                t
            },
            Task::new("task-c", "Edit src/lib.rs"),
        ]);
        let warnings = manifest.check_file_overlaps();
        assert!(
            warnings
                .iter()
                .any(|w| w.contains("src/lib.rs") && w.contains("task-c")),
            "expected overlap warning involving task-c, got: {warnings:?}"
        );
    }

    #[test]
    fn check_file_overlaps_suppressed_when_chain_declares_dependency() {
        // tasks a and b both touch src/lib.rs, but are sequenced via chains.
        // Without desugar_chains first, check_file_overlaps would emit a warning.
        let mut manifest = Manifest::new(vec![
            Task::new("task-a", "Fix the bug in src/lib.rs"),
            Task::new("task-b", "Add tests in src/lib.rs"),
        ]);
        manifest.chains = Some(vec![vec![
            ChainStep::Single("task-a".into()),
            ChainStep::Single("task-b".into()),
        ]]);

        // Desugar first — this is the ordering fix from issue #442.
        manifest.desugar_chains();
        let warnings = manifest.check_file_overlaps();
        assert!(
            warnings.is_empty(),
            "expected no warnings for chain-sequenced tasks, got: {warnings:?}"
        );
    }

    #[test]
    fn deserialize_json_without_created_at_uses_default() {
        let json = r#"{
            "version": 1,
            "tasks": [{ "name": "t1", "prompt": "do it" }]
        }"#;
        let manifest: Manifest = serde_json::from_str(json).unwrap();
        assert_eq!(manifest.version, 1);
        assert_eq!(manifest.tasks[0].name, "t1");
        // created_at should be populated (close to now)
        let diff = Utc::now() - manifest.created_at;
        assert!(diff.num_seconds() < 5);
    }

    #[test]
    fn deserialize_json_without_version_defaults_to_1() {
        let json = r#"{
            "created_at": "2026-03-18T10:30:00Z",
            "tasks": [{ "name": "t1", "prompt": "do it" }]
        }"#;
        let manifest: Manifest = serde_json::from_str(json).unwrap();
        assert_eq!(manifest.version, 1);
    }

    #[test]
    fn deserialize_toml_without_version_or_created_at() {
        let toml = r#"
[[tasks]]
name = "t1"
prompt = "do it"
"#;
        let manifest = Manifest::from_toml(toml).unwrap();
        assert_eq!(manifest.version, 1);
        assert_eq!(manifest.tasks[0].name, "t1");
        let diff = Utc::now() - manifest.created_at;
        assert!(diff.num_seconds() < 5);
    }

    #[test]
    fn deserialize_explicit_values_are_preserved() {
        let json = r#"{
            "version": 1,
            "created_at": "2020-01-01T00:00:00Z",
            "tasks": [{ "name": "t1", "prompt": "do it" }]
        }"#;
        let manifest: Manifest = serde_json::from_str(json).unwrap();
        assert_eq!(manifest.version, 1);
        assert_eq!(
            manifest.created_at.to_rfc3339(),
            "2020-01-01T00:00:00+00:00"
        );
    }

    // ========================================================================
    // depends_on tests
    // ========================================================================

    #[test]
    fn validate_depends_on_unknown_task_is_error() {
        let manifest = Manifest::new(vec![{
            let mut t = Task::new("a", "do a");
            t.depends_on = Some(vec!["nonexistent".into()]);
            t
        }]);
        let err = manifest.validate().unwrap_err();
        assert!(err.iter().any(|e| e.contains("nonexistent")));
    }

    #[test]
    fn validate_depends_on_valid_reference_is_ok() {
        let manifest = Manifest::new(vec![Task::new("a", "do a"), {
            let mut t = Task::new("b", "do b");
            t.depends_on = Some(vec!["a".into()]);
            t
        }]);
        assert!(manifest.validate().is_ok());
    }

    #[test]
    fn validate_dependency_cycle_is_error() {
        let manifest = Manifest::new(vec![
            {
                let mut t = Task::new("a", "do a");
                t.depends_on = Some(vec!["b".into()]);
                t
            },
            {
                let mut t = Task::new("b", "do b");
                t.depends_on = Some(vec!["a".into()]);
                t
            },
        ]);
        let err = manifest.validate().unwrap_err();
        assert!(err.iter().any(|e| e.contains("cycle")));
    }

    #[test]
    fn validate_self_dependency_is_cycle() {
        let manifest = Manifest::new(vec![{
            let mut t = Task::new("a", "do a");
            t.depends_on = Some(vec!["a".into()]);
            t
        }]);
        let err = manifest.validate().unwrap_err();
        assert!(err.iter().any(|e| e.contains("cycle")));
    }

    #[test]
    fn topological_order_no_deps() {
        let manifest = Manifest::new(vec![
            Task::new("a", "do a"),
            Task::new("b", "do b"),
            Task::new("c", "do c"),
        ]);
        let layers = manifest.topological_order().unwrap();
        assert_eq!(layers.len(), 1);
        assert_eq!(layers[0].len(), 3);
    }

    #[test]
    fn topological_order_linear_chain() {
        let manifest = Manifest::new(vec![
            Task::new("a", "do a"),
            {
                let mut t = Task::new("b", "do b");
                t.depends_on = Some(vec!["a".into()]);
                t
            },
            {
                let mut t = Task::new("c", "do c");
                t.depends_on = Some(vec!["b".into()]);
                t
            },
        ]);
        let layers = manifest.topological_order().unwrap();
        assert_eq!(layers.len(), 3);
        assert_eq!(layers[0][0].name, "a");
        assert_eq!(layers[1][0].name, "b");
        assert_eq!(layers[2][0].name, "c");
    }

    #[test]
    fn topological_order_fan_out_fan_in() {
        // a -> (b1, b2) -> c
        let manifest = Manifest::new(vec![
            Task::new("a", "do a"),
            {
                let mut t = Task::new("b1", "do b1");
                t.depends_on = Some(vec!["a".into()]);
                t
            },
            {
                let mut t = Task::new("b2", "do b2");
                t.depends_on = Some(vec!["a".into()]);
                t
            },
            {
                let mut t = Task::new("c", "do c");
                t.depends_on = Some(vec!["b1".into(), "b2".into()]);
                t
            },
        ]);
        let layers = manifest.topological_order().unwrap();
        assert_eq!(layers.len(), 3);
        assert_eq!(layers[0][0].name, "a");
        // Layer 1 has b1 and b2 in some order.
        let layer1_names: Vec<&str> = layers[1].iter().map(|t| t.name.as_str()).collect();
        assert!(layer1_names.contains(&"b1"));
        assert!(layer1_names.contains(&"b2"));
        assert_eq!(layers[2][0].name, "c");
    }

    #[test]
    fn depends_on_preserved_through_resolve() {
        let manifest = Manifest::new(vec![Task::new("a", "do a"), {
            let mut t = Task::new("b", "do b");
            t.depends_on = Some(vec!["a".into()]);
            t
        }]);
        let resolved = manifest.resolve();
        assert_eq!(
            resolved.tasks[1].depends_on.as_ref().unwrap(),
            &vec!["a".to_string()]
        );
    }

    #[test]
    fn depends_on_serialization_roundtrip() {
        let manifest = Manifest::new(vec![Task::new("a", "do a"), {
            let mut t = Task::new("b", "do b");
            t.depends_on = Some(vec!["a".into()]);
            t
        }]);
        let json = serde_json::to_string(&manifest).unwrap();
        let parsed: Manifest = serde_json::from_str(&json).unwrap();
        assert_eq!(
            parsed.tasks[1].depends_on.as_ref().unwrap(),
            &vec!["a".to_string()]
        );
    }

    // ========================================================================
    // chains tests
    // ========================================================================

    #[test]
    fn desugar_linear_chain() {
        let mut manifest = Manifest::new(vec![
            Task::new("a", "do a"),
            Task::new("b", "do b"),
            Task::new("c", "do c"),
        ]);
        manifest.chains = Some(vec![vec![
            ChainStep::Single("a".into()),
            ChainStep::Single("b".into()),
            ChainStep::Single("c".into()),
        ]]);

        manifest.desugar_chains();

        assert!(manifest.tasks[0].depends_on.is_none());
        assert_eq!(manifest.tasks[1].depends_on.as_ref().unwrap(), &["a"]);
        assert_eq!(manifest.tasks[2].depends_on.as_ref().unwrap(), &["b"]);
    }

    #[test]
    fn desugar_fan_out_fan_in() {
        let mut manifest = Manifest::new(vec![
            Task::new("a", "do a"),
            Task::new("b1", "do b1"),
            Task::new("b2", "do b2"),
            Task::new("c", "do c"),
        ]);
        manifest.chains = Some(vec![vec![
            ChainStep::Single("a".into()),
            ChainStep::Group(vec!["b1".into(), "b2".into()]),
            ChainStep::Single("c".into()),
        ]]);

        manifest.desugar_chains();

        assert!(manifest.tasks[0].depends_on.is_none()); // a
        assert_eq!(manifest.tasks[1].depends_on.as_ref().unwrap(), &["a"]); // b1
        assert_eq!(manifest.tasks[2].depends_on.as_ref().unwrap(), &["a"]); // b2
        let c_deps = manifest.tasks[3].depends_on.as_ref().unwrap();
        assert!(c_deps.contains(&"b1".to_string()));
        assert!(c_deps.contains(&"b2".to_string()));
        assert_eq!(c_deps.len(), 2);
    }

    #[test]
    fn desugar_multi_chain_merge() {
        // Two chains that share task "d":
        // chain 1: a -> b -> d
        // chain 2: a -> c -> d
        // d should depend on both b and c
        let mut manifest = Manifest::new(vec![
            Task::new("a", "do a"),
            Task::new("b", "do b"),
            Task::new("c", "do c"),
            Task::new("d", "do d"),
        ]);
        manifest.chains = Some(vec![
            vec![
                ChainStep::Single("a".into()),
                ChainStep::Single("b".into()),
                ChainStep::Single("d".into()),
            ],
            vec![
                ChainStep::Single("a".into()),
                ChainStep::Single("c".into()),
                ChainStep::Single("d".into()),
            ],
        ]);

        manifest.desugar_chains();

        // a has no deps
        assert!(manifest.tasks[0].depends_on.is_none());
        // b depends on a
        assert_eq!(manifest.tasks[1].depends_on.as_ref().unwrap(), &["a"]);
        // c depends on a
        assert_eq!(manifest.tasks[2].depends_on.as_ref().unwrap(), &["a"]);
        // d depends on both b and c (merged)
        let d_deps = manifest.tasks[3].depends_on.as_ref().unwrap();
        assert!(d_deps.contains(&"b".to_string()));
        assert!(d_deps.contains(&"c".to_string()));
        assert_eq!(d_deps.len(), 2);
    }

    #[test]
    fn desugar_chains_merges_with_existing_depends_on() {
        let mut manifest = Manifest::new(vec![
            Task::new("a", "do a"),
            {
                let mut t = Task::new("b", "do b");
                t.depends_on = Some(vec!["a".into()]);
                t
            },
            Task::new("c", "do c"),
        ]);
        manifest.chains = Some(vec![vec![
            ChainStep::Single("b".into()),
            ChainStep::Single("c".into()),
        ]]);

        manifest.desugar_chains();

        // b keeps its original depends_on
        assert_eq!(manifest.tasks[1].depends_on.as_ref().unwrap(), &["a"]);
        // c gets depends_on from chain
        assert_eq!(manifest.tasks[2].depends_on.as_ref().unwrap(), &["b"]);
    }

    #[test]
    fn desugar_chains_no_duplicates() {
        let mut manifest = Manifest::new(vec![Task::new("a", "do a"), Task::new("b", "do b")]);
        // Same chain twice
        manifest.chains = Some(vec![
            vec![ChainStep::Single("a".into()), ChainStep::Single("b".into())],
            vec![ChainStep::Single("a".into()), ChainStep::Single("b".into())],
        ]);

        manifest.desugar_chains();

        // b should depend on a only once
        assert_eq!(manifest.tasks[1].depends_on.as_ref().unwrap(), &["a"]);
    }

    #[test]
    fn chains_json_roundtrip() {
        let json = r#"{
            "tasks": [
                {"name": "a", "prompt": "do a"},
                {"name": "b", "prompt": "do b"},
                {"name": "c", "prompt": "do c"}
            ],
            "chains": [["a", "b", "c"]]
        }"#;
        let mut manifest: Manifest = serde_json::from_str(json).unwrap();
        manifest.desugar_chains();
        assert_eq!(manifest.tasks[2].depends_on.as_ref().unwrap(), &["b"]);
    }

    #[test]
    fn chains_json_fan_out_roundtrip() {
        let json = r#"{
            "tasks": [
                {"name": "a", "prompt": "do a"},
                {"name": "b1", "prompt": "do b1"},
                {"name": "b2", "prompt": "do b2"},
                {"name": "c", "prompt": "do c"}
            ],
            "chains": [["a", ["b1", "b2"], "c"]]
        }"#;
        let mut manifest: Manifest = serde_json::from_str(json).unwrap();
        manifest.desugar_chains();
        assert_eq!(manifest.tasks[1].depends_on.as_ref().unwrap(), &["a"]);
        assert_eq!(manifest.tasks[2].depends_on.as_ref().unwrap(), &["a"]);
        let c_deps = manifest.tasks[3].depends_on.as_ref().unwrap();
        assert!(c_deps.contains(&"b1".to_string()));
        assert!(c_deps.contains(&"b2".to_string()));
    }

    #[test]
    fn task_condition_field_json() {
        let json = r#"{
            "name": "check-task",
            "prompt": "do the work",
            "condition": "test -f marker.txt"
        }"#;
        let task: Task = serde_json::from_str(json).unwrap();
        assert_eq!(task.condition.as_deref(), Some("test -f marker.txt"));
    }

    #[test]
    fn task_condition_field_toml() {
        let toml_str = r#"
            name = "check-task"
            prompt = "do the work"
            condition = "test -f marker.txt"
        "#;
        let task: Task = toml::from_str(toml_str).unwrap();
        assert_eq!(task.condition.as_deref(), Some("test -f marker.txt"));
    }

    #[test]
    fn task_condition_field_absent_defaults_to_none() {
        let json = r#"{"name": "t", "prompt": "p"}"#;
        let task: Task = serde_json::from_str(json).unwrap();
        assert!(task.condition.is_none());
    }

    #[test]
    fn task_condition_not_inherited_from_shared() {
        // condition is task-only: it must not appear on the Shared struct.
        // Verify that resolving a manifest preserves the task's condition.
        let json = r#"{
            "tasks": [
                {"name": "t", "prompt": "p", "condition": "exit 1"}
            ]
        }"#;
        let manifest: Manifest = serde_json::from_str(json).unwrap();
        let resolved = manifest.resolve();
        assert_eq!(resolved.tasks[0].condition.as_deref(), Some("exit 1"));
    }
}
