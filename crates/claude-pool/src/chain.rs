//! Chain execution — sequential pipelines of tasks.
//!
//! A chain runs steps in order, feeding each step's output as context
//! to the next. Steps can reference skills or use inline prompts.
//!
//! Chains can be run synchronously via [`execute_chain`] or submitted
//! for async execution via [`Pool::submit_chain`](crate::Pool::submit_chain).

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::pool::Pool;
use crate::skill::SkillRegistry;
use crate::store::PoolStore;
use crate::types::{TaskId, WorkerConfig};

/// A step in a chain pipeline.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChainStep {
    /// Step name (for logging and result tracking).
    pub name: String,

    /// Either an inline prompt or a skill reference.
    pub action: StepAction,

    /// Per-step config overrides (model, effort, etc.).
    pub config: Option<WorkerConfig>,

    /// Failure policy for this step.
    #[serde(default)]
    pub failure_policy: StepFailurePolicy,
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

/// Per-step failure handling policy.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct StepFailurePolicy {
    /// Number of retries before giving up or recovering (default: 0).
    #[serde(default)]
    pub retries: u32,
    /// If set, run this prompt on failure instead of failing the chain.
    /// `{error}` is replaced with the error message, `{previous_output}`
    /// with the last successful step's output.
    pub recovery_prompt: Option<String>,
}

/// Options for chain execution.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ChainOptions {
    /// Tags for the chain task (used when submitted async).
    #[serde(default)]
    pub tags: Vec<String>,
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
    /// Number of retries used.
    #[serde(default)]
    pub retries_used: u32,
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

/// Progress of an in-flight chain.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChainProgress {
    /// Total number of steps.
    pub total_steps: usize,
    /// Index of the currently running step (0-based), or None if done.
    pub current_step: Option<usize>,
    /// Name of the currently running step.
    pub current_step_name: Option<String>,
    /// Completed step results so far.
    pub completed_steps: Vec<StepResult>,
    /// Overall status.
    pub status: ChainStatus,
}

/// Status of a chain.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChainStatus {
    /// Chain is running.
    Running,
    /// All steps completed successfully.
    Completed,
    /// A step failed and the chain stopped.
    Failed,
}

/// Execute a chain of steps against the pool.
pub async fn execute_chain<S: PoolStore + 'static>(
    pool: &Pool<S>,
    skills: &SkillRegistry,
    steps: &[ChainStep],
) -> crate::Result<ChainResult> {
    execute_chain_with_progress(pool, skills, steps, None).await
}

/// Execute a chain with optional progress tracking.
///
/// If `chain_task_id` is provided, intermediate progress is stored so callers
/// can poll for status.
pub async fn execute_chain_with_progress<S: PoolStore + 'static>(
    pool: &Pool<S>,
    skills: &SkillRegistry,
    steps: &[ChainStep],
    chain_task_id: Option<&TaskId>,
) -> crate::Result<ChainResult> {
    let mut step_results = Vec::with_capacity(steps.len());
    let mut previous_output = String::new();
    let mut total_cost = 0u64;

    for (step_idx, step) in steps.iter().enumerate() {
        // Update progress in the store if we have a task ID.
        if let Some(task_id) = chain_task_id {
            let progress = ChainProgress {
                total_steps: steps.len(),
                current_step: Some(step_idx),
                current_step_name: Some(step.name.clone()),
                completed_steps: step_results.clone(),
                status: ChainStatus::Running,
            };
            pool.set_chain_progress(task_id, progress).await;
        }

        let prompt = render_step_prompt(step, &previous_output, skills)?;

        let (step_result, step_cost) =
            execute_step_with_retries(pool, step, &prompt, &previous_output, skills).await;

        total_cost += step_cost;

        match step_result {
            Ok(result) => {
                previous_output = result.output.clone();
                step_results.push(result);

                if !step_results.last().unwrap().success {
                    update_chain_progress_final(
                        pool,
                        chain_task_id,
                        steps.len(),
                        &step_results,
                        ChainStatus::Failed,
                    )
                    .await;
                    return Ok(ChainResult {
                        final_output: previous_output,
                        steps: step_results,
                        total_cost_microdollars: total_cost,
                        success: false,
                    });
                }
            }
            Err(output) => {
                step_results.push(StepResult {
                    name: step.name.clone(),
                    output: output.clone(),
                    success: false,
                    cost_microdollars: 0,
                    retries_used: step.failure_policy.retries,
                });
                update_chain_progress_final(
                    pool,
                    chain_task_id,
                    steps.len(),
                    &step_results,
                    ChainStatus::Failed,
                )
                .await;
                return Ok(ChainResult {
                    final_output: output,
                    steps: step_results,
                    total_cost_microdollars: total_cost,
                    success: false,
                });
            }
        }
    }

    update_chain_progress_final(
        pool,
        chain_task_id,
        steps.len(),
        &step_results,
        ChainStatus::Completed,
    )
    .await;

    Ok(ChainResult {
        final_output: previous_output,
        steps: step_results,
        total_cost_microdollars: total_cost,
        success: true,
    })
}

/// Render the prompt for a step, substituting `{previous_output}`.
fn render_step_prompt(
    step: &ChainStep,
    previous_output: &str,
    skills: &SkillRegistry,
) -> crate::Result<String> {
    match &step.action {
        StepAction::Prompt { prompt } => Ok(prompt.replace("{previous_output}", previous_output)),
        StepAction::Skill { skill, arguments } => {
            let skill_def = skills
                .get(skill)
                .ok_or_else(|| crate::Error::Store(format!("skill not found: {skill}")))?;
            let mut args = arguments.clone();
            if !previous_output.is_empty() {
                args.entry("_previous_output".into())
                    .or_insert(previous_output.to_string());
            }
            skill_def.render(&args)
        }
    }
}

/// Execute a step with retry and recovery support.
///
/// Returns `Ok(StepResult)` on success (or successful recovery), or
/// `Err(error_message)` if all retries and recovery are exhausted.
async fn execute_step_with_retries<S: PoolStore + 'static>(
    pool: &Pool<S>,
    step: &ChainStep,
    initial_prompt: &str,
    previous_output: &str,
    skills: &SkillRegistry,
) -> (std::result::Result<StepResult, String>, u64) {
    let max_attempts = 1 + step.failure_policy.retries;
    let mut total_cost = 0u64;
    let mut last_error = String::new();

    for attempt in 0..max_attempts {
        let prompt = if attempt == 0 {
            initial_prompt.to_string()
        } else {
            // Re-render the prompt for retries (same prompt, fresh attempt).
            match render_step_prompt(step, previous_output, skills) {
                Ok(p) => p,
                Err(e) => return (Err(e.to_string()), total_cost),
            }
        };

        match pool.run_with_config(&prompt, step.config.clone()).await {
            Ok(task_result) => {
                total_cost += task_result.cost_microdollars;
                if task_result.success {
                    return (
                        Ok(StepResult {
                            name: step.name.clone(),
                            output: task_result.output,
                            success: true,
                            cost_microdollars: total_cost,
                            retries_used: attempt,
                        }),
                        total_cost,
                    );
                }
                // Task ran but reported failure.
                last_error = task_result.output;
            }
            Err(e) => {
                last_error = e.to_string();
            }
        }

        tracing::warn!(
            step = %step.name,
            attempt = attempt + 1,
            max_attempts,
            "chain step failed, will retry"
        );
    }

    // All retries exhausted. Try recovery prompt if configured.
    if let Some(ref recovery_template) = step.failure_policy.recovery_prompt {
        let recovery_prompt = recovery_template
            .replace("{error}", &last_error)
            .replace("{previous_output}", previous_output);

        tracing::info!(step = %step.name, "attempting recovery prompt");

        match pool
            .run_with_config(&recovery_prompt, step.config.clone())
            .await
        {
            Ok(task_result) => {
                total_cost += task_result.cost_microdollars;
                return (
                    Ok(StepResult {
                        name: step.name.clone(),
                        output: task_result.output,
                        success: task_result.success,
                        cost_microdollars: total_cost,
                        retries_used: max_attempts,
                    }),
                    total_cost,
                );
            }
            Err(e) => {
                last_error = e.to_string();
            }
        }
    }

    (Err(last_error), total_cost)
}

/// Update chain progress to a terminal state.
async fn update_chain_progress_final<S: PoolStore + 'static>(
    pool: &Pool<S>,
    chain_task_id: Option<&TaskId>,
    total_steps: usize,
    completed_steps: &[StepResult],
    status: ChainStatus,
) {
    if let Some(task_id) = chain_task_id {
        let progress = ChainProgress {
            total_steps,
            current_step: None,
            current_step_name: None,
            completed_steps: completed_steps.to_vec(),
            status,
        };
        pool.set_chain_progress(task_id, progress).await;
    }
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
            failure_policy: StepFailurePolicy::default(),
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
                retries_used: 0,
            }],
            final_output: "done".into(),
            total_cost_microdollars: 1000,
            success: true,
        };

        let json = serde_json::to_string(&result).unwrap();
        assert!(json.contains("step1"));
    }

    #[test]
    fn step_failure_policy_defaults() {
        let policy = StepFailurePolicy::default();
        assert_eq!(policy.retries, 0);
        assert!(policy.recovery_prompt.is_none());
    }

    #[test]
    fn chain_options_defaults() {
        let opts = ChainOptions::default();
        assert!(opts.tags.is_empty());
    }

    #[test]
    fn chain_progress_serializes() {
        let progress = ChainProgress {
            total_steps: 3,
            current_step: Some(1),
            current_step_name: Some("implement".into()),
            completed_steps: vec![StepResult {
                name: "plan".into(),
                output: "planned".into(),
                success: true,
                cost_microdollars: 500,
                retries_used: 0,
            }],
            status: ChainStatus::Running,
        };

        let json = serde_json::to_string(&progress).unwrap();
        assert!(json.contains("implement"));
        assert!(json.contains("running"));
    }
}
