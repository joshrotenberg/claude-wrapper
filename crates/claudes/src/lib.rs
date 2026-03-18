//! claudes — manifest-driven execution engine for headless Claude Code sessions.
//!
//! The core abstraction is the **manifest** — a fully resolved JSON document
//! describing exactly what to execute. The runner takes a manifest and executes it.
//! Everything else exists to produce manifests conveniently.
//!
//! # Architecture
//!
//! ```text
//! CLI args / Config / MCP  -->  Planner  -->  Manifest  -->  Runner  -->  Results
//! ```

pub mod cli;
pub mod error;
pub mod isolation;
pub mod manifest;
pub mod output;
pub mod planner;
pub mod runner;
pub mod state;

pub use error::{Error, Result};
pub use manifest::{Isolation, Manifest, Task};
pub use planner::{PlanOptions, plan};
pub use runner::{CleanupPolicy, RunOptions, RunResult, TaskEvent, TaskResult, run};
