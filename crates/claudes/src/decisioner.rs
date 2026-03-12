use anyhow::{Context, Result};
use claude_wrapper::{Claude, ClaudeCommand, OutputFormat, PermissionMode, QueryCommand};
use serde::{Deserialize, Serialize};

use crate::context::TaskContext;

/// An execution plan produced by the decisioner.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "strategy", rename_all = "snake_case")]
pub enum ExecutionPlan {
    /// Run a single claude call.
    Single {
        prompt: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        model: Option<String>,
    },
    /// Run multiple independent tasks in parallel.
    Parallel {
        tasks: Vec<PlannedTask>,
        #[serde(skip_serializing_if = "Option::is_none")]
        slots: Option<usize>,
    },
    /// Run tasks sequentially, output of each feeds the next.
    Chain { steps: Vec<PlannedStep> },
}

/// A single task within a parallel execution plan.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlannedTask {
    pub prompt: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
}

/// A single step within a chain execution plan.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlannedStep {
    pub name: String,
    pub prompt: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
}

/// Strategy hint that tunes the decisioner's behavior.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Strategy {
    /// Let the decisioner choose based on task complexity.
    #[default]
    Auto,
    /// Prefer single calls. Only parallelize when explicitly multi-part.
    Conservative,
    /// Split when clearly independent, chain when dependent.
    Balanced,
    /// Parallelize eagerly.
    Aggressive,
}

impl std::fmt::Display for Strategy {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Auto => write!(f, "auto"),
            Self::Conservative => write!(f, "conservative"),
            Self::Balanced => write!(f, "balanced"),
            Self::Aggressive => write!(f, "aggressive"),
        }
    }
}

impl std::str::FromStr for Strategy {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self> {
        match s.to_lowercase().as_str() {
            "auto" => Ok(Self::Auto),
            "conservative" => Ok(Self::Conservative),
            "balanced" => Ok(Self::Balanced),
            "aggressive" => Ok(Self::Aggressive),
            _ => anyhow::bail!(
                "unknown strategy: {s} (expected: auto, conservative, balanced, aggressive)"
            ),
        }
    }
}

/// The decisioner trait. Implement this for custom decision logic.
pub trait Decisioner: Send + Sync {
    /// Given a task and codebase context, produce an execution plan.
    fn decide(
        &self,
        task: &str,
        context: &TaskContext,
        strategy: Strategy,
        max_budget_usd: Option<f64>,
    ) -> impl std::future::Future<Output = Result<ExecutionPlan>> + Send;
}

/// Default decisioner that uses a single claude call to produce an execution plan.
pub struct ClaudeDecisioner<'a> {
    claude: &'a Claude,
}

impl<'a> ClaudeDecisioner<'a> {
    pub fn new(claude: &'a Claude) -> Self {
        Self { claude }
    }

    fn build_system_prompt(&self, strategy: Strategy, max_budget_usd: Option<f64>) -> String {
        let mut prompt = SYSTEM_PROMPT.to_string();

        prompt.push_str(&format!(
            "\n\n## Strategy\nUse the `{}` strategy:\n",
            strategy
        ));
        match strategy {
            Strategy::Auto => {
                prompt.push_str("- Analyze the task and pick the best approach.\n");
                prompt.push_str("- Use a single call for simple tasks.\n");
                prompt.push_str(
                    "- Split into parallel tasks when there are clearly independent units.\n",
                );
                prompt.push_str("- Use a chain when steps depend on each other.\n");
            }
            Strategy::Conservative => {
                prompt.push_str("- Strongly prefer a single call.\n");
                prompt
                    .push_str("- Only use parallel/chain if the task is explicitly multi-part.\n");
            }
            Strategy::Balanced => {
                prompt.push_str("- Use single for simple tasks.\n");
                prompt.push_str("- Split into parallel when units are clearly independent.\n");
                prompt.push_str("- Chain when steps have dependencies.\n");
            }
            Strategy::Aggressive => {
                prompt.push_str("- Eagerly split tasks into parallel subtasks.\n");
                prompt.push_str("- Prefer more smaller tasks over fewer larger ones.\n");
                prompt.push_str("- Only use single for truly atomic operations.\n");
            }
        }

        if let Some(budget) = max_budget_usd {
            prompt.push_str(&format!(
                "\n## Budget Constraint\nThe total budget is ${:.2}. ",
                budget
            ));
            if budget < 0.50 {
                prompt.push_str("This is a tight budget. Prefer haiku and single calls.\n");
            } else if budget < 2.00 {
                prompt.push_str(
                    "Moderate budget. Use sonnet for main work, haiku for simple subtasks.\n",
                );
            } else {
                prompt.push_str("Generous budget. Use the best model for each subtask.\n");
            }
        }

        prompt
    }
}

impl Decisioner for ClaudeDecisioner<'_> {
    async fn decide(
        &self,
        task: &str,
        context: &TaskContext,
        strategy: Strategy,
        max_budget_usd: Option<f64>,
    ) -> Result<ExecutionPlan> {
        let system = self.build_system_prompt(strategy, max_budget_usd);
        let user_prompt = format!(
            "## Task\n{}\n\n## Codebase Context\n{}",
            task,
            context.to_prompt_section()
        );

        let output = QueryCommand::new(&user_prompt)
            .system_prompt(&system)
            .output_format(OutputFormat::Json)
            .permission_mode(PermissionMode::Plan)
            .no_session_persistence()
            .max_turns(1)
            .execute(self.claude)
            .await
            .context("decisioner claude call failed")?;

        // Parse the result text — extract JSON from the response.
        let result_text = &output.stdout;

        // Try to parse the JSON output. The wrapper returns JSON with a "result" field.
        let plan = parse_plan_from_output(result_text)
            .context("failed to parse execution plan from decisioner output")?;

        Ok(plan)
    }
}

/// Parse an execution plan from the claude JSON output.
///
/// The wrapper returns `{"result": "...", ...}` where the result field contains
/// the decisioner's response. We look for a JSON object with a "strategy" field.
fn parse_plan_from_output(output: &str) -> Result<ExecutionPlan> {
    // First try: parse the whole output as a QueryResult and extract the result text.
    if let Ok(query_result) = serde_json::from_str::<serde_json::Value>(output)
        && let Some(result_text) = query_result.get("result").and_then(|v| v.as_str())
        && let Ok(plan) = extract_json_plan(result_text)
    {
        return Ok(plan);
    }

    // Second try: maybe the output is the plan directly.
    if let Ok(plan) = extract_json_plan(output) {
        return Ok(plan);
    }

    anyhow::bail!("could not find a valid execution plan in the decisioner output")
}

/// Extract a JSON execution plan from text that may contain markdown fences.
fn extract_json_plan(text: &str) -> Result<ExecutionPlan> {
    // Try direct parse first.
    if let Ok(plan) = serde_json::from_str::<ExecutionPlan>(text) {
        return Ok(plan);
    }

    // Try extracting from ```json ... ``` blocks.
    if let Some(start) = text.find("```json") {
        let json_start = start + 7;
        if let Some(end) = text[json_start..].find("```") {
            let json_str = text[json_start..json_start + end].trim();
            if let Ok(plan) = serde_json::from_str::<ExecutionPlan>(json_str) {
                return Ok(plan);
            }
        }
    }

    // Try extracting from ``` ... ``` blocks (no language tag).
    if let Some(start) = text.find("```\n") {
        let json_start = start + 4;
        if let Some(end) = text[json_start..].find("```") {
            let json_str = text[json_start..json_start + end].trim();
            if let Ok(plan) = serde_json::from_str::<ExecutionPlan>(json_str) {
                return Ok(plan);
            }
        }
    }

    // Try finding a JSON object with "strategy" anywhere in the text.
    if let Some(start) = text.find(r#""strategy""#) {
        // Walk backwards to find the opening brace.
        let before = &text[..start];
        if let Some(brace) = before.rfind('{') {
            let candidate = &text[brace..];
            // Find matching closing brace (simple nesting count).
            let mut depth = 0;
            let mut end = 0;
            for (i, ch) in candidate.char_indices() {
                match ch {
                    '{' => depth += 1,
                    '}' => {
                        depth -= 1;
                        if depth == 0 {
                            end = i + 1;
                            break;
                        }
                    }
                    _ => {}
                }
            }
            if end > 0 {
                let json_str = &candidate[..end];
                if let Ok(plan) = serde_json::from_str::<ExecutionPlan>(json_str) {
                    return Ok(plan);
                }
            }
        }
    }

    anyhow::bail!("no valid JSON execution plan found in text")
}

const SYSTEM_PROMPT: &str = r#"You are a workload decisioner. Given a task description and codebase context, you decide the optimal execution strategy.

You MUST respond with ONLY a JSON object (no markdown, no explanation, no text before or after). The JSON must have a "strategy" field set to one of: "single", "parallel", or "chain".

## Output Formats

### Single task
Use when the task is a single unit of work.
```
{"strategy": "single", "prompt": "the full task prompt for the worker", "model": "sonnet"}
```

### Parallel tasks
Use when the task contains clearly independent subtasks that can run simultaneously.
```
{"strategy": "parallel", "tasks": [{"prompt": "subtask 1", "model": "sonnet"}, {"prompt": "subtask 2", "model": "haiku"}]}
```

### Sequential chain
Use when tasks depend on each other (output of one feeds the next).
```
{"strategy": "chain", "steps": [{"name": "step-1", "prompt": "first step", "model": "sonnet"}, {"name": "step-2", "prompt": "use the output above to do next step", "model": "sonnet"}]}
```

## Model Selection

Pick the model based on subtask complexity:
- **haiku**: simple, bounded tasks (formatting, simple lookups, one-file fixes)
- **sonnet**: most work (multi-file changes, refactoring, feature implementation)
- **opus**: complex architectural decisions, large-scope reasoning

Default to sonnet when unsure. Each task/step prompt should be self-contained and detailed enough for a worker to execute without additional context.

## Rules

1. RESPOND WITH ONLY JSON. No markdown fences, no explanation.
2. For the "prompt" fields, write detailed instructions as if briefing a developer. Include file paths, function names, and specific requirements when possible.
3. If the task is simple enough for one call, use "single". Don't over-split.
4. Parallel tasks must be truly independent — no task should depend on another's output.
5. Chain steps should reference "the output above" or "the previous step" when they depend on prior work.
6. Keep the number of parallel tasks or chain steps reasonable (2-6 typically)."#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_single_plan() {
        let json = r#"{"strategy": "single", "prompt": "fix the bug", "model": "sonnet"}"#;
        let plan = extract_json_plan(json).unwrap();
        match plan {
            ExecutionPlan::Single { prompt, model } => {
                assert_eq!(prompt, "fix the bug");
                assert_eq!(model.as_deref(), Some("sonnet"));
            }
            _ => panic!("expected Single"),
        }
    }

    #[test]
    fn test_parse_parallel_plan() {
        let json = r#"{"strategy": "parallel", "tasks": [{"prompt": "task 1"}, {"prompt": "task 2", "model": "haiku"}]}"#;
        let plan = extract_json_plan(json).unwrap();
        match plan {
            ExecutionPlan::Parallel { tasks, .. } => {
                assert_eq!(tasks.len(), 2);
                assert_eq!(tasks[0].prompt, "task 1");
                assert!(tasks[0].model.is_none());
                assert_eq!(tasks[1].model.as_deref(), Some("haiku"));
            }
            _ => panic!("expected Parallel"),
        }
    }

    #[test]
    fn test_parse_chain_plan() {
        let json = r#"{"strategy": "chain", "steps": [{"name": "step-1", "prompt": "do first thing"}, {"name": "step-2", "prompt": "then do this"}]}"#;
        let plan = extract_json_plan(json).unwrap();
        match plan {
            ExecutionPlan::Chain { steps } => {
                assert_eq!(steps.len(), 2);
                assert_eq!(steps[0].name, "step-1");
            }
            _ => panic!("expected Chain"),
        }
    }

    #[test]
    fn test_parse_from_markdown_fence() {
        let text = r#"Here is the plan:
```json
{"strategy": "single", "prompt": "fix it", "model": "haiku"}
```
"#;
        let plan = extract_json_plan(text).unwrap();
        assert!(matches!(plan, ExecutionPlan::Single { .. }));
    }

    #[test]
    fn test_parse_from_query_result() {
        let output = r#"{"result": "{\"strategy\": \"single\", \"prompt\": \"do the thing\", \"model\": \"sonnet\"}", "session_id": "abc", "cost_usd": 0.01}"#;
        let plan = parse_plan_from_output(output).unwrap();
        assert!(matches!(plan, ExecutionPlan::Single { .. }));
    }

    #[test]
    fn test_parse_from_embedded_json() {
        let text = r#"I think the best approach is {"strategy": "single", "prompt": "just do it", "model": "sonnet"} and that should work."#;
        let plan = extract_json_plan(text).unwrap();
        assert!(matches!(plan, ExecutionPlan::Single { .. }));
    }

    #[test]
    fn test_strategy_display_parse_roundtrip() {
        for s in [
            Strategy::Auto,
            Strategy::Conservative,
            Strategy::Balanced,
            Strategy::Aggressive,
        ] {
            let parsed: Strategy = s.to_string().parse().unwrap();
            assert_eq!(parsed, s);
        }
    }

    #[test]
    fn test_strategy_from_str_invalid() {
        assert!("invalid".parse::<Strategy>().is_err());
    }

    #[test]
    fn test_plan_serde_roundtrip() {
        let plan = ExecutionPlan::Parallel {
            tasks: vec![
                PlannedTask {
                    prompt: "a".into(),
                    model: Some("haiku".into()),
                },
                PlannedTask {
                    prompt: "b".into(),
                    model: None,
                },
            ],
            slots: Some(2),
        };
        let json = serde_json::to_string(&plan).unwrap();
        let parsed: ExecutionPlan = serde_json::from_str(&json).unwrap();
        assert!(matches!(parsed, ExecutionPlan::Parallel { .. }));
    }
}
