//! Configuration resolution for workers and tasks.
//!
//! Configuration cascades in three layers:
//! 1. [`GlobalWorkerConfig`] — pool-wide defaults
//! 2. [`WorkerConfig`] — per-worker overrides
//! 3. [`WorkerConfig`] on a task record — per-task overrides

use claude_wrapper::types::{Effort, PermissionMode};

use crate::types::{GlobalWorkerConfig, WorkerConfig};

/// Resolved configuration for a single task execution.
///
/// Produced by merging global -> worker -> task config layers.
#[derive(Debug, Clone)]
pub struct ResolvedConfig {
    pub model: Option<String>,
    pub permission_mode: PermissionMode,
    pub max_turns: Option<u32>,
    pub system_prompt: Option<String>,
    pub allowed_tools: Vec<String>,
    pub effort: Option<Effort>,
}

impl ResolvedConfig {
    /// Resolve configuration by layering global, worker, and task configs.
    ///
    /// Later layers override earlier layers for scalar fields.
    /// List fields (allowed_tools) are merged.
    pub fn resolve(
        global: &GlobalWorkerConfig,
        worker: &WorkerConfig,
        task: Option<&WorkerConfig>,
    ) -> Self {
        let model = task
            .and_then(|t| t.model.clone())
            .or_else(|| worker.model.clone())
            .or_else(|| global.model.clone());

        let permission_mode = task
            .and_then(|t| t.permission_mode)
            .or(worker.permission_mode)
            .or(global.permission_mode)
            .unwrap_or(PermissionMode::Plan);

        let max_turns = task
            .and_then(|t| t.max_turns)
            .or(worker.max_turns)
            .or(global.max_turns);

        let system_prompt = task
            .and_then(|t| t.system_prompt.clone())
            .or_else(|| worker.system_prompt.clone())
            .or_else(|| global.system_prompt.clone());

        let effort = task
            .and_then(|t| t.effort)
            .or(worker.effort)
            .or(global.effort);

        // Merge allowed tools: global + worker + task
        let mut allowed_tools = global.allowed_tools.clone();
        if let Some(ref wt) = worker.allowed_tools {
            allowed_tools.extend(wt.iter().cloned());
        }
        if let Some(task_cfg) = task
            && let Some(ref tt) = task_cfg.allowed_tools
        {
            allowed_tools.extend(tt.iter().cloned());
        }

        Self {
            model,
            permission_mode,
            max_turns,
            system_prompt,
            allowed_tools,
            effort,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn global_defaults() {
        let global = GlobalWorkerConfig::default();
        let worker = WorkerConfig::default();
        let resolved = ResolvedConfig::resolve(&global, &worker, None);

        // GlobalWorkerConfig defaults to Plan mode.
        assert_eq!(resolved.permission_mode, PermissionMode::Plan);
        assert!(resolved.model.is_none());
        assert!(resolved.allowed_tools.is_empty());
    }

    #[test]
    fn worker_overrides_global() {
        let global = GlobalWorkerConfig {
            model: Some("haiku".into()),
            effort: Some(Effort::Low),
            ..Default::default()
        };
        let worker = WorkerConfig {
            model: Some("opus".into()),
            ..Default::default()
        };
        let resolved = ResolvedConfig::resolve(&global, &worker, None);

        assert_eq!(resolved.model.as_deref(), Some("opus"));
        assert_eq!(resolved.effort, Some(Effort::Low)); // inherited from global
    }

    #[test]
    fn task_overrides_worker() {
        let global = GlobalWorkerConfig {
            model: Some("haiku".into()),
            ..Default::default()
        };
        let worker = WorkerConfig {
            model: Some("sonnet".into()),
            effort: Some(Effort::Medium),
            ..Default::default()
        };
        let task = WorkerConfig {
            effort: Some(Effort::Max),
            ..Default::default()
        };
        let resolved = ResolvedConfig::resolve(&global, &worker, Some(&task));

        assert_eq!(resolved.model.as_deref(), Some("sonnet")); // worker wins over global
        assert_eq!(resolved.effort, Some(Effort::Max)); // task wins over worker
    }

    #[test]
    fn allowed_tools_merge() {
        let global = GlobalWorkerConfig {
            allowed_tools: vec!["Bash".into(), "Read".into()],
            ..Default::default()
        };
        let worker = WorkerConfig {
            allowed_tools: Some(vec!["Write".into()]),
            ..Default::default()
        };
        let task = WorkerConfig {
            allowed_tools: Some(vec!["Edit".into()]),
            ..Default::default()
        };
        let resolved = ResolvedConfig::resolve(&global, &worker, Some(&task));

        assert_eq!(
            resolved.allowed_tools,
            vec!["Bash", "Read", "Write", "Edit"]
        );
    }
}
