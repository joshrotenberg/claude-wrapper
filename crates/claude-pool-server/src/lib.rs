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
use tokio::sync::RwLock;

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
}
