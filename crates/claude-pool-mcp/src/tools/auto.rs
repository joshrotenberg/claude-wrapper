//! Auto-routing tools: auto, auto_with_hints, route, route_with_hints.

use std::sync::Arc;

use claude_pool::{AutoHint, RoutePreference};
use schemars::JsonSchema;
use serde::Deserialize;
use tower_mcp::ToolBuilder;
use tower_mcp::protocol::CallToolResult;
use tower_mcp::tool::Tool;

use super::{PoolState, json_result};

#[derive(Debug, Deserialize, JsonSchema)]
struct AutoInput {
    /// The task prompt. The router decides how to execute it.
    prompt: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct AutoWithHintsInput {
    /// The task prompt.
    prompt: String,
    /// Cap on parallel tasks.
    max_parallel: Option<usize>,
    /// Cap on chain depth.
    max_chain_steps: Option<usize>,
    /// Soft route preference (prefer_single, prefer_parallel, prefer_chain).
    prefer: Option<String>,
    /// Domain description (not instructions).
    domain: Option<String>,
    /// Pre-named decomposition boundaries.
    decomposition_hints: Option<Vec<String>>,
}

fn parse_preference(s: &str) -> Option<RoutePreference> {
    match s {
        "prefer_single" => Some(RoutePreference::PreferSingle),
        "prefer_parallel" => Some(RoutePreference::PreferParallel),
        "prefer_chain" => Some(RoutePreference::PreferChain),
        _ => None,
    }
}

fn hints_from_input(input: &AutoWithHintsInput) -> AutoHint {
    AutoHint {
        max_parallel: input.max_parallel,
        max_chain_steps: input.max_chain_steps,
        prefer: input.prefer.as_deref().and_then(parse_preference),
        domain: input.domain.clone(),
        decomposition_hints: input.decomposition_hints.clone(),
    }
}

pub(super) fn tools(state: PoolState) -> Vec<Tool> {
    vec![
        pool_auto(Arc::clone(&state)),
        pool_auto_with_hints(Arc::clone(&state)),
        pool_route(Arc::clone(&state)),
        pool_route_with_hints(state),
    ]
}

fn pool_auto(state: PoolState) -> Tool {
    ToolBuilder::new("pool_auto")
        .title("Auto Route")
        .description(
            "Auto-route: LLM classifies the task as single/parallel/chain and executes it. \
             Use when you're not sure which execution path is best.",
        )
        .handler(move |input: AutoInput| {
            let state = Arc::clone(&state);
            async move {
                match state.auto(&input.prompt).await {
                    Ok(result) => Ok(CallToolResult::json(serde_json::json!({
                        "route": result.route_name(),
                        "output": result.output(),
                        "cost_microdollars": result.cost_microdollars(),
                    }))),
                    Err(e) => Ok(CallToolResult::error(format!("{e}"))),
                }
            }
        })
        .build()
}

fn pool_auto_with_hints(state: PoolState) -> Tool {
    ToolBuilder::new("pool_auto_with_hints")
        .title("Auto Route with Hints")
        .description(
            "Auto-route with structured hints (max_parallel, max_chain_steps, prefer, domain, \
             decomposition_hints). Hints inform routing without overriding it.",
        )
        .handler(move |input: AutoWithHintsInput| {
            let state = Arc::clone(&state);
            async move {
                let hints = hints_from_input(&input);
                match state.auto_with_hints(&input.prompt, &hints).await {
                    Ok(result) => Ok(CallToolResult::json(serde_json::json!({
                        "route": result.route_name(),
                        "output": result.output(),
                        "cost_microdollars": result.cost_microdollars(),
                    }))),
                    Err(e) => Ok(CallToolResult::error(format!("{e}"))),
                }
            }
        })
        .build()
}

fn pool_route(state: PoolState) -> Tool {
    ToolBuilder::new("pool_route")
        .title("Classify Route")
        .description(
            "Classify a task as single/parallel/chain without executing. For debugging or preview.",
        )
        .read_only_safe()
        .handler(move |input: AutoInput| {
            let state = Arc::clone(&state);
            async move {
                match state.route(&input.prompt).await {
                    Ok(route) => Ok(json_result(&route)),
                    Err(e) => Ok(CallToolResult::error(format!("{e}"))),
                }
            }
        })
        .build()
}

fn pool_route_with_hints(state: PoolState) -> Tool {
    ToolBuilder::new("pool_route_with_hints")
        .title("Classify Route with Hints")
        .description("Classify with structured hints, no execution.")
        .read_only_safe()
        .handler(move |input: AutoWithHintsInput| {
            let state = Arc::clone(&state);
            async move {
                let hints = hints_from_input(&input);
                match state.route_with_hints(&input.prompt, &hints).await {
                    Ok(route) => Ok(json_result(&route)),
                    Err(e) => Ok(CallToolResult::error(format!("{e}"))),
                }
            }
        })
        .build()
}
