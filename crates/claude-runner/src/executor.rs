//! Executor — agent-agnostic task execution.
//!
//! Translates planned stages into agent CLI invocations. The executor
//! doesn't know how to talk to any specific agent — it delegates to
//! an agent adapter that maps stage requests to concrete commands.

use serde::{Deserialize, Serialize};

use crate::planner::PlannedStage;

/// Result of executing a single stage.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StageResult {
    /// Stage kind name.
    pub stage: String,
    /// Whether the stage succeeded.
    pub success: bool,
    /// Duration in seconds.
    pub duration_secs: f64,
    /// Cost in USD (if tracked by the agent).
    pub cost_usd: Option<f64>,
    /// Stage output (summary or error).
    pub output: String,
    /// Files modified during this stage.
    pub files_modified: Option<u32>,
}

/// Agent adapter trait — implement for each supported agent.
pub trait AgentAdapter: Send + Sync {
    /// Execute a stage and return the result.
    fn execute_stage(
        &self,
        stage: &PlannedStage,
        work_dir: &std::path::Path,
    ) -> impl std::future::Future<Output = crate::error::Result<StageResult>> + Send;
}
