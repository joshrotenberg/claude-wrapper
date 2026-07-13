//! Shared spawn-time flag construction for
//! [`QueryCommand`](crate::QueryCommand) and
//! [`DuplexOptions`](crate::duplex::DuplexOptions).
//!
//! Both builders embed [`SharedSpawnArgs`] and delegate their setters
//! to it, so the flag mapping (names, value normalization, joining)
//! cannot drift between the oneshot and duplex paths.

use crate::tool_pattern::ToolPattern;
use crate::types::{Effort, HermeticScope, PermissionMode};

/// The spawn-time knobs common to `QueryCommand` and `DuplexOptions`.
///
/// Fields mirror the CLI flags one-to-one; [`Self::append_to`] owns
/// the flag emission. Knobs specific to one builder (output format,
/// stdin plumbing, subscriber capacity, ...) stay on that builder.
#[derive(Debug, Default, Clone)]
pub(crate) struct SharedSpawnArgs {
    pub(crate) model: Option<String>,
    pub(crate) system_prompt: Option<String>,
    pub(crate) append_system_prompt: Option<String>,
    pub(crate) max_budget_usd: Option<f64>,
    pub(crate) permission_mode: Option<PermissionMode>,
    pub(crate) allowed_tools: Vec<ToolPattern>,
    pub(crate) disallowed_tools: Vec<ToolPattern>,
    pub(crate) mcp_config: Vec<String>,
    pub(crate) add_dir: Vec<String>,
    pub(crate) effort: Option<Effort>,
    pub(crate) max_turns: Option<u32>,
    pub(crate) json_schema: Option<String>,
    pub(crate) continue_session: bool,
    pub(crate) resume: Option<String>,
    pub(crate) session_id: Option<String>,
    pub(crate) fallback_model: Option<String>,
    pub(crate) no_session_persistence: bool,
    pub(crate) dangerously_skip_permissions: bool,
    pub(crate) agent: Option<String>,
    pub(crate) agents_json: Option<String>,
    pub(crate) strict_mcp_config: bool,
    pub(crate) setting_sources: Option<String>,
    pub(crate) worktree: bool,
    pub(crate) worktree_name: Option<String>,
    pub(crate) tools: Vec<String>,
    pub(crate) file: Vec<String>,
    pub(crate) settings: Option<String>,
    pub(crate) fork_session: bool,
    pub(crate) debug_filter: Option<String>,
    pub(crate) debug_file: Option<String>,
    pub(crate) betas: Option<String>,
    pub(crate) plugin_dirs: Vec<String>,
    pub(crate) plugin_urls: Vec<String>,
    pub(crate) tmux: bool,
    pub(crate) bare: bool,
    pub(crate) safe_mode: bool,
    pub(crate) disable_slash_commands: bool,
    pub(crate) include_hook_events: bool,
    pub(crate) exclude_dynamic_system_prompt_sections: bool,
    pub(crate) name: Option<String>,
}

impl SharedSpawnArgs {
    /// Seal the ambient `~/.claude` promptspace for a reproducible run.
    ///
    /// Sets the three flags a hermetic run needs, in one place so the
    /// oneshot and duplex presets cannot drift:
    /// - `--setting-sources <scope>` (the ambient-config seal)
    /// - `--strict-mcp-config` (only servers from `--mcp-config`)
    /// - `--exclude-dynamic-system-prompt-sections` (drop cwd/env/git
    ///   status from the default system prompt)
    ///
    /// Deliberately does not touch `bare`: a hermetic seal must leave
    /// authentication alone, whereas `--bare` forces API-key billing.
    /// A later [`Self::setting_sources`](crate::QueryCommand::setting_sources)
    /// call (or its duplex peer) overrides the scope chosen here.
    pub(crate) fn apply_hermetic(&mut self, scope: HermeticScope) {
        self.setting_sources = Some(scope.setting_sources_value().to_string());
        self.strict_mcp_config = true;
        self.exclude_dynamic_system_prompt_sections = true;
    }

    /// Append the configured flags to `args`, in a stable order.
    pub(crate) fn append_to(&self, args: &mut Vec<String>) {
        if let Some(ref model) = self.model {
            args.push("--model".to_string());
            args.push(model.clone());
        }

        if let Some(ref prompt) = self.system_prompt {
            args.push("--system-prompt".to_string());
            args.push(prompt.clone());
        }

        if let Some(ref prompt) = self.append_system_prompt {
            args.push("--append-system-prompt".to_string());
            args.push(prompt.clone());
        }

        if let Some(budget) = self.max_budget_usd {
            args.push("--max-budget-usd".to_string());
            args.push(budget.to_string());
        }

        if let Some(ref mode) = self.permission_mode {
            args.push("--permission-mode".to_string());
            args.push(mode.as_arg().to_string());
        }

        if !self.allowed_tools.is_empty() {
            args.push("--allowed-tools".to_string());
            args.push(join_patterns(&self.allowed_tools));
        }

        if !self.disallowed_tools.is_empty() {
            args.push("--disallowed-tools".to_string());
            args.push(join_patterns(&self.disallowed_tools));
        }

        for config in &self.mcp_config {
            args.push("--mcp-config".to_string());
            args.push(config.clone());
        }

        for dir in &self.add_dir {
            args.push("--add-dir".to_string());
            args.push(dir.clone());
        }

        if let Some(ref effort) = self.effort {
            args.push("--effort".to_string());
            args.push(effort.as_arg().to_string());
        }

        if let Some(turns) = self.max_turns {
            args.push("--max-turns".to_string());
            args.push(turns.to_string());
        }

        if let Some(ref schema) = self.json_schema {
            args.push("--json-schema".to_string());
            args.push(schema.clone());
        }

        if self.continue_session {
            args.push("--continue".to_string());
        }

        if let Some(ref session_id) = self.resume {
            args.push("--resume".to_string());
            args.push(session_id.clone());
        }

        if let Some(ref id) = self.session_id {
            args.push("--session-id".to_string());
            args.push(id.clone());
        }

        if let Some(ref model) = self.fallback_model {
            args.push("--fallback-model".to_string());
            args.push(model.clone());
        }

        if self.no_session_persistence {
            args.push("--no-session-persistence".to_string());
        }

        if self.dangerously_skip_permissions {
            args.push("--dangerously-skip-permissions".to_string());
        }

        if let Some(ref agent) = self.agent {
            args.push("--agent".to_string());
            args.push(agent.clone());
        }

        if let Some(ref agents) = self.agents_json {
            args.push("--agents".to_string());
            args.push(agents.clone());
        }

        if self.strict_mcp_config {
            args.push("--strict-mcp-config".to_string());
        }

        if let Some(ref sources) = self.setting_sources {
            args.push("--setting-sources".to_string());
            args.push(sources.clone());
        }

        if self.worktree {
            args.push("--worktree".to_string());
            if let Some(ref name) = self.worktree_name {
                args.push(name.clone());
            }
        }

        if !self.tools.is_empty() {
            args.push("--tools".to_string());
            args.push(self.tools.join(","));
        }

        for spec in &self.file {
            args.push("--file".to_string());
            args.push(spec.clone());
        }

        if let Some(ref settings) = self.settings {
            args.push("--settings".to_string());
            args.push(settings.clone());
        }

        if self.fork_session {
            args.push("--fork-session".to_string());
        }

        if let Some(ref filter) = self.debug_filter {
            args.push("--debug".to_string());
            args.push(filter.clone());
        }

        if let Some(ref path) = self.debug_file {
            args.push("--debug-file".to_string());
            args.push(path.clone());
        }

        if let Some(ref betas) = self.betas {
            args.push("--betas".to_string());
            args.push(betas.clone());
        }

        for dir in &self.plugin_dirs {
            args.push("--plugin-dir".to_string());
            args.push(dir.clone());
        }

        for url in &self.plugin_urls {
            args.push("--plugin-url".to_string());
            args.push(url.clone());
        }

        if self.tmux {
            args.push("--tmux".to_string());
        }

        if self.bare {
            args.push("--bare".to_string());
        }

        if self.safe_mode {
            args.push("--safe-mode".to_string());
        }

        if self.disable_slash_commands {
            args.push("--disable-slash-commands".to_string());
        }

        if self.include_hook_events {
            args.push("--include-hook-events".to_string());
        }

        if self.exclude_dynamic_system_prompt_sections {
            args.push("--exclude-dynamic-system-prompt-sections".to_string());
        }

        if let Some(ref name) = self.name {
            args.push("--name".to_string());
            args.push(name.clone());
        }
    }
}

/// Join tool patterns into the comma-separated form the CLI's
/// `--allowed-tools` / `--disallowed-tools` flags expect.
pub(crate) fn join_patterns(patterns: &[ToolPattern]) -> String {
    let mut out = String::new();
    for (i, p) in patterns.iter().enumerate() {
        if i > 0 {
            out.push(',');
        }
        out.push_str(p.as_str());
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args_of(shared: SharedSpawnArgs) -> Vec<String> {
        let mut args = Vec::new();
        shared.append_to(&mut args);
        args
    }

    #[test]
    fn default_emits_nothing() {
        assert!(args_of(SharedSpawnArgs::default()).is_empty());
    }

    #[test]
    fn scalar_flags_carry_their_values() {
        let args = args_of(SharedSpawnArgs {
            max_turns: Some(3),
            max_budget_usd: Some(0.5),
            json_schema: Some(r#"{"type":"object"}"#.to_string()),
            fallback_model: Some("haiku".to_string()),
            session_id: Some("sid-1".to_string()),
            ..Default::default()
        });
        for pair in [
            ["--max-turns", "3"],
            ["--max-budget-usd", "0.5"],
            ["--json-schema", r#"{"type":"object"}"#],
            ["--fallback-model", "haiku"],
            ["--session-id", "sid-1"],
        ] {
            assert!(
                args.windows(2).any(|w| w[0] == pair[0] && w[1] == pair[1]),
                "expected {pair:?} in {args:?}"
            );
        }
    }

    #[test]
    fn repeatable_flags_emit_once_per_value() {
        let args = args_of(SharedSpawnArgs {
            mcp_config: vec!["a.json".to_string(), "b.json".to_string()],
            add_dir: vec!["/x".to_string()],
            ..Default::default()
        });
        assert_eq!(args.iter().filter(|a| *a == "--mcp-config").count(), 2);
        assert_eq!(args.iter().filter(|a| *a == "--add-dir").count(), 1);
    }

    #[test]
    fn setting_sources_carries_its_value() {
        let args = args_of(SharedSpawnArgs {
            setting_sources: Some("user,project".to_string()),
            ..Default::default()
        });
        assert!(
            args.windows(2)
                .any(|w| w[0] == "--setting-sources" && w[1] == "user,project"),
            "got {args:?}"
        );
    }

    #[test]
    fn empty_setting_sources_still_emits_flag() {
        // An empty value is meaningful: it loads no setting sources (a full seal).
        let args = args_of(SharedSpawnArgs {
            setting_sources: Some(String::new()),
            ..Default::default()
        });
        assert_eq!(args, vec!["--setting-sources".to_string(), String::new()]);
    }

    #[test]
    fn tool_patterns_join_comma_separated() {
        let args = args_of(SharedSpawnArgs {
            allowed_tools: vec!["Read".into(), "Bash(git:*)".into()],
            ..Default::default()
        });
        assert!(
            args.windows(2)
                .any(|w| w[0] == "--allowed-tools" && w[1] == "Read,Bash(git:*)"),
            "got {args:?}"
        );
    }

    #[test]
    fn bare_flags_emit_without_values() {
        let args = args_of(SharedSpawnArgs {
            continue_session: true,
            no_session_persistence: true,
            strict_mcp_config: true,
            ..Default::default()
        });
        assert_eq!(
            args,
            vec![
                "--continue".to_string(),
                "--no-session-persistence".to_string(),
                "--strict-mcp-config".to_string(),
            ]
        );
    }

    #[test]
    fn hermetic_full_seals_all_setting_sources() {
        let mut shared = SharedSpawnArgs::default();
        shared.apply_hermetic(HermeticScope::Full);
        let args = args_of(shared);
        // Empty --setting-sources value (full seal) plus the two guards.
        assert!(
            args.windows(2)
                .any(|w| w[0] == "--setting-sources" && w[1].is_empty()),
            "got {args:?}"
        );
        assert!(args.iter().any(|a| a == "--strict-mcp-config"));
        assert!(
            args.iter()
                .any(|a| a == "--exclude-dynamic-system-prompt-sections")
        );
        // A seal must never imply --bare (that would force API-key billing).
        assert!(!args.iter().any(|a| a == "--bare"));
    }

    #[test]
    fn hermetic_project_keeps_user_source() {
        let mut shared = SharedSpawnArgs::default();
        shared.apply_hermetic(HermeticScope::Project);
        let args = args_of(shared);
        assert!(
            args.windows(2).any(|w| w == ["--setting-sources", "user"]),
            "got {args:?}"
        );
        assert!(args.iter().any(|a| a == "--strict-mcp-config"));
        assert!(
            args.iter()
                .any(|a| a == "--exclude-dynamic-system-prompt-sections")
        );
    }

    #[test]
    fn hermetic_default_scope_is_full() {
        assert_eq!(HermeticScope::default(), HermeticScope::Full);
    }

    #[test]
    fn worktree_name_follows_flag() {
        let args = args_of(SharedSpawnArgs {
            worktree: true,
            worktree_name: Some("wt1".to_string()),
            ..Default::default()
        });
        assert_eq!(args, vec!["--worktree".to_string(), "wt1".to_string()]);
    }
}
