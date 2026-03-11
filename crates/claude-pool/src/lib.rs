//! MCP server for managing a pool of Claude CLI slots.
//!
//! `claude-pool` manages N Claude CLI instances behind an MCP server interface.
//! A coordinator (typically an interactive Claude session) calls MCP tools to
//! submit work, fan out tasks, chain pipelines, and track budgets. The pool
//! handles slot lifecycle, session resumption, restarts, and shared context.
//!
//! # Architecture
//!
//! ```text
//! Coordinator (interactive Claude session)
//!   +-- .mcp.json includes "claude-pool"
//!         +-- claude-pool MCP server
//!               +-- slot-0 (Claude instance)
//!               +-- slot-1 (Claude instance)
//!               +-- slot-N
//! ```
//!
//! One server. N slots. Nothing else.

pub mod chain;
pub(crate) mod cli_parsing;
pub mod config;
pub mod error;
pub(crate) mod executor;
pub mod messaging;
pub mod pool;
pub mod skill;
pub mod store;
pub mod supervisor;
pub mod types;
pub mod workflow;
pub mod worktree;

pub use chain::{
    ChainIsolation, ChainOptions, ChainProgress, ChainResult, ChainStatus, ChainStep, StepAction,
    StepFailurePolicy, StepResult, execute_chain,
};
pub use error::{Error, Result};
pub use messaging::{Message, MessageBus};
pub use pool::{DrainSummary, Pool, PoolBuilder, PoolStatus};
pub use skill::{RegisteredSkill, Skill, SkillArgument, SkillRegistry, SkillScope, SkillSource};
pub use store::{InMemoryStore, PoolStore};
pub use supervisor::{SupervisorHandle, check_and_restart_slots};
pub use types::*;
pub use workflow::{Workflow, WorkflowArgument, WorkflowRegistry};
pub use worktree::WorktreeManager;
