//! Executor — agent-agnostic task execution.
//!
//! The `AgentAdapter` trait abstracts over different coding agents.
//! `ClaudeAdapter` implements it using claude-wrapper.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use tracing::info;

use crate::error::{Error, Result};
use crate::planner::PlannedStage;
use crate::workflow::StageKind;

/// Result of executing a single stage.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StageResult {
    pub stage: String,
    pub success: bool,
    pub duration_secs: f64,
    pub cost_usd: Option<f64>,
    pub output: String,
    pub files_modified: Option<u32>,
}

/// Agent adapter trait — implement for each supported agent.
pub trait AgentAdapter: Send + Sync {
    /// Execute a stage and return the result.
    fn execute_stage(
        &self,
        stage: &PlannedStage,
        work_dir: &Path,
    ) -> impl std::future::Future<Output = Result<StageResult>> + Send;
}

/// Claude Code adapter using claude-wrapper.
pub struct ClaudeAdapter {
    binary: Option<PathBuf>,
    model: Option<String>,
    max_turns: Option<u32>,
}

impl ClaudeAdapter {
    pub fn new() -> Self {
        Self {
            binary: None,
            model: None,
            max_turns: None,
        }
    }

    pub fn model(mut self, model: impl Into<String>) -> Self {
        self.model = Some(model.into());
        self
    }

    pub fn max_turns(mut self, turns: u32) -> Self {
        self.max_turns = Some(turns);
        self
    }

    pub fn binary(mut self, path: impl Into<PathBuf>) -> Self {
        self.binary = Some(path.into());
        self
    }
}

impl Default for ClaudeAdapter {
    fn default() -> Self {
        Self::new()
    }
}

impl AgentAdapter for ClaudeAdapter {
    async fn execute_stage(&self, stage: &PlannedStage, work_dir: &Path) -> Result<StageResult> {
        use claude_wrapper::{Claude, ClaudeCommand, OutputFormat, PermissionMode, QueryCommand};

        let start = std::time::Instant::now();

        let mut builder = Claude::builder().working_dir(work_dir);
        if let Some(ref binary) = self.binary {
            builder = builder.binary(binary);
        }
        let claude = builder
            .build()
            .map_err(|e| Error::Executor(format!("failed to create claude client: {e}")))?;

        let mut cmd = QueryCommand::new(&stage.prompt)
            .output_format(OutputFormat::Json)
            .permission_mode(PermissionMode::BypassPermissions)
            .no_session_persistence();

        if let Some(ref model) = self.model {
            cmd = cmd.model(model);
        }
        if let Some(turns) = self.max_turns {
            cmd = cmd.max_turns(turns);
        }

        // Scope tools based on stage kind.
        match stage.kind {
            StageKind::Plan | StageKind::Research => {
                cmd = cmd.allowed_tools(["Read", "Glob", "Grep", "Write", "WebSearch", "WebFetch"]);
            }
            StageKind::Implement => {
                cmd = cmd.allowed_tools([
                    "Read",
                    "Edit",
                    "Write",
                    "Glob",
                    "Grep",
                    "Bash(cargo *)",
                    "Bash(npm *)",
                    "Bash(git *)",
                ]);
            }
            StageKind::Test => {
                cmd = cmd.allowed_tools([
                    "Read",
                    "Edit",
                    "Glob",
                    "Grep",
                    "Bash(cargo *)",
                    "Bash(npm *)",
                    "Bash(make *)",
                ]);
            }
            StageKind::Review => {
                cmd = cmd.allowed_tools(["Read", "Glob", "Grep"]);
                cmd = cmd.disallowed_tools(["Edit", "Write"]);
            }
            StageKind::Comment | StageKind::Clarify => {
                cmd = cmd.allowed_tools(["Read", "Glob", "Grep", "Bash(gh *)"]);
            }
            _ => {}
        }

        info!(
            stage = stage.kind_name(),
            work_dir = %work_dir.display(),
            "executing stage"
        );

        let output = cmd
            .execute(&claude)
            .await
            .map_err(|e| Error::Executor(format!("claude execution failed: {e}")))?;

        let duration = start.elapsed();

        // Parse cost from JSON output if available.
        let cost_usd = serde_json::from_str::<serde_json::Value>(&output.stdout)
            .ok()
            .and_then(|v| {
                v.get("total_cost_usd")
                    .or_else(|| v.get("cost_usd"))
                    .and_then(|c| c.as_f64())
            });

        Ok(StageResult {
            stage: stage.kind_name().to_string(),
            success: output.success,
            duration_secs: duration.as_secs_f64(),
            cost_usd,
            output: if output.success {
                output.stdout
            } else {
                output.stderr
            },
            files_modified: None,
        })
    }
}
