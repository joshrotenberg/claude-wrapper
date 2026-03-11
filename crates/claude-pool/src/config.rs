//! Configuration resolution for slots and tasks.
//!
//! Configuration cascades in three layers:
//! 1. [`PoolConfig`] — pool-wide defaults
//! 2. [`SlotConfig`] — per-slot overrides
//! 3. [`SlotConfig`] on a task record — per-task overrides

use std::collections::HashMap;

use claude_wrapper::types::{Effort, PermissionMode};

use crate::types::{PoolConfig, SlotConfig};

/// Resolved configuration for a single task execution.
///
/// Produced by merging global -> slot -> task config layers.
#[derive(Debug, Clone)]
pub struct ResolvedConfig {
    pub model: Option<String>,
    pub permission_mode: PermissionMode,
    pub max_turns: Option<u32>,
    pub system_prompt: Option<String>,
    pub allowed_tools: Vec<String>,
    pub effort: Option<Effort>,
    /// Merged MCP servers from global + slot + task layers.
    /// Later layers override earlier layers by server name.
    pub mcp_servers: HashMap<String, serde_json::Value>,
    /// Whether to pass `--strict-mcp-config` to the CLI.
    pub strict_mcp_config: bool,
}

impl ResolvedConfig {
    /// Resolve configuration by layering global, slot, and task configs.
    ///
    /// Later layers override earlier layers for scalar fields.
    /// Map/list fields (allowed_tools, mcp_servers) are merged additively,
    /// with later layers overriding same-named entries.
    pub fn resolve(global: &PoolConfig, slot: &SlotConfig, task: Option<&SlotConfig>) -> Self {
        let model = task
            .and_then(|t| t.model.clone())
            .or_else(|| slot.model.clone())
            .or_else(|| global.model.clone());

        let permission_mode = task
            .and_then(|t| t.permission_mode)
            .or(slot.permission_mode)
            .or(global.permission_mode)
            .unwrap_or(PermissionMode::Plan);

        let max_turns = task
            .and_then(|t| t.max_turns)
            .or(slot.max_turns)
            .or(global.max_turns);

        let system_prompt = task
            .and_then(|t| t.system_prompt.clone())
            .or_else(|| slot.system_prompt.clone())
            .or_else(|| global.system_prompt.clone());

        let effort = task
            .and_then(|t| t.effort)
            .or(slot.effort)
            .or(global.effort);

        // Merge allowed tools: global + slot + task
        let mut allowed_tools = global.allowed_tools.clone();
        if let Some(ref wt) = slot.allowed_tools {
            allowed_tools.extend(wt.iter().cloned());
        }
        if let Some(task_cfg) = task
            && let Some(ref tt) = task_cfg.allowed_tools
        {
            allowed_tools.extend(tt.iter().cloned());
        }

        // Merge MCP servers: global base, slot overrides/adds, task overrides/adds.
        // Same-named servers in later layers replace earlier ones.
        let mut mcp_servers = global.mcp_servers.clone();
        if let Some(ref slot_servers) = slot.mcp_servers {
            mcp_servers.extend(slot_servers.iter().map(|(k, v)| (k.clone(), v.clone())));
        }
        if let Some(task_cfg) = task
            && let Some(ref task_servers) = task_cfg.mcp_servers
        {
            mcp_servers.extend(task_servers.iter().map(|(k, v)| (k.clone(), v.clone())));
        }

        let strict_mcp_config = global.strict_mcp_config;

        Self {
            model,
            permission_mode,
            max_turns,
            system_prompt,
            allowed_tools,
            effort,
            mcp_servers,
            strict_mcp_config,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn global_defaults() {
        let global = PoolConfig::default();
        let slot = SlotConfig::default();
        let resolved = ResolvedConfig::resolve(&global, &slot, None);

        // PoolConfig defaults to Plan mode.
        assert_eq!(resolved.permission_mode, PermissionMode::Plan);
        assert!(resolved.model.is_none());
        assert!(resolved.allowed_tools.is_empty());
        assert!(resolved.mcp_servers.is_empty());
        assert!(resolved.strict_mcp_config);
    }

    #[test]
    fn slot_overrides_global() {
        let global = PoolConfig {
            model: Some("haiku".into()),
            effort: Some(Effort::Low),
            ..Default::default()
        };
        let slot = SlotConfig {
            model: Some("opus".into()),
            ..Default::default()
        };
        let resolved = ResolvedConfig::resolve(&global, &slot, None);

        assert_eq!(resolved.model.as_deref(), Some("opus"));
        assert_eq!(resolved.effort, Some(Effort::Low)); // inherited from global
    }

    #[test]
    fn task_overrides_slot() {
        let global = PoolConfig {
            model: Some("haiku".into()),
            ..Default::default()
        };
        let slot = SlotConfig {
            model: Some("sonnet".into()),
            effort: Some(Effort::Medium),
            ..Default::default()
        };
        let task = SlotConfig {
            effort: Some(Effort::Max),
            ..Default::default()
        };
        let resolved = ResolvedConfig::resolve(&global, &slot, Some(&task));

        assert_eq!(resolved.model.as_deref(), Some("sonnet")); // slot wins over global
        assert_eq!(resolved.effort, Some(Effort::Max)); // task wins over slot
    }

    #[test]
    fn allowed_tools_merge() {
        let global = PoolConfig {
            allowed_tools: vec!["Bash".into(), "Read".into()],
            ..Default::default()
        };
        let slot = SlotConfig {
            allowed_tools: Some(vec!["Write".into()]),
            ..Default::default()
        };
        let task = SlotConfig {
            allowed_tools: Some(vec!["Edit".into()]),
            ..Default::default()
        };
        let resolved = ResolvedConfig::resolve(&global, &slot, Some(&task));

        assert_eq!(
            resolved.allowed_tools,
            vec!["Bash", "Read", "Write", "Edit"]
        );
    }

    #[test]
    fn mcp_servers_merge() {
        let global = PoolConfig {
            mcp_servers: [(
                "hub".to_string(),
                serde_json::json!({"type": "http", "url": "http://global/"}),
            )]
            .into_iter()
            .collect(),
            ..Default::default()
        };
        let slot = SlotConfig {
            mcp_servers: Some(
                [(
                    "tool".to_string(),
                    serde_json::json!({"type": "stdio", "command": "npx"}),
                )]
                .into_iter()
                .collect(),
            ),
            ..Default::default()
        };
        let resolved = ResolvedConfig::resolve(&global, &slot, None);

        assert_eq!(resolved.mcp_servers.len(), 2);
        assert!(resolved.mcp_servers.contains_key("hub"));
        assert!(resolved.mcp_servers.contains_key("tool"));
    }

    #[test]
    fn task_mcp_servers_override_slot() {
        let global = PoolConfig::default();
        let slot = SlotConfig {
            mcp_servers: Some(
                [(
                    "hub".to_string(),
                    serde_json::json!({"type": "http", "url": "http://slot/"}),
                )]
                .into_iter()
                .collect(),
            ),
            ..Default::default()
        };
        let task = SlotConfig {
            mcp_servers: Some(
                [(
                    "hub".to_string(),
                    serde_json::json!({"type": "http", "url": "http://task/"}),
                )]
                .into_iter()
                .collect(),
            ),
            ..Default::default()
        };
        let resolved = ResolvedConfig::resolve(&global, &slot, Some(&task));

        assert_eq!(resolved.mcp_servers["hub"]["url"], "http://task/");
    }

    #[test]
    fn strict_mcp_config_defaults_true() {
        let global = PoolConfig::default();
        let slot = SlotConfig::default();
        let resolved = ResolvedConfig::resolve(&global, &slot, None);
        assert!(resolved.strict_mcp_config);
    }

    #[test]
    fn strict_mcp_config_can_be_disabled() {
        let global = PoolConfig {
            strict_mcp_config: false,
            ..Default::default()
        };
        let slot = SlotConfig::default();
        let resolved = ResolvedConfig::resolve(&global, &slot, None);
        assert!(!resolved.strict_mcp_config);
    }
}
