//! Coordinator/worker orchestration for Claude Code.
//!
//! `claude-pool` manages N Claude CLI instances behind a pool. A **coordinator**
//! (an interactive Claude session with the pool MCP server in its `.mcp.json`)
//! dispatches work to **worker** slots, monitors results, reviews outputs, and
//! decides what merges. Workers are isolated Claude instances that execute one
//! task at a time.
//!
//! This crate provides the pool library. For the MCP server binary, see
//! `claude-pool-server`.
//!
//! # The Coordinator/Worker Model
//!
//! The coordinator/worker split is the central organizing principle:
//!
//! - **Coordinator**: A long-running interactive Claude session. It holds the
//!   human's context, makes sequencing decisions, reviews outputs, and calls
//!   `pool_*` MCP tools to delegate work. It does not execute tasks directly;
//!   it schedules work and acts on results.
//! - **Worker**: A Claude slot executing a single task (a `pool_run`, a chain
//!   step, or a fan-out branch). Workers are stateless between tasks; they
//!   receive a prompt, do the work, and return output. The pool handles slot
//!   assignment, session resumption, and restarts.
//!
//! This model matters because it separates *what to do* (coordinator) from
//! *how to do it* (worker). The coordinator stays responsive; workers run in
//! parallel without blocking the human's session.
//!
//! # Vocabulary
//!
//! ## Nouns
//!
//! | Term | Meaning |
//! |------|---------|
//! | Coordinator | Interactive Claude session that schedules and reviews work |
//! | Worker | A slot executing one task; returns output to the coordinator |
//! | Slot | A managed Claude CLI instance; the execution unit |
//! | Chain | Ordered pipeline of steps where each step feeds the next |
//! | Fan-out | N independent tasks running in parallel across slots |
//! | Task | A single unit of work submitted to the pool |
//! | Tick | One iteration of the coordinator's monitor loop |
//!
//! ## Verbs
//!
//! | Term | Meaning |
//! |------|---------|
//! | schedule | Coordinator decides what work to dispatch next |
//! | execute | A worker carries out the scheduled task |
//! | monitor | Coordinator polls for results (tick loop) |
//! | review | Coordinator inspects worker output before acting |
//! | merge | Coordinator accepts a worker result into the main branch |
//! | dispatch | Coordinator sends a task to the pool |
//! | fan out | Submit N independent tasks in parallel |
//! | chain | Submit an ordered pipeline of dependent steps |
//!
//! # When to Use What
//!
//! **Inline**: Do the work yourself without the pool. Use when the task is
//! fast, single-step, or requires coordinator context that is impractical to
//! inject.
//!
//! **`pool_run` / run**: One task, one result, blocks until done. Use for
//! single bounded operations (file a ticket, run a check, rebase a branch).
//!
//! **`pool_chain` / chain**: Ordered pipeline where each step depends on the
//! previous. Use when work has a clear linear dependency graph: draft ->
//! review -> revise -> commit, or scaffold -> implement -> test -> PR. If you
//! find yourself writing "and then" more than once, chain instead of run.
//!
//! **`pool_fan_out` / fan out**: N independent tasks in parallel. Use when the
//! same operation applies to N independent targets (audit N crates, refactor N
//! modules). No step depends on another; all run simultaneously.
//!
//! # Coordinator Rhythm
//!
//! A coordinator session follows a repeating rhythm:
//!
//! 1. **Dispatch**: Submit tasks (`pool_submit`, `pool_submit_chain`,
//!    `pool_fan_out`). Receive task or chain IDs.
//! 2. **Monitor**: Poll for results (`pool_result`, `pool_chain_result`).
//!    Repeat until complete. Each poll is a tick.
//! 3. **Review**: Inspect output. Check diffs, test results, summaries.
//!    Decide whether to accept, reject with feedback, or escalate.
//! 4. **Merge**: Accept the result (`pool_approve_result` for reviewed tasks,
//!    or direct merge for unreviewed chains). Update coordinator state.
//!
//! Chains compress this into a single dispatch/monitor cycle. Fan-outs run
//! monitor in parallel. With `pool_submit_with_review`, the coordinator
//! explicitly gates on the review step before the result is finalized.
//!
//! # Model Selection
//!
//! Workers do not need the most capable model for every task. Match the model
//! to the task complexity:
//!
//! - **Haiku**: Bounded single tasks -- file a ticket, run checks, tag a
//!   release, rebase. High-volume fan-outs where throughput matters more than
//!   depth.
//! - **Sonnet**: Multi-file changes, code review needing judgment, planning
//!   steps where quality matters, most chain steps.
//! - **Opus**: Large mechanical refactors where one error breaks compilation,
//!   complex architectural reasoning, tasks where you would send a senior
//!   engineer.
//!
//! Override per-task or per-chain-step with the `model` field in
//! [`RunOptions`]. The coordinator itself typically runs on Sonnet or Opus;
//! workers can use cheaper models for routine work.
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

pub mod auto;
pub mod chain;
pub(crate) mod cli_parsing;
pub mod config;
pub mod error;
pub(crate) mod executor;
pub mod messaging;
pub mod pool;
pub mod prelude;
pub mod route_test;
pub mod store;
pub mod supervisor;
pub mod types;
pub(crate) mod utils;
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

// Storage
pub use store::{InMemoryStore, JsonFileStore, PoolStore};

// Supervisor
pub use supervisor::{SupervisorHandle, check_and_restart_slots};

// Messaging
pub use messaging::{Message, MessageBus};

// Worktree management
pub use worktree::WorktreeManager;

// Auto-routing
pub use auto::{AutoConfig, AutoHint, AutoResult, AutoRoute, AutoStep, RoutePreference};
