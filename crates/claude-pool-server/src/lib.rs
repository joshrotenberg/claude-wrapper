//! Library interface for claude-pool-server.
//!
//! Exposes [`State`] and the tool/resource builders so that integration tests
//! (and downstream embedders) can construct a router without going through the
//! binary entry point.

pub mod prompts;
pub mod resources;
pub mod tools;

pub mod auth;

use std::path::PathBuf;
use std::sync::Arc;

use claude_pool::{Pool, PoolStore, SkillRegistry, WorkflowRegistry};
use serde::Serialize;
use tokio::sync::RwLock;

/// Server version and metadata information.
#[derive(Debug, Clone, Serialize)]
pub struct ServerInfo {
    /// Server version from Cargo.toml.
    pub version: String,
    /// Git commit hash (short form).
    pub commit: String,
    /// Default model for slots.
    pub model: Option<String>,
    /// Permission mode for slots.
    pub permission_mode: String,
    /// Total number of slots.
    pub slots: usize,
}

impl ServerInfo {
    /// Create a new ServerInfo instance.
    pub fn new(model: Option<String>, permission_mode: String, slots: usize) -> Self {
        Self {
            version: env!("CARGO_PKG_VERSION").to_string(),
            commit: env!("GIT_HASH").to_string(),
            model,
            permission_mode,
            slots,
        }
    }
}

/// Shared state accessible by all tool/resource handlers.
pub struct State<S: PoolStore> {
    /// The pool instance.
    pub pool: Pool<S>,
    /// Thread-safe skill registry (mutated by skill management tools).
    pub skills: Arc<RwLock<SkillRegistry>>,
    /// Workflow registry.
    pub workflows: WorkflowRegistry,
    /// Directory for persisting project-local skills.
    pub skills_dir: PathBuf,
    /// Server metadata.
    pub server_info: ServerInfo,
}
