//! claude-runner — autonomous GitHub issue runner.
//!
//! Turns GitHub issues into pull requests through a staged pipeline.
//! Agent-agnostic design with three separated layers:
//!
//! ```text
//! GitHub (issues/PRs)  -->  Orchestrator  -->  Agent (claude/codex/etc)
//!     platform layer        domain layer        execution layer
//! ```
//!
//! # Architecture
//!
//! - **Platform** (`github`): Repository adapter for issues, PRs, comments, labels
//! - **Domain** (`policy`, `triage`, `planner`, `workflow`): Orchestration logic
//! - **Execution** (`executor`, `isolation`): Agent invocation and work isolation

pub mod config;
pub mod error;
pub mod executor;
pub mod github;
pub mod isolation;
pub mod orchestrator;
pub mod planner;
pub mod policy;
pub mod state;
pub mod triage;
pub mod workflow;

pub use config::RunnerConfig;
pub use orchestrator::{process_batch, process_issue};
