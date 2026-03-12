//! Skills management REST endpoints.
//!
//! - `GET /v1/skills` — list registered skills
//! - `GET /v1/skills/:name` — get skill details
//! - `POST /v1/skills` — register a skill
//! - `DELETE /v1/skills/:name` — remove a skill

use std::sync::Arc;

use axum::Json;
use axum::extract::{Path, State};
use claude_pool::PoolStore;
use claude_pool::skill::{SkillArgument, SkillScope, SkillSource};
use serde::{Deserialize, Serialize};

use crate::rest::AppState;
use crate::rest::error::ProblemDetails;

/// Response body for a single skill.
#[derive(Debug, Serialize)]
pub struct SkillResponse {
    pub name: String,
    pub description: String,
    pub scope: String,
    pub source: Option<String>,
    pub arguments: Vec<SkillArgumentResponse>,
}

/// Skill argument in response body.
#[derive(Debug, Serialize)]
pub struct SkillArgumentResponse {
    pub name: String,
    pub description: String,
    pub required: bool,
}

/// Request body for `POST /v1/skills`.
#[derive(Debug, Deserialize)]
pub struct RegisterSkillRequest {
    pub name: String,
    pub description: String,
    pub prompt: String,
    #[serde(default)]
    pub arguments: Vec<SkillArgumentInput>,
    #[serde(default = "default_scope")]
    pub scope: String,
}

fn default_scope() -> String {
    "task".to_string()
}

/// Skill argument in request body.
#[derive(Debug, Deserialize)]
pub struct SkillArgumentInput {
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub required: bool,
}

fn skill_response(
    skill: &claude_pool::skill::Skill,
    source: Option<&SkillSource>,
) -> SkillResponse {
    SkillResponse {
        name: skill.name.clone(),
        description: skill.description.clone(),
        scope: format!("{:?}", skill.scope).to_lowercase(),
        source: source.map(|s| format!("{s}")),
        arguments: skill
            .arguments
            .iter()
            .map(|a| SkillArgumentResponse {
                name: a.name.clone(),
                description: a.description.clone(),
                required: a.required,
            })
            .collect(),
    }
}

/// `GET /v1/skills` — list all registered skills.
pub async fn list_skills<S: PoolStore + 'static>(
    State(state): State<Arc<AppState<S>>>,
) -> Result<Json<Vec<SkillResponse>>, ProblemDetails> {
    let skills = state.state.skills.read().await;
    let list: Vec<SkillResponse> = skills
        .list_registered()
        .iter()
        .map(|rs| skill_response(&rs.skill, Some(&rs.source)))
        .collect();
    Ok(Json(list))
}

/// `GET /v1/skills/:name` — get a skill by name.
pub async fn get_skill<S: PoolStore + 'static>(
    State(state): State<Arc<AppState<S>>>,
    Path(name): Path<String>,
) -> Result<Json<SkillResponse>, ProblemDetails> {
    let skills = state.state.skills.read().await;
    let registered = skills
        .get_registered(&name)
        .ok_or_else(|| ProblemDetails::not_found("skill", &name))?;
    Ok(Json(skill_response(
        &registered.skill,
        Some(&registered.source),
    )))
}

/// `POST /v1/skills` — register a new skill.
pub async fn register_skill<S: PoolStore + 'static>(
    State(state): State<Arc<AppState<S>>>,
    Json(req): Json<RegisterSkillRequest>,
) -> Result<(axum::http::StatusCode, Json<SkillResponse>), ProblemDetails> {
    let scope = match req.scope.as_str() {
        "task" => SkillScope::Task,
        "coordinator" => SkillScope::Coordinator,
        "chain" => SkillScope::Chain,
        _ => {
            return Err(ProblemDetails::bad_request(
                "scope must be 'task', 'coordinator', or 'chain'",
            ));
        }
    };

    let skill = claude_pool::skill::Skill {
        name: req.name.clone(),
        description: req.description,
        prompt: req.prompt,
        arguments: req
            .arguments
            .into_iter()
            .map(|a| SkillArgument {
                name: a.name,
                description: a.description,
                required: a.required,
            })
            .collect(),
        config: None,
        scope,
        argument_hint: None,
        skill_dir: None,
    };

    let response = skill_response(&skill, Some(&SkillSource::Runtime));

    let mut skills = state.state.skills.write().await;
    skills.register(skill, SkillSource::Runtime);

    Ok((axum::http::StatusCode::CREATED, Json(response)))
}

/// `DELETE /v1/skills/:name` — remove a skill.
pub async fn remove_skill<S: PoolStore + 'static>(
    State(state): State<Arc<AppState<S>>>,
    Path(name): Path<String>,
) -> Result<axum::http::StatusCode, ProblemDetails> {
    let mut skills = state.state.skills.write().await;
    skills
        .remove(&name)
        .ok_or_else(|| ProblemDetails::not_found("skill", &name))?;
    Ok(axum::http::StatusCode::NO_CONTENT)
}
