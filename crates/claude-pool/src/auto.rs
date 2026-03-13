//! Auto-routing: let a single LLM call pick run/fan_out/chain.
//!
//! The [`AutoRoute`] enum represents the three execution paths the pool
//! supports. [`Pool::auto`] sends the user's prompt to a routing LLM call
//! that classifies the work into one of these three, then executes it.
//!
//! # Configuration layers
//!
//! The routing prompt is assembled from up to three layers:
//!
//! 1. **System prompt** — the built-in classification instructions
//!    (loaded from `prompts/auto_route.md` via `include_str!`). Can be
//!    overridden entirely via [`AutoConfig::custom_prompt`].
//! 2. **Hints** — optional structured [`AutoHint`] that constrain or bias
//!    the routing decision (max parallelism, preferred route, domain context,
//!    decomposition boundaries).
//! 3. **Task** — the actual work prompt.
//!
//! # Prompt iteration
//!
//! The system prompt lives in `src/prompts/auto_route.md`. You can test
//! routing decisions without compiling by feeding the file to `claude`
//! directly, or by using [`Pool::route`] which classifies without executing.

use std::fmt;

use claude_wrapper::ClaudeCommand;
use serde::{Deserialize, Serialize};

use crate::chain::{ChainOptions, ChainResult, ChainStep, StepAction, StepFailurePolicy};
use crate::pool::Pool;
use crate::skill::SkillRegistry;
use crate::store::PoolStore;
use crate::types::TaskResult;

/// The default routing system prompt, loaded from `prompts/auto_route.md`.
const DEFAULT_ROUTING_PROMPT: &str = include_str!("prompts/auto_route.md");

/// Soft routing preference. The router can still disagree.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RoutePreference {
    /// Prefer running as a single task.
    PreferSingle,
    /// Prefer splitting into parallel tasks.
    PreferParallel,
    /// Prefer an ordered chain of steps.
    PreferChain,
}

impl fmt::Display for RoutePreference {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::PreferSingle => write!(f, "single"),
            Self::PreferParallel => write!(f, "parallel"),
            Self::PreferChain => write!(f, "chain"),
        }
    }
}

/// Structured hints that inform the routing decision without overriding it.
///
/// Hints are rendered into the prompt's context layer. They constrain or bias
/// the router but do not force a specific route — for that, call
/// [`Pool::run`], [`Pool::fan_out`], or [`Pool::submit_chain`] directly.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AutoHint {
    /// Cap on parallel tasks (e.g. "I only have 2 slots").
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_parallel: Option<usize>,
    /// Cap on chain depth.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_chain_steps: Option<usize>,
    /// Soft bias toward a route. The router can still disagree.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub prefer: Option<RoutePreference>,
    /// Domain description (not instructions).
    /// e.g. "monorepo with independent crates", "microservices behind a gateway".
    #[serde(skip_serializing_if = "Option::is_none")]
    pub domain: Option<String>,
    /// Pre-named boundaries for parallel/chain decomposition.
    /// e.g. `["auth module", "api module", "db module"]`.
    /// The router uses these if it picks parallel/chain, ignores them for single.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub decomposition_hints: Option<Vec<String>>,
}

/// Full configuration for auto-routing.
///
/// For most callers, [`Pool::auto`] or [`Pool::auto_with_hints`] is sufficient.
/// Use `AutoConfig` when you need the escape hatch of a custom prompt.
#[derive(Debug, Clone, Default)]
pub struct AutoConfig {
    /// Override the built-in routing prompt entirely.
    ///
    /// You probably don't want this. The default prompt has been tuned to
    /// produce reliable three-way classification. But we aren't your dad.
    ///
    /// If set, replaces layer 1 (the system prompt). Hints still render
    /// into the context layer if present, and the task still appends.
    pub custom_prompt: Option<String>,
    /// Structured hints (rendered into the prompt's context layer).
    pub hints: Option<AutoHint>,
}

/// Render hints into a context section for the routing prompt.
fn render_hints(hints: &AutoHint) -> String {
    let mut parts = Vec::new();

    if let Some(n) = hints.max_parallel {
        parts.push(format!("- Maximum parallel tasks: {n}"));
    }
    if let Some(n) = hints.max_chain_steps {
        parts.push(format!("- Maximum chain steps: {n}"));
    }
    if let Some(pref) = &hints.prefer {
        parts.push(format!(
            "- Preferred route: {pref} (but choose differently if the task clearly warrants it)"
        ));
    }
    if let Some(domain) = &hints.domain {
        parts.push(format!("- Domain: {domain}"));
    }
    if let Some(decomp) = &hints.decomposition_hints
        && !decomp.is_empty()
    {
        parts.push(format!(
            "- Suggested decomposition boundaries: {}",
            decomp.join(", ")
        ));
    }

    if parts.is_empty() {
        return String::new();
    }

    let mut section = String::from("\n\n## Constraints\n\n");
    section.push_str(&parts.join("\n"));
    section
}

/// Assemble the full routing prompt from config and task.
pub(crate) fn assemble_routing_prompt(task: &str, config: Option<&AutoConfig>) -> String {
    let base = config
        .and_then(|c| c.custom_prompt.as_deref())
        .unwrap_or(DEFAULT_ROUTING_PROMPT);

    let mut prompt = base.to_string();

    if let Some(hints) = config.and_then(|c| c.hints.as_ref()) {
        prompt.push_str(&render_hints(hints));
    }

    prompt.push_str("\n\n## Task\n\n");
    prompt.push_str(task);
    prompt
}

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
    ///
    /// # Decomposition boundary
    ///
    /// For parallel and chain routes, the router decomposes the task into
    /// subtasks/steps. This works well when the decomposition is obvious from
    /// the prompt (e.g. "review these 5 files" -> one task per file). For
    /// ambiguous decompositions, prefer explicit [`Pool::fan_out`] or
    /// [`Pool::submit_chain`] where the caller controls the split.
    ///
    /// # Fallback
    ///
    /// If the routing LLM returns unparseable output, the original prompt is
    /// executed as a single task rather than returning an error. Wrong routing
    /// is suboptimal, not catastrophic.
    pub async fn auto(&self, prompt: &str) -> crate::Result<AutoResult> {
        self.auto_with_config(prompt, None).await
    }

    /// Auto-route with structured hints.
    ///
    /// Hints inform the routing decision without overriding it. See
    /// [`AutoHint`] for available fields.
    pub async fn auto_with_hints(
        &self,
        prompt: &str,
        hints: &AutoHint,
    ) -> crate::Result<AutoResult> {
        let config = AutoConfig {
            custom_prompt: None,
            hints: Some(hints.clone()),
        };
        self.auto_with_config(prompt, Some(&config)).await
    }

    /// Auto-route with full configuration.
    ///
    /// Use this when you need the escape hatch of a custom prompt or when
    /// combining a custom prompt with hints.
    pub async fn auto_with_config(
        &self,
        prompt: &str,
        config: Option<&AutoConfig>,
    ) -> crate::Result<AutoResult> {
        let route = match self.route_with_config(prompt, config).await {
            Ok(route) => route,
            Err(e) => {
                tracing::warn!(error = %e, "auto-route parse failed, falling back to single");
                AutoRoute::Single {
                    prompt: prompt.to_string(),
                }
            }
        };

        tracing::info!(route = route.route_name(), "auto-route decided");

        self.execute_route(route).await
    }

    /// Route only: get the routing decision without executing.
    ///
    /// Useful for debugging, logging, or prompt iteration — see what the
    /// router would choose without spending slots on execution.
    pub async fn route(&self, prompt: &str) -> crate::Result<AutoRoute> {
        self.route_with_config(prompt, None).await
    }

    /// Route with structured hints (no execution).
    pub async fn route_with_hints(
        &self,
        prompt: &str,
        hints: &AutoHint,
    ) -> crate::Result<AutoRoute> {
        let config = AutoConfig {
            custom_prompt: None,
            hints: Some(hints.clone()),
        };
        self.route_with_config(prompt, Some(&config)).await
    }

    /// Route with full configuration (no execution).
    pub async fn route_with_config(
        &self,
        prompt: &str,
        config: Option<&AutoConfig>,
    ) -> crate::Result<AutoRoute> {
        let routing_prompt = assemble_routing_prompt(prompt, config);

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
    fn fallback_to_single_on_parse_failure() {
        // Simulates what auto_with_context does: if routing fails, wrap as single.
        let original_prompt = "do the thing";
        let route =
            parse_route_from_output("unparseable garbage").unwrap_or_else(|_| AutoRoute::Single {
                prompt: original_prompt.to_string(),
            });
        match route {
            AutoRoute::Single { prompt } => assert_eq!(prompt, "do the thing"),
            _ => panic!("expected fallback to Single"),
        }
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

    // --- Hint and prompt assembly tests ---

    #[test]
    fn render_empty_hints_produces_nothing() {
        let hints = AutoHint::default();
        assert_eq!(render_hints(&hints), "");
    }

    #[test]
    fn render_hints_max_parallel() {
        let hints = AutoHint {
            max_parallel: Some(3),
            ..Default::default()
        };
        let rendered = render_hints(&hints);
        assert!(rendered.contains("Maximum parallel tasks: 3"));
        assert!(rendered.contains("## Constraints"));
    }

    #[test]
    fn render_hints_max_chain_steps() {
        let hints = AutoHint {
            max_chain_steps: Some(4),
            ..Default::default()
        };
        let rendered = render_hints(&hints);
        assert!(rendered.contains("Maximum chain steps: 4"));
    }

    #[test]
    fn render_hints_preference() {
        let hints = AutoHint {
            prefer: Some(RoutePreference::PreferParallel),
            ..Default::default()
        };
        let rendered = render_hints(&hints);
        assert!(rendered.contains("Preferred route: parallel"));
        assert!(rendered.contains("choose differently if the task clearly warrants it"));
    }

    #[test]
    fn render_hints_domain() {
        let hints = AutoHint {
            domain: Some("monorepo with independent crates".into()),
            ..Default::default()
        };
        let rendered = render_hints(&hints);
        assert!(rendered.contains("Domain: monorepo with independent crates"));
    }

    #[test]
    fn render_hints_decomposition() {
        let hints = AutoHint {
            decomposition_hints: Some(vec![
                "auth module".into(),
                "api module".into(),
                "db module".into(),
            ]),
            ..Default::default()
        };
        let rendered = render_hints(&hints);
        assert!(
            rendered
                .contains("Suggested decomposition boundaries: auth module, api module, db module")
        );
    }

    #[test]
    fn render_hints_empty_decomposition_skipped() {
        let hints = AutoHint {
            decomposition_hints: Some(vec![]),
            ..Default::default()
        };
        assert_eq!(render_hints(&hints), "");
    }

    #[test]
    fn render_hints_all_fields() {
        let hints = AutoHint {
            max_parallel: Some(2),
            max_chain_steps: Some(3),
            prefer: Some(RoutePreference::PreferChain),
            domain: Some("microservices".into()),
            decomposition_hints: Some(vec!["svc-a".into(), "svc-b".into()]),
        };
        let rendered = render_hints(&hints);
        assert!(rendered.contains("Maximum parallel tasks: 2"));
        assert!(rendered.contains("Maximum chain steps: 3"));
        assert!(rendered.contains("Preferred route: chain"));
        assert!(rendered.contains("Domain: microservices"));
        assert!(rendered.contains("svc-a, svc-b"));
    }

    #[test]
    fn assemble_prompt_no_config() {
        let prompt = assemble_routing_prompt("do the thing", None);
        assert!(prompt.starts_with("You are a work router."));
        assert!(prompt.contains("## Task\n\ndo the thing"));
        assert!(!prompt.contains("## Constraints"));
    }

    #[test]
    fn assemble_prompt_with_hints() {
        let config = AutoConfig {
            custom_prompt: None,
            hints: Some(AutoHint {
                max_parallel: Some(2),
                ..Default::default()
            }),
        };
        let prompt = assemble_routing_prompt("review files", Some(&config));
        assert!(prompt.starts_with("You are a work router."));
        assert!(prompt.contains("## Constraints"));
        assert!(prompt.contains("Maximum parallel tasks: 2"));
        assert!(prompt.contains("## Task\n\nreview files"));
    }

    #[test]
    fn assemble_prompt_with_custom_prompt() {
        let config = AutoConfig {
            custom_prompt: Some("You are a custom router.".into()),
            hints: None,
        };
        let prompt = assemble_routing_prompt("my task", Some(&config));
        assert!(prompt.starts_with("You are a custom router."));
        assert!(!prompt.contains("You are a work router."));
        assert!(prompt.contains("## Task\n\nmy task"));
    }

    #[test]
    fn assemble_prompt_custom_prompt_with_hints() {
        let config = AutoConfig {
            custom_prompt: Some("Custom instructions.".into()),
            hints: Some(AutoHint {
                domain: Some("testing".into()),
                ..Default::default()
            }),
        };
        let prompt = assemble_routing_prompt("task", Some(&config));
        assert!(prompt.starts_with("Custom instructions."));
        assert!(prompt.contains("## Constraints"));
        assert!(prompt.contains("Domain: testing"));
        assert!(prompt.contains("## Task\n\ntask"));
    }

    #[test]
    fn default_prompt_loaded_from_file() {
        assert!(DEFAULT_ROUTING_PROMPT.contains("You are a work router."));
        assert!(DEFAULT_ROUTING_PROMPT.contains("THREE options"));
        assert!(DEFAULT_ROUTING_PROMPT.contains("SINGLE"));
        assert!(DEFAULT_ROUTING_PROMPT.contains("PARALLEL"));
        assert!(DEFAULT_ROUTING_PROMPT.contains("CHAIN"));
    }

    #[test]
    fn route_preference_display() {
        assert_eq!(RoutePreference::PreferSingle.to_string(), "single");
        assert_eq!(RoutePreference::PreferParallel.to_string(), "parallel");
        assert_eq!(RoutePreference::PreferChain.to_string(), "chain");
    }

    #[test]
    fn route_preference_serde_roundtrip() {
        let pref = RoutePreference::PreferParallel;
        let json = serde_json::to_string(&pref).unwrap();
        let parsed: RoutePreference = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, RoutePreference::PreferParallel);
    }

    #[test]
    fn auto_hint_serde_skips_none_fields() {
        let hints = AutoHint {
            max_parallel: Some(3),
            ..Default::default()
        };
        let json = serde_json::to_string(&hints).unwrap();
        assert!(json.contains("max_parallel"));
        assert!(!json.contains("max_chain_steps"));
        assert!(!json.contains("prefer"));
        assert!(!json.contains("domain"));
        assert!(!json.contains("decomposition_hints"));
    }

    #[test]
    fn auto_hint_default_is_empty() {
        let hints = AutoHint::default();
        assert!(hints.max_parallel.is_none());
        assert!(hints.max_chain_steps.is_none());
        assert!(hints.prefer.is_none());
        assert!(hints.domain.is_none());
        assert!(hints.decomposition_hints.is_none());
    }
}
