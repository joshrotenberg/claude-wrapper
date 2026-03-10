//! MCP resource definitions for claude-pool.

use std::sync::Arc;

use claude_pool::PoolStore;
use tower_mcp::protocol::{ReadResourceResult, ResourceContent};
use tower_mcp::resource::{Resource, ResourceBuilder, ResourceTemplate, ResourceTemplateBuilder};

use crate::State;

fn text_resource(uri: &str, json: String) -> ReadResourceResult {
    ReadResourceResult {
        contents: vec![ResourceContent {
            uri: uri.to_string(),
            mime_type: Some("application/json".to_string()),
            text: Some(json),
            blob: None,
            meta: None,
        }],
        meta: None,
    }
}

pub fn pool_status_resource<S: PoolStore + 'static>(state: Arc<State<S>>) -> Resource {
    ResourceBuilder::new("pool://status")
        .name("Pool Status")
        .description("Pool overview: workers, budget, tasks")
        .mime_type("application/json")
        .handler(move || {
            let state = Arc::clone(&state);
            async move {
                let status = state
                    .pool
                    .status()
                    .await
                    .map_err(|e| tower_mcp::Error::internal(e.to_string()))?;
                let json = serde_json::to_string_pretty(&status)?;
                Ok(text_resource("pool://status", json))
            }
        })
        .build()
}

pub fn pool_workers_resource<S: PoolStore + 'static>(state: Arc<State<S>>) -> Resource {
    ResourceBuilder::new("pool://workers")
        .name("Workers")
        .description("List of all workers with state and stats")
        .mime_type("application/json")
        .handler(move || {
            let state = Arc::clone(&state);
            async move {
                let workers = state
                    .pool
                    .store()
                    .list_workers()
                    .await
                    .map_err(|e| tower_mcp::Error::internal(e.to_string()))?;
                let json = serde_json::to_string_pretty(&workers)?;
                Ok(text_resource("pool://workers", json))
            }
        })
        .build()
}

pub fn pool_budget_resource<S: PoolStore + 'static>(state: Arc<State<S>>) -> Resource {
    ResourceBuilder::new("pool://budget")
        .name("Budget")
        .description("Budget breakdown: total, spent, remaining, per-worker")
        .mime_type("application/json")
        .handler(move || {
            let state = Arc::clone(&state);
            async move {
                let status = state
                    .pool
                    .status()
                    .await
                    .map_err(|e| tower_mcp::Error::internal(e.to_string()))?;
                let workers = state
                    .pool
                    .store()
                    .list_workers()
                    .await
                    .map_err(|e| tower_mcp::Error::internal(e.to_string()))?;

                let per_worker: Vec<serde_json::Value> = workers
                    .iter()
                    .map(|w| {
                        serde_json::json!({
                            "id": w.id.0,
                            "cost_microdollars": w.cost_microdollars,
                            "tasks_completed": w.tasks_completed,
                        })
                    })
                    .collect();

                let remaining = status
                    .budget_microdollars
                    .map(|b| b.saturating_sub(status.total_spend_microdollars));

                let budget = serde_json::json!({
                    "total_microdollars": status.budget_microdollars,
                    "spent_microdollars": status.total_spend_microdollars,
                    "remaining_microdollars": remaining,
                    "per_worker": per_worker,
                });

                let json = serde_json::to_string_pretty(&budget)?;
                Ok(text_resource("pool://budget", json))
            }
        })
        .build()
}

pub fn pool_context_resource<S: PoolStore + 'static>(state: Arc<State<S>>) -> Resource {
    ResourceBuilder::new("pool://context")
        .name("Shared Context")
        .description("All shared context key-value pairs")
        .mime_type("application/json")
        .handler(move || {
            let state = Arc::clone(&state);
            async move {
                let entries = state.pool.list_context();
                let map: serde_json::Map<String, serde_json::Value> = entries
                    .into_iter()
                    .map(|(k, v)| (k, serde_json::Value::String(v)))
                    .collect();
                let json = serde_json::to_string_pretty(&serde_json::Value::Object(map))?;
                Ok(text_resource("pool://context", json))
            }
        })
        .build()
}

pub fn pool_worker_template<S: PoolStore + 'static>(state: Arc<State<S>>) -> ResourceTemplate {
    ResourceTemplateBuilder::new("pool://workers/{id}")
        .name("Worker Detail")
        .description("Detail for a specific worker")
        .mime_type("application/json")
        .handler(
            move |uri: String, vars: std::collections::HashMap<String, String>| {
                let state = Arc::clone(&state);
                async move {
                    let id = vars
                        .get("id")
                        .ok_or_else(|| tower_mcp::Error::internal("missing id parameter"))?;
                    let worker_id = claude_pool::WorkerId(id.clone());
                    let worker = state
                        .pool
                        .store()
                        .get_worker(&worker_id)
                        .await
                        .map_err(|e| tower_mcp::Error::internal(e.to_string()))?
                        .ok_or_else(|| {
                            tower_mcp::Error::internal(format!("worker not found: {id}"))
                        })?;
                    let json = serde_json::to_string_pretty(&worker)?;
                    Ok(text_resource(&uri, json))
                }
            },
        )
}

pub fn pool_result_template<S: PoolStore + 'static>(state: Arc<State<S>>) -> ResourceTemplate {
    ResourceTemplateBuilder::new("pool://results/{task_id}")
        .name("Task Result")
        .description("Result for a specific task")
        .mime_type("application/json")
        .handler(
            move |uri: String, vars: std::collections::HashMap<String, String>| {
                let state = Arc::clone(&state);
                async move {
                    let id = vars
                        .get("task_id")
                        .ok_or_else(|| tower_mcp::Error::internal("missing task_id parameter"))?;
                    let task_id = claude_pool::TaskId(id.clone());
                    let task = state
                        .pool
                        .store()
                        .get_task(&task_id)
                        .await
                        .map_err(|e| tower_mcp::Error::internal(e.to_string()))?
                        .ok_or_else(|| {
                            tower_mcp::Error::internal(format!("task not found: {id}"))
                        })?;
                    let json = serde_json::to_string_pretty(&task)?;
                    Ok(text_resource(&uri, json))
                }
            },
        )
}

pub fn pool_chain_template<S: PoolStore + 'static>(state: Arc<State<S>>) -> ResourceTemplate {
    ResourceTemplateBuilder::new("pool://chains/{chain_id}")
        .name("Chain Progress")
        .description("Per-step progress for an async chain")
        .mime_type("application/json")
        .handler(
            move |uri: String, vars: std::collections::HashMap<String, String>| {
                let state = Arc::clone(&state);
                async move {
                    let chain_id = vars
                        .get("chain_id")
                        .ok_or_else(|| tower_mcp::Error::internal("missing chain_id parameter"))?;
                    let task_id = claude_pool::TaskId(chain_id.clone());
                    match state.pool.chain_progress(&task_id) {
                        Some(progress) => {
                            let json = serde_json::to_string_pretty(&progress)?;
                            Ok(text_resource(&uri, json))
                        }
                        None => match state.pool.result(&task_id).await {
                            Ok(Some(result)) => {
                                let json = serde_json::to_string_pretty(&result)?;
                                Ok(text_resource(&uri, json))
                            }
                            Ok(None) => Err(tower_mcp::Error::internal(format!(
                                "chain not found: {chain_id}"
                            ))),
                            Err(e) => Err(tower_mcp::Error::internal(e.to_string())),
                        },
                    }
                }
            },
        )
}

/// Build all pool resources.
pub fn all_resources<S: PoolStore + 'static>(state: &Arc<State<S>>) -> Vec<Resource> {
    vec![
        pool_status_resource(Arc::clone(state)),
        pool_workers_resource(Arc::clone(state)),
        pool_budget_resource(Arc::clone(state)),
        pool_context_resource(Arc::clone(state)),
    ]
}

/// Build all pool resource templates.
pub fn all_templates<S: PoolStore + 'static>(state: &Arc<State<S>>) -> Vec<ResourceTemplate> {
    vec![
        pool_worker_template(Arc::clone(state)),
        pool_result_template(Arc::clone(state)),
        pool_chain_template(Arc::clone(state)),
    ]
}
