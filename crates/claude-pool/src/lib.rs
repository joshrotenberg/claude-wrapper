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
//!
//! # Getting Started
//!
//! For most use cases, import the [`prelude`] module:
//!
//! ```rust,no_run
//! use claude_pool::prelude::*;
//! ```
//!
//! This brings in the most commonly needed types: [`Pool`], [`PoolBuilder`],
//! [`TaskRecord`], [`TaskState`], [`ChainResult`], and related types.

pub mod chain;
pub(crate) mod cli_parsing;
pub mod config;
pub mod error;
pub(crate) mod executor;
pub mod messaging;
pub mod pool;
pub mod prelude;
pub mod skill;
pub mod store;
pub mod supervisor;
pub mod types;
pub(crate) mod utils;
pub mod workflow;
pub mod worktree;

// Core pool types
pub use pool::{DrainSummary, Pool, PoolBuilder, PoolStatus, RunOptions};

// Error handling
pub use error::{Error, Result};

// All types (backwards compatibility)
pub use types::*;

// Chain execution
pub use chain::{
    ChainIsolation, ChainOptions, ChainProgress, ChainResult, ChainStatus, ChainStep, StepAction,
    StepFailurePolicy, StepResult, execute_chain,
};

// Skill management
pub use skill::{RegisteredSkill, Skill, SkillArgument, SkillRegistry, SkillScope, SkillSource};

// Storage
pub use store::{InMemoryStore, PoolStore};

// Supervisor
pub use supervisor::{SupervisorHandle, check_and_restart_slots};

// Messaging
pub use messaging::{Message, MessageBus};

// Workflow
pub use workflow::{Workflow, WorkflowArgument, WorkflowRegistry};

// Worktree management
pub use worktree::WorktreeManager;
