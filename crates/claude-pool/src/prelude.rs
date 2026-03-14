//! Commonly used types for working with `claude-pool` as a library.
//!
//! Import this module to bring the most frequently needed types into scope:
//!
//! ```rust,no_run
//! use claude_pool::prelude::*;
//! ```
//!
//! # What's Included
//!
//! **Pool management** — build and run the pool:
//! - [`Pool`], [`PoolBuilder`], [`PoolStatus`], [`DrainSummary`]
//!
//! **Task tracking** — inspect work in flight:
//! - [`TaskRecord`], [`TaskState`], [`TaskResult`], [`TaskFilter`], [`TaskId`]
//!
//! **Chain execution** — build multi-step pipelines:
//! - [`execute_chain`], [`ChainStep`], [`ChainOptions`], [`ChainResult`], [`ChainStatus`]
//!
//! **Configuration** — tune pool and slot behavior:
//! - [`PoolConfig`], [`SlotConfig`]
//!
//! **Errors** — handle failures:
//! - [`Error`], [`Result`]

// Pool management
pub use crate::pool::{DrainSummary, Pool, PoolBuilder, PoolStatus};

// Error handling
pub use crate::error::{Error, Result};

// Task types
pub use crate::types::{
    PoolConfig, ScalingConfig, SlotConfig, SlotId, SlotMode, SlotRecord, SlotState, TaskFilter,
    TaskId, TaskRecord, TaskResult, TaskState,
};

// Chain execution
pub use crate::chain::{ChainOptions, ChainResult, ChainStatus, ChainStep, execute_chain};
