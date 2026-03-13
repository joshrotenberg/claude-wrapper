//! Auto-routing: let a single LLM call pick run/fan_out/chain.
//!
//! The [`AutoRoute`] enum represents the three execution paths the pool
//! supports. [`Pool::auto`] sends the user's prompt to a routing LLM call
//! that classifies the work into one of these three, then executes it.

use claude_wrapper::ClaudeCommand;
use serde::{Deserialize, Serialize};

use crate::chain::{ChainOptions, ChainResult, ChainStep, StepAction, StepFailurePolicy};
use crate::pool::Pool;
use crate::skill::SkillRegistry;
use crate::store::PoolStore;
use crate::types::TaskResult;

/// The routing decision made by the LLM.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "route", rename_all = "snake_case")]
pub enum AutoRoute {
    /// Run a single task.
    Single { prompt: String },
    /// Run N independent tasks in parallel.
    Parallel { prompts: Vec<String> },
    /// Run an ordered pipeline of steps.
    Chain { steps: Vec<AutoStep> },
}

/// A step in an auto-routed chain.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AutoStep {
    /// Step name.
    pub name: String,
    /// Step prompt (may contain `{previous_output}`).
    pub prompt: String,
}

/// Result of an auto-routed execution.
#[derive(Debug, Clone)]
pub enum AutoResult {
    /// Result from a single task.
    Single(TaskResult),
    /// Results from parallel tasks.
    Parallel(Vec<TaskResult>),
    /// Result from a chain.
    Chain(ChainResult),
}

impl AutoResult {
    /// Get the final output text regardless of which route was taken.
    pub fn output(&self) -> String {
        match self {
            Self::Single(r) => r.output.clone(),
            Self::Parallel(results) => results
                .iter()
                .enumerate()
                .map(|(i, r)| format!("[{}] {}", i, r.output.trim()))
                .collect::<Vec<_>>()
                .join("\n"),
            Self::Chain(r) => r.final_output.clone(),
        }
    }

    /// Get the route that was chosen.
    pub fn route_name(&self) -> &'static str {
        match self {
            Self::Single(_) => "single",
            Self::Parallel(_) => "parallel",
            Self::Chain(_) => "chain",
        }
    }

    /// Total cost in microdollars (best-effort; chain cost may be 0 due to known tracking gap).
    pub fn cost_microdollars(&self) -> u64 {
        match self {
            Self::Single(r) => r.cost_microdollars,
            Self::Parallel(results) => results.iter().map(|r| r.cost_microdollars).sum(),
            Self::Chain(r) => r.total_cost_microdollars,
        }
    }
}

impl<S: PoolStore + 'static> Pool<S> {
    /// Auto-route a task: let an LLM decide whether to run, fan_out, or chain.
    ///
    /// Sends `prompt` to a single routing call that classifies the work,
    /// then executes via the chosen pool method.
    pub async fn auto(&self, prompt: &str) -> crate::Result<AutoResult> {
        let route = self.route(prompt).await?;

        tracing::info!(route = route.route_name(), "auto-route decided");

        self.execute_route(route).await
    }

    /// Route only: get the routing decision without executing.
    ///
    /// Useful for debugging or logging what the router would choose.
    pub async fn route(&self, prompt: &str) -> crate::Result<AutoRoute> {
        let routing_prompt = format!("{}\n\n## Task\n\n{}", ROUTING_SYSTEM_PROMPT, prompt);

        let cmd = claude_wrapper::QueryCommand::new(&routing_prompt)
            .output_format(claude_wrapper::OutputFormat::Json)
            .permission_mode(claude_wrapper::PermissionMode::Plan)
            .no_session_persistence()
            .max_turns(1);

        let output = cmd
            .execute(self.claude())
            .await
            .map_err(crate::Error::Wrapper)?;

        parse_route_from_output(&output.stdout)
    }

    /// Execute an already-decided route.
    pub async fn execute_route(&self, route: AutoRoute) -> crate::Result<AutoResult> {
        match route {
            AutoRoute::Single { prompt } => {
                let result = self.run(&prompt).await?;
                Ok(AutoResult::Single(result))
            }
            AutoRoute::Parallel { prompts } => {
                let refs: Vec<&str> = prompts.iter().map(|s| s.as_str()).collect();
                let results = self.fan_out(&refs).await?;
                Ok(AutoResult::Parallel(results))
            }
            AutoRoute::Chain { steps } => {
                let chain_steps: Vec<ChainStep> = steps
                    .into_iter()
                    .map(|s| ChainStep {
                        name: s.name,
                        action: StepAction::Prompt { prompt: s.prompt },
                        config: None,
                        failure_policy: StepFailurePolicy::default(),
                        output_vars: Default::default(),
                    })
                    .collect();

                let skills = SkillRegistry::new();
                let task_id = self
                    .submit_chain(chain_steps, &skills, ChainOptions::default())
                    .await?;

                // Poll for result.
                loop {
                    if let Some(result) = self.result(&task_id).await? {
                        // Chain results are serialized as JSON in the task output.
                        if let Ok(chain_result) =
                            serde_json::from_str::<ChainResult>(&result.output)
                        {
                            return Ok(AutoResult::Chain(chain_result));
                        }
                        // Fallback: wrap the raw output as a single result.
                        return Ok(AutoResult::Single(result));
                    }
                    tokio::time::sleep(std::time::Duration::from_secs(1)).await;
                }
            }
        }
    }
}

impl AutoRoute {
    /// Short name for the route.
    fn route_name(&self) -> &'static str {
        match self {
            Self::Single { .. } => "single",
            Self::Parallel { .. } => "parallel",
            Self::Chain { .. } => "chain",
        }
    }
}

// --- Parsing ---

/// Parse the routing decision from Claude's JSON output.
///
/// The wrapper returns `{"result": "...", ...}` where the result field
/// contains the router's response. Falls back through several extraction
/// strategies.
pub(crate) fn parse_route_from_output(output: &str) -> crate::Result<AutoRoute> {
    // First try: parse as QueryResult wrapper, extract the result text.
    if let Ok(query_result) = serde_json::from_str::<serde_json::Value>(output)
        && let Some(result_text) = query_result.get("result").and_then(|v| v.as_str())
        && let Ok(route) = extract_json_route(result_text)
    {
        return Ok(route);
    }

    // Second try: raw output is the route directly.
    if let Ok(route) = extract_json_route(output) {
        return Ok(route);
    }

    Err(crate::Error::Store(
        "could not parse routing decision from LLM output".into(),
    ))
}

/// Extract a JSON route from text that may contain markdown fences or surrounding text.
pub(crate) fn extract_json_route(text: &str) -> crate::Result<AutoRoute> {
    // Direct parse.
    if let Ok(route) = serde_json::from_str::<AutoRoute>(text) {
        return Ok(route);
    }

    // ```json ... ``` blocks.
    if let Some(start) = text.find("```json") {
        let json_start = start + 7;
        if let Some(end) = text[json_start..].find("```") {
            let json_str = text[json_start..json_start + end].trim();
            if let Ok(route) = serde_json::from_str::<AutoRoute>(json_str) {
                return Ok(route);
            }
        }
    }

    // ``` ... ``` blocks (no language tag).
    if let Some(start) = text.find("```\n") {
        let json_start = start + 4;
        if let Some(end) = text[json_start..].find("```") {
            let json_str = text[json_start..json_start + end].trim();
            if let Ok(route) = serde_json::from_str::<AutoRoute>(json_str) {
                return Ok(route);
            }
        }
    }

    // Scan for a JSON object with "route" key.
    if let Some(start) = text.find(r#""route""#) {
        let before = &text[..start];
        if let Some(brace) = before.rfind('{') {
            let candidate = &text[brace..];
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
                if let Ok(route) = serde_json::from_str::<AutoRoute>(json_str) {
                    return Ok(route);
                }
            }
        }
    }

    Err(crate::Error::Store(
        "no valid JSON routing decision found in text".into(),
    ))
}

// --- Prompt ---

const ROUTING_SYSTEM_PROMPT: &str = r#"You are a work router. Given a task, you decide how to execute it.

You have exactly THREE options:

1. SINGLE — one task, one result. Use when the work is one coherent unit.
2. PARALLEL — N independent tasks that can run simultaneously. Use when there are clearly independent subtasks with no dependencies between them.
3. CHAIN — ordered steps where each feeds the next. Use when later steps depend on earlier results.

Rules:
- Respond with ONLY a JSON object. No markdown fences, no explanation, no text before or after.
- If in doubt, use SINGLE. Only split when the task is clearly multi-part.
- PARALLEL tasks must be truly independent — no task should need another's output.
- CHAIN steps should reference "{previous_output}" when they depend on prior work.
- Keep prompts detailed and self-contained. Each prompt should make sense on its own.
- Keep the number of parallel tasks or chain steps reasonable (2-6).

Output format:

For SINGLE:
{"route": "single", "prompt": "the full task prompt"}

For PARALLEL:
{"route": "parallel", "prompts": ["task 1", "task 2", "task 3"]}

For CHAIN:
{"route": "chain", "steps": [{"name": "step-1", "prompt": "first step"}, {"name": "step-2", "prompt": "use {previous_output} to do the next thing"}]}"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_single_route() {
        let json = r#"{"route": "single", "prompt": "fix the bug"}"#;
        let route = extract_json_route(json).unwrap();
        match route {
            AutoRoute::Single { prompt } => assert_eq!(prompt, "fix the bug"),
            _ => panic!("expected Single"),
        }
    }

    #[test]
    fn parse_parallel_route() {
        let json =
            r#"{"route": "parallel", "prompts": ["review a.rs", "review b.rs", "review c.rs"]}"#;
        let route = extract_json_route(json).unwrap();
        match route {
            AutoRoute::Parallel { prompts } => {
                assert_eq!(prompts.len(), 3);
                assert_eq!(prompts[0], "review a.rs");
            }
            _ => panic!("expected Parallel"),
        }
    }

    #[test]
    fn parse_chain_route() {
        let json = r#"{"route": "chain", "steps": [{"name": "analyze", "prompt": "analyze the code"}, {"name": "fix", "prompt": "fix based on {previous_output}"}]}"#;
        let route = extract_json_route(json).unwrap();
        match route {
            AutoRoute::Chain { steps } => {
                assert_eq!(steps.len(), 2);
                assert_eq!(steps[0].name, "analyze");
                assert!(steps[1].prompt.contains("{previous_output}"));
            }
            _ => panic!("expected Chain"),
        }
    }

    #[test]
    fn parse_from_markdown_fence() {
        let text = r#"Here is my decision:
```json
{"route": "single", "prompt": "just do it"}
```
"#;
        let route = extract_json_route(text).unwrap();
        assert!(matches!(route, AutoRoute::Single { .. }));
    }

    #[test]
    fn parse_from_bare_fence() {
        let text = "```\n{\"route\": \"single\", \"prompt\": \"do it\"}\n```\n";
        let route = extract_json_route(text).unwrap();
        assert!(matches!(route, AutoRoute::Single { .. }));
    }

    #[test]
    fn parse_from_embedded_json() {
        let text = r#"I think this should be {"route": "single", "prompt": "just do it"} and that's my answer."#;
        let route = extract_json_route(text).unwrap();
        assert!(matches!(route, AutoRoute::Single { .. }));
    }

    #[test]
    fn parse_from_query_result_wrapper() {
        let output = r#"{"result": "{\"route\": \"parallel\", \"prompts\": [\"a\", \"b\"]}", "session_id": "abc", "cost_usd": 0.01}"#;
        let route = parse_route_from_output(output).unwrap();
        match route {
            AutoRoute::Parallel { prompts } => assert_eq!(prompts.len(), 2),
            _ => panic!("expected Parallel"),
        }
    }

    #[test]
    fn parse_fails_on_garbage() {
        assert!(extract_json_route("this is not json at all").is_err());
        assert!(parse_route_from_output("garbage").is_err());
    }

    #[test]
    fn auto_result_output_single() {
        let result = AutoResult::Single(TaskResult::success(String::from("hello world"), 100, 50));
        assert_eq!(result.output(), "hello world");
        assert_eq!(result.route_name(), "single");
        assert_eq!(result.cost_microdollars(), 100);
    }

    #[test]
    fn auto_result_output_parallel() {
        let results = vec![
            TaskResult::success(String::from("one"), 100, 50),
            TaskResult::success(String::from("two"), 200, 50),
        ];
        let result = AutoResult::Parallel(results);
        assert_eq!(result.route_name(), "parallel");
        assert_eq!(result.cost_microdollars(), 300);
        assert!(result.output().contains("[0] one"));
        assert!(result.output().contains("[1] two"));
    }

    #[test]
    fn auto_result_output_chain() {
        let chain = ChainResult {
            steps: vec![],
            final_output: "chain done".into(),
            total_cost_microdollars: 500,
            success: true,
        };
        let result = AutoResult::Chain(chain);
        assert_eq!(result.output(), "chain done");
        assert_eq!(result.route_name(), "chain");
        assert_eq!(result.cost_microdollars(), 500);
    }

    #[test]
    fn serde_roundtrip_single() {
        let route = AutoRoute::Single {
            prompt: "test".into(),
        };
        let json = serde_json::to_string(&route).unwrap();
        let parsed: AutoRoute = serde_json::from_str(&json).unwrap();
        assert!(matches!(parsed, AutoRoute::Single { .. }));
    }

    #[test]
    fn serde_roundtrip_parallel() {
        let route = AutoRoute::Parallel {
            prompts: vec!["a".into(), "b".into()],
        };
        let json = serde_json::to_string(&route).unwrap();
        let parsed: AutoRoute = serde_json::from_str(&json).unwrap();
        assert!(matches!(parsed, AutoRoute::Parallel { .. }));
    }

    #[test]
    fn serde_roundtrip_chain() {
        let route = AutoRoute::Chain {
            steps: vec![AutoStep {
                name: "s1".into(),
                prompt: "do it".into(),
            }],
        };
        let json = serde_json::to_string(&route).unwrap();
        let parsed: AutoRoute = serde_json::from_str(&json).unwrap();
        assert!(matches!(parsed, AutoRoute::Chain { .. }));
    }
}
