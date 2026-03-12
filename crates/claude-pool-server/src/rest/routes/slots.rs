//! Slot management REST endpoints.
//!
//! - `GET /v1/slots` — list all slots
//! - `GET /v1/slots/:id` — get a single slot

use std::sync::Arc;

use axum::Json;
use axum::extract::{Path, Query, State};
use claude_pool::{PoolStore, SlotId, SlotState};
use serde::{Deserialize, Serialize};

use crate::rest::AppState;
use crate::rest::error::ProblemDetails;

/// Query parameters for `GET /v1/slots`.
#[derive(Debug, Deserialize)]
pub struct SlotFilter {
    pub name: Option<String>,
    pub role: Option<String>,
    pub state: Option<String>,
}

/// Response body for a single slot.
#[derive(Debug, Serialize)]
pub struct SlotResponse {
    pub id: String,
    pub state: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub role: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub current_task: Option<String>,
    pub tasks_completed: u64,
    pub cost_microdollars: u64,
    pub restart_count: u32,
}

/// `GET /v1/slots` — list slots with optional filtering.
pub async fn list_slots<S: PoolStore + 'static>(
    State(state): State<Arc<AppState<S>>>,
    Query(filter): Query<SlotFilter>,
) -> Result<Json<Vec<SlotResponse>>, ProblemDetails> {
    let slot_state = filter.state.as_deref().and_then(parse_slot_state);

    let slots = state
        .state
        .pool
        .find_slots(filter.name.as_deref(), filter.role.as_deref(), slot_state)
        .await
        .map_err(ProblemDetails::from)?;

    let responses: Vec<SlotResponse> = slots
        .into_iter()
        .map(|s| SlotResponse {
            id: s.id.0.clone(),
            state: format!("{:?}", s.state).to_lowercase(),
            name: s.config.name,
            role: s.config.role,
            description: s.config.description,
            model: s.config.model,
            current_task: s.current_task.map(|t| t.0),
            tasks_completed: s.tasks_completed,
            cost_microdollars: s.cost_microdollars,
            restart_count: s.restart_count,
        })
        .collect();

    Ok(Json(responses))
}

/// `GET /v1/slots/:id` — get a single slot.
pub async fn get_slot<S: PoolStore + 'static>(
    State(state): State<Arc<AppState<S>>>,
    Path(slot_id): Path<String>,
) -> Result<Json<SlotResponse>, ProblemDetails> {
    let id = SlotId(slot_id.clone());
    let slot = state
        .state
        .pool
        .store()
        .get_slot(&id)
        .await
        .map_err(ProblemDetails::from)?
        .ok_or_else(|| ProblemDetails::not_found("slot", &slot_id))?;

    Ok(Json(SlotResponse {
        id: slot.id.0.clone(),
        state: format!("{:?}", slot.state).to_lowercase(),
        name: slot.config.name,
        role: slot.config.role,
        description: slot.config.description,
        model: slot.config.model,
        current_task: slot.current_task.map(|t| t.0),
        tasks_completed: slot.tasks_completed,
        cost_microdollars: slot.cost_microdollars,
        restart_count: slot.restart_count,
    }))
}

fn parse_slot_state(s: &str) -> Option<SlotState> {
    match s {
        "idle" => Some(SlotState::Idle),
        "busy" => Some(SlotState::Busy),
        "stopped" => Some(SlotState::Stopped),
        "errored" => Some(SlotState::Errored),
        _ => None,
    }
}
