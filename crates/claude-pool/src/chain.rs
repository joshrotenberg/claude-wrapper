//! Chain execution — sequential pipelines of tasks.
//!
//! A chain runs steps in order, feeding each step's output as context
//! to the next. Steps can reference skills or use inline prompts.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::pool::Pool;
use crate::skill::SkillRegistry;
use crate::store::PoolStore;
use crate::types::WorkerConfig;

/// A step in a chain pipeline.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChainStep {
    /// Step name (for logging and result tracking).
    pub name: String,

    /// Either an inline prompt or a skill reference.
    pub action: StepAction,

    /// Per-step config overrides (model, effort, etc.).
    pub config: Option<WorkerConfig>,
}

/// What a chain step does.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum StepAction {
    /// Run an inline prompt. `{previous_output}` is replaced with
    /// the output from the prior step.
    Prompt {
        /// The prompt template.
        prompt: String,
    },
    /// Run a registered skill with the given arguments.
    /// The special argument `_previous_output` is automatically set
    /// to the output from the prior step.
    Skill {
        /// Skill name.
        skill: String,
        /// Skill arguments.
        #[serde(default)]
        arguments: HashMap<String, String>,
    },
}

/// Result of a single chain step.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StepResult {
    /// Step name.
    pub name: String,
    /// Output text from this step.
    pub output: String,
    /// Whether the step succeeded.
    pub success: bool,
    /// Cost in microdollars.
    pub cost_microdollars: u64,
}

/// Result of a full chain execution.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChainResult {
    /// Per-step results in execution order.
    pub steps: Vec<StepResult>,
    /// Final output (from the last step).
    pub final_output: String,
    /// Total cost across all steps.
    pub total_cost_microdollars: u64,
    /// Whether all steps succeeded.
    pub success: bool,
}

/// Execute a chain of steps against the pool.
pub async fn execute_chain<S: PoolStore + 'static>(
    pool: &Pool<S>,
    skills: &SkillRegistry,
    steps: &[ChainStep],
) -> crate::Result<ChainResult> {
    let mut step_results = Vec::with_capacity(steps.len());
    let mut previous_output = String::new();
    let mut total_cost = 0u64;

    for step in steps {
        let prompt = match &step.action {
            StepAction::Prompt { prompt } => prompt.replace("{previous_output}", &previous_output),
            StepAction::Skill { skill, arguments } => {
                let skill_def = skills
                    .get(skill)
                    .ok_or_else(|| crate::Error::Store(format!("skill not found: {skill}")))?;
                let mut args = arguments.clone();
                if !previous_output.is_empty() {
                    args.entry("_previous_output".into())
                        .or_insert(previous_output.clone());
                }
                skill_def.render(&args)?
            }
        };

        let result = pool.run_with_config(&prompt, step.config.clone()).await;

        match result {
            Ok(task_result) => {
                total_cost += task_result.cost_microdollars;
                previous_output = task_result.output.clone();
                step_results.push(StepResult {
                    name: step.name.clone(),
                    output: task_result.output,
                    success: task_result.success,
                    cost_microdollars: task_result.cost_microdollars,
                });

                if !task_result.success {
                    return Ok(ChainResult {
                        final_output: previous_output,
                        steps: step_results,
                        total_cost_microdollars: total_cost,
                        success: false,
                    });
                }
            }
            Err(e) => {
                step_results.push(StepResult {
                    name: step.name.clone(),
                    output: e.to_string(),
                    success: false,
                    cost_microdollars: 0,
                });
                return Ok(ChainResult {
                    final_output: e.to_string(),
                    steps: step_results,
                    total_cost_microdollars: total_cost,
                    success: false,
                });
            }
        }
    }

    Ok(ChainResult {
        final_output: previous_output,
        steps: step_results,
        total_cost_microdollars: total_cost,
        success: true,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prompt_step_replaces_previous_output() {
        let step = ChainStep {
            name: "step1".into(),
            action: StepAction::Prompt {
                prompt: "Based on: {previous_output}\nDo more.".into(),
            },
            config: None,
        };

        if let StepAction::Prompt { prompt } = &step.action {
            let rendered = prompt.replace("{previous_output}", "hello world");
            assert_eq!(rendered, "Based on: hello world\nDo more.");
        }
    }

    #[test]
    fn chain_result_serializes() {
        let result = ChainResult {
            steps: vec![StepResult {
                name: "step1".into(),
                output: "done".into(),
                success: true,
                cost_microdollars: 1000,
            }],
            final_output: "done".into(),
            total_cost_microdollars: 1000,
            success: true,
        };

        let json = serde_json::to_string(&result).unwrap();
        assert!(json.contains("step1"));
    }
}
