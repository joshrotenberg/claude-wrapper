//! Chain execution — sequential pipelines of tasks.
//!
//! A chain runs steps in order, feeding each step's output as context
//! to the next. Steps can reference skills or use inline prompts.
//!
//! Chains can be run synchronously via [`execute_chain`] or submitted
//! for async execution via [`Pool::submit_chain`](crate::Pool::submit_chain).

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::pool::Pool;
use crate::skill::SkillRegistry;
use crate::store::PoolStore;
use crate::types::{TaskId, TaskOverrides, TaskState};

/// A step in a chain pipeline.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChainStep {
    /// Step name (for logging and result tracking).
    pub name: String,

    /// Either an inline prompt or a skill reference.
    pub action: StepAction,

    /// Per-step config overrides (model, effort, etc.).
    pub config: Option<TaskOverrides>,

    /// Failure policy for this step.
    #[serde(default)]
    pub failure_policy: StepFailurePolicy,

    /// Extract named values from this step's JSON output for use in later steps.
    ///
    /// Key = variable name, Value = dot-path into the JSON output.
    /// Use `"."` or `""` for the whole output. Use `"key"` for a top-level field.
    /// Use `"a.b.c"` for nested access. String values are returned as-is; other
    /// JSON types are serialized to their JSON representation.
    ///
    /// Extracted values are available in subsequent step prompts as
    /// `{steps.STEP_NAME.VAR_NAME}`.
    #[serde(default)]
    pub output_vars: HashMap<String, String>,
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

/// Isolation mode for a chain execution.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChainIsolation {
    /// Use the slot's working directory (no isolation).
    None,
    /// Create a temporary git worktree shared by all steps in the chain (default).
    #[default]
    Worktree,
    /// Create a full clone with `git clone --local --shared` for complete isolation.
    Clone,
}

/// Options for chain execution.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ChainOptions {
    /// Tags for the chain task (used when submitted async).
    #[serde(default)]
    pub tags: Vec<String>,
    /// Isolation mode for this chain.
    #[serde(default)]
    pub isolation: ChainIsolation,
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
    /// Whether this step was skipped due to chain cancellation.
    #[serde(default)]
    pub skipped: bool,
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
    /// Live partial output from the currently running step.
    ///
    /// Updated incrementally as streaming output arrives. `None` when
    /// no step is running (chain completed or not yet started).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub current_step_partial_output: Option<String>,
    /// Unix timestamp (seconds) when the current step started.
    ///
    /// Callers can compute elapsed time as `now - started_at`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub current_step_started_at: Option<u64>,
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
    /// Chain was cancelled; remaining steps were skipped.
    Cancelled,
}

/// Callback for receiving partial output chunks during streaming execution.
pub type OnOutputChunk = Arc<dyn Fn(&str) + Send + Sync>;

fn extract_json_path(json_str: &str, path: &str) -> Option<String> {
    if path == "." || path.is_empty() {
        return Some(json_str.to_string());
    }
    let value: serde_json::Value = serde_json::from_str(json_str).ok()?;
    let mut current = &value;
    for key in path.split('.') {
        current = current.get(key)?;
    }
    Some(match current {
        serde_json::Value::String(s) => s.clone(),
        other => other.to_string(),
    })
}

fn expand_step_refs(mut text: String, step_context: &HashMap<String, String>) -> String {
    for (key, value) in step_context {
        text = text.replace(&format!("{{steps.{key}}}"), value);
    }
    text
}

fn unix_secs_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

/// Execute a chain of steps against the pool.
pub async fn execute_chain<S: PoolStore + 'static>(
    pool: &Pool<S>,
    skills: &SkillRegistry,
    steps: &[ChainStep],
) -> crate::Result<ChainResult> {
    execute_chain_with_progress(pool, skills, steps, None, None).await
}

/// Execute a chain with optional progress tracking.
///
/// If `chain_task_id` is provided, intermediate progress is stored so callers
/// can poll for status. When a chain task ID is present, steps execute with
/// streaming output so partial results are visible via
/// [`Pool::chain_progress`](crate::Pool::chain_progress). If `working_dir`
/// is provided, all steps use that directory instead of the slot's default.
pub async fn execute_chain_with_progress<S: PoolStore + 'static>(
    pool: &Pool<S>,
    skills: &SkillRegistry,
    steps: &[ChainStep],
    chain_task_id: Option<&TaskId>,
    working_dir: Option<&std::path::Path>,
) -> crate::Result<ChainResult> {
    let mut step_results = Vec::with_capacity(steps.len());
    let mut previous_output = String::new();
    let mut total_cost = 0u64;
    let mut step_context: HashMap<String, String> = HashMap::new();

    for (step_idx, step) in steps.iter().enumerate() {
        // Check for cancellation before starting each step.
        if let Some(task_id) = chain_task_id
            && let Ok(Some(task)) = pool.store().get_task(task_id).await
            && task.state == TaskState::Cancelled
        {
            for s in &steps[step_idx..] {
                step_results.push(StepResult {
                    name: s.name.clone(),
                    output: String::new(),
                    success: false,
                    cost_microdollars: 0,
                    retries_used: 0,
                    skipped: true,
                });
            }
            update_chain_progress_final(
                pool,
                Some(task_id),
                steps.len(),
                &step_results,
                ChainStatus::Cancelled,
            )
            .await;
            return Ok(ChainResult {
                final_output: previous_output,
                steps: step_results,
                total_cost_microdollars: total_cost,
                success: false,
            });
        }

        // Update progress in the store if we have a task ID.
        if let Some(task_id) = chain_task_id {
            let progress = ChainProgress {
                total_steps: steps.len(),
                current_step: Some(step_idx),
                current_step_name: Some(step.name.clone()),
                current_step_partial_output: Some(String::new()),
                current_step_started_at: Some(unix_secs_now()),
                completed_steps: step_results.clone(),
                status: ChainStatus::Running,
            };
            pool.set_chain_progress(task_id, progress).await;
        }

        let prompt = render_step_prompt(step, &previous_output, skills, &step_context)?;

        // Build an output callback that updates chain progress when we have a task ID.
        let on_output: Option<OnOutputChunk> = chain_task_id.map(|tid| {
            let pool = pool.clone();
            let tid = tid.clone();
            Arc::new(move |chunk: &str| {
                pool.append_chain_partial_output(&tid, chunk);
            }) as OnOutputChunk
        });

        let (step_result, step_cost) = execute_step_with_retries(
            pool,
            step,
            &prompt,
            &previous_output,
            skills,
            on_output.clone(),
            working_dir,
            &step_context,
        )
        .await;

        total_cost += step_cost;

        match step_result {
            Ok(result) => {
                previous_output = result.output.clone();

                if result.success {
                    for (var_name, path) in &step.output_vars {
                        match extract_json_path(&result.output, path) {
                            Some(extracted) => {
                                step_context
                                    .insert(format!("{}.{}", step.name, var_name), extracted);
                            }
                            None => {
                                tracing::warn!(
                                    step = %step.name,
                                    var = %var_name,
                                    path = %path,
                                    "output_var extraction failed (output not JSON or path not found)"
                                );
                            }
                        }
                    }
                }

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
                    skipped: false,
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

/// Render the prompt for a step, substituting `{previous_output}` and step refs.
fn render_step_prompt(
    step: &ChainStep,
    previous_output: &str,
    skills: &SkillRegistry,
    step_context: &HashMap<String, String>,
) -> crate::Result<String> {
    match &step.action {
        StepAction::Prompt { prompt } => {
            let rendered = prompt.replace("{previous_output}", previous_output);
            Ok(expand_step_refs(rendered, step_context))
        }
        StepAction::Skill { skill, arguments } => {
            let skill_def = skills
                .get(skill)
                .ok_or_else(|| crate::Error::Store(format!("skill not found: {skill}")))?;
            let mut args = arguments.clone();
            if !previous_output.is_empty() {
                args.entry("_previous_output".into())
                    .or_insert(previous_output.to_string());
            }
            let rendered = skill_def.render(&args)?;
            Ok(expand_step_refs(rendered, step_context))
        }
    }
}

/// Execute a step with retry and recovery support.
///
/// Returns `Ok(StepResult)` on success (or successful recovery), or
/// `Err(error_message)` if all retries and recovery are exhausted.
#[allow(clippy::too_many_arguments)]
async fn execute_step_with_retries<S: PoolStore + 'static>(
    pool: &Pool<S>,
    step: &ChainStep,
    initial_prompt: &str,
    previous_output: &str,
    skills: &SkillRegistry,
    on_output: Option<OnOutputChunk>,
    working_dir: Option<&std::path::Path>,
    step_context: &HashMap<String, String>,
) -> (std::result::Result<StepResult, String>, u64) {
    let max_attempts = 1 + step.failure_policy.retries;
    let mut total_cost = 0u64;
    let mut last_error = String::new();

    for attempt in 0..max_attempts {
        let prompt = if attempt == 0 {
            initial_prompt.to_string()
        } else {
            // Re-render the prompt for retries (same prompt, fresh attempt).
            match render_step_prompt(step, previous_output, skills, step_context) {
                Ok(p) => p,
                Err(e) => return (Err(e.to_string()), total_cost),
            }
        };

        let result = pool
            .run_with_config_streaming(
                &prompt,
                step.config.clone(),
                on_output.clone(),
                working_dir.map(|p| p.to_path_buf()),
            )
            .await;

        match result {
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
                            skipped: false,
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
        let recovery_prompt = expand_step_refs(
            recovery_template
                .replace("{error}", &last_error)
                .replace("{previous_output}", previous_output),
            step_context,
        );

        tracing::info!(step = %step.name, "attempting recovery prompt");

        let result = pool
            .run_with_config_streaming(
                &recovery_prompt,
                step.config.clone(),
                on_output,
                working_dir.map(|p| p.to_path_buf()),
            )
            .await;

        match result {
            Ok(task_result) => {
                total_cost += task_result.cost_microdollars;
                return (
                    Ok(StepResult {
                        name: step.name.clone(),
                        output: task_result.output,
                        success: task_result.success,
                        cost_microdollars: total_cost,
                        retries_used: max_attempts,
                        skipped: false,
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
            current_step_partial_output: None,
            current_step_started_at: None,
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
            output_vars: Default::default(),
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
                skipped: false,
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
        assert_eq!(opts.isolation, ChainIsolation::Worktree);
    }

    #[test]
    fn chain_isolation_serde_roundtrip() {
        let worktree = ChainIsolation::Worktree;
        let json = serde_json::to_string(&worktree).unwrap();
        assert_eq!(json, r#""worktree""#);

        let none = ChainIsolation::None;
        let json = serde_json::to_string(&none).unwrap();
        assert_eq!(json, r#""none""#);

        let parsed: ChainIsolation = serde_json::from_str(r#""worktree""#).unwrap();
        assert_eq!(parsed, ChainIsolation::Worktree);

        let parsed: ChainIsolation = serde_json::from_str(r#""none""#).unwrap();
        assert_eq!(parsed, ChainIsolation::None);
    }

    #[test]
    fn chain_options_with_isolation_serializes() {
        let opts = ChainOptions {
            tags: vec!["test".into()],
            isolation: ChainIsolation::Worktree,
        };
        let json = serde_json::to_string(&opts).unwrap();
        let parsed: ChainOptions = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.isolation, ChainIsolation::Worktree);
        assert_eq!(parsed.tags, vec!["test"]);
    }

    #[test]
    fn chain_progress_serializes_with_partial_output() {
        let progress = ChainProgress {
            total_steps: 3,
            current_step: Some(1),
            current_step_name: Some("implement".into()),
            current_step_partial_output: Some("partial text".into()),
            current_step_started_at: Some(1700000000),
            completed_steps: vec![StepResult {
                name: "plan".into(),
                output: "planned".into(),
                success: true,
                cost_microdollars: 500,
                retries_used: 0,
                skipped: false,
            }],
            status: ChainStatus::Running,
        };

        let json = serde_json::to_string(&progress).unwrap();
        assert!(json.contains("implement"));
        assert!(json.contains("running"));
        assert!(json.contains("partial text"));
        assert!(json.contains("1700000000"));
    }

    #[test]
    fn chain_progress_omits_none_fields() {
        let progress = ChainProgress {
            total_steps: 2,
            current_step: None,
            current_step_name: None,
            current_step_partial_output: None,
            current_step_started_at: None,
            completed_steps: vec![],
            status: ChainStatus::Completed,
        };

        let json = serde_json::to_string(&progress).unwrap();
        assert!(!json.contains("current_step_partial_output"));
        assert!(!json.contains("current_step_started_at"));
    }

    #[test]
    fn chain_progress_empty_partial_output_when_step_starts() {
        let progress = ChainProgress {
            total_steps: 3,
            current_step: Some(0),
            current_step_name: Some("plan".into()),
            current_step_partial_output: Some(String::new()),
            current_step_started_at: Some(1700000000),
            completed_steps: vec![],
            status: ChainStatus::Running,
        };

        let json = serde_json::to_string(&progress).unwrap();
        // Empty string is still serialized (not None).
        assert!(json.contains("\"current_step_partial_output\":\"\""));
    }

    #[test]
    fn cancelled_status_serializes() {
        let progress = ChainProgress {
            total_steps: 3,
            current_step: None,
            current_step_name: None,
            current_step_partial_output: None,
            current_step_started_at: None,
            completed_steps: vec![
                StepResult {
                    name: "plan".into(),
                    output: "planned".into(),
                    success: true,
                    cost_microdollars: 500,
                    retries_used: 0,
                    skipped: false,
                },
                StepResult {
                    name: "implement".into(),
                    output: String::new(),
                    success: false,
                    cost_microdollars: 0,
                    retries_used: 0,
                    skipped: true,
                },
                StepResult {
                    name: "review".into(),
                    output: String::new(),
                    success: false,
                    cost_microdollars: 0,
                    retries_used: 0,
                    skipped: true,
                },
            ],
            status: ChainStatus::Cancelled,
        };

        let json = serde_json::to_string(&progress).unwrap();
        assert!(json.contains("cancelled"));
        assert!(json.contains("\"skipped\":true"));
    }

    #[test]
    fn skipped_defaults_to_false_on_deserialize() {
        let json =
            r#"{"name":"s","output":"o","success":true,"cost_microdollars":0,"retries_used":0}"#;
        let result: StepResult = serde_json::from_str(json).unwrap();
        assert!(!result.skipped);
    }

    #[test]
    fn extract_json_path_whole_output() {
        let json = r#"{"a": 1}"#;
        assert_eq!(extract_json_path(json, "."), Some(json.to_string()));
        assert_eq!(extract_json_path(json, ""), Some(json.to_string()));
    }

    #[test]
    fn extract_json_path_top_level_key() {
        let json = r#"{"summary": "all good"}"#;
        assert_eq!(
            extract_json_path(json, "summary"),
            Some("all good".to_string())
        );
    }

    #[test]
    fn extract_json_path_nested() {
        let json = r#"{"result": {"count": 42}}"#;
        assert_eq!(
            extract_json_path(json, "result.count"),
            Some("42".to_string())
        );
    }

    #[test]
    fn extract_json_path_not_json() {
        assert_eq!(extract_json_path("not json", "key"), None);
    }

    #[test]
    fn extract_json_path_missing_key() {
        let json = r#"{"a": 1}"#;
        assert_eq!(extract_json_path(json, "b"), None);
    }

    #[test]
    fn expand_step_refs_substitutes() {
        let mut ctx = HashMap::new();
        ctx.insert("plan.summary".into(), "do stuff".into());
        let text = "Based on {steps.plan.summary}, implement it.".to_string();
        assert_eq!(
            expand_step_refs(text, &ctx),
            "Based on do stuff, implement it."
        );
    }

    #[test]
    fn expand_step_refs_unknown_left_as_is() {
        let ctx = HashMap::new();
        let text = "Use {steps.missing.var} here.".to_string();
        assert_eq!(expand_step_refs(text.clone(), &ctx), text);
    }

    #[test]
    fn chain_step_output_vars_defaults_empty() {
        let json = r#"{"name":"s","action":{"type":"prompt","prompt":"hi"}}"#;
        let step: ChainStep = serde_json::from_str(json).unwrap();
        assert!(step.output_vars.is_empty());
    }

    #[test]
    fn chain_step_serializes_output_vars() {
        let mut vars = HashMap::new();
        vars.insert("summary".into(), "result.summary".into());
        let step = ChainStep {
            name: "s".into(),
            action: StepAction::Prompt {
                prompt: "hi".into(),
            },
            config: None,
            failure_policy: StepFailurePolicy::default(),
            output_vars: vars,
        };
        let json = serde_json::to_string(&step).unwrap();
        assert!(json.contains("output_vars"));
        assert!(json.contains("result.summary"));

        let parsed: ChainStep = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.output_vars.get("summary").unwrap(), "result.summary");
    }
}
