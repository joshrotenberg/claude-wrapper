# claude-pool

> **⚠ DEPRECATED — unmaintained and not published.**
>
> `claude-pool` is kept in the repo for git history but is `publish = false`, excluded from the workspace build, and will not receive updates. The maintained crate in this repo is [`claude-wrapper`](../claude-wrapper/).
>
> Old published versions of `claude-pool` (up to 0.4.0) remain on crates.io and will continue to resolve for existing dependents, but no further releases are planned. To revive this crate, move it from `[workspace] exclude` back into `[workspace] members` in the root `Cargo.toml`.

---

Slot pool orchestration library for Claude CLI.

[![Crates.io](https://img.shields.io/crates/v/claude-pool.svg)](https://crates.io/crates/claude-pool)
[![License](https://img.shields.io/crates/l/claude-pool.svg)](LICENSE-MIT)

## Overview

`claude-pool` manages N Claude CLI slots behind a unified interface. A coordinator (typically an interactive Claude session) submits work, and the pool routes tasks by availability, tracks budgets, and handles slot lifecycle and session management.

## Architecture

```
Coordinator (your app or interactive session)
  |
  +-- pool.run("task")           -> synchronous
  +-- pool.submit("task")        -> async, returns task ID
  +-- pool.fan_out([tasks])      -> parallel execution
  +-- pool.auto("task")          -> LLM picks single/parallel/chain
  +-- pool.submit_chain(steps)   -> sequential pipeline
        |
        +-- Pool (task queue, context, budget)
        |
        +-- Slot-0 (Claude instance)
        +-- Slot-1 (Claude instance)
        +-- Slot-N (Claude instance)
```

## Installation

```bash
cargo add claude-pool
```

## Quick Start

```rust
use claude_pool::Pool;
use claude_wrapper::Claude;

#[tokio::main]
async fn main() -> claude_pool::Result<()> {
    let claude = Claude::builder().build()?;
    let pool = Pool::builder(claude).slots(4).build().await?;

    let result = pool.run("write a haiku about rust").await?;
    println!("{}", result.output);

    pool.drain().await?;
    Ok(())
}
```

## Execution Patterns

### Single Task

```rust
// Synchronous (blocking)
let result = pool.run("fix the bug in main.rs").await?;

// Asynchronous (non-blocking)
let task_id = pool.submit("long-running analysis").await?;
// ... do other work ...
if let Some(result) = pool.result(&task_id).await? {
    println!("{}", result.output);
}
```

### Parallel Fan-Out

```rust
let results = pool.fan_out(&[
    "review file1.rs",
    "review file2.rs",
    "review file3.rs",
]).await?;
```

### Sequential Chains

```rust
use claude_pool::{ChainStep, StepAction, ChainOptions};

let steps = vec![
    ChainStep {
        name: "analyze".into(),
        action: StepAction::Prompt { prompt: "analyze the error".into() },
        config: None,
        failure_policy: Default::default(),
        output_vars: Default::default(),
    },
    ChainStep {
        name: "fix".into(),
        action: StepAction::Prompt {
            prompt: "write a fix based on {previous_output}".into(),
        },
        config: None,
        failure_policy: Default::default(),
        output_vars: Default::default(),
    },
];

let task_id = pool.submit_chain(steps, ChainOptions::default()).await?;
```

### Auto-Routing

Let an LLM classify the task as single, parallel, or chain:

```rust
let result = pool.auto("translate hello into French, Spanish, and German").await?;
// Router decides this is parallel and fans out automatically.
```

## Configuration

```rust
use claude_pool::{Pool, PoolConfig};

let pool = Pool::builder(claude)
    .slots(8)
    .config(PoolConfig {
        model: Some("sonnet".into()),
        budget_microdollars: Some(50_000_000), // $50
        ..Default::default()
    })
    .build()
    .await?;
```

## Features

- **Task execution**: synchronous (`run`), async (`submit`/`result`), parallel (`fan_out`)
- **Chains**: sequential pipelines with `{previous_output}` threading, failure policies, retries
- **Auto-routing**: LLM classifies tasks as single/parallel/chain with structured hints
- **Budget control**: pool-level and per-task spending caps
- **Shared context**: inject key-value pairs into all slot system prompts
- **Review gates**: `submit_with_review` / `approve_result` / `reject_result`
- **Worktree isolation**: optional git worktree per slot or per chain
- **Messaging**: inter-slot messaging with broadcast support
- **Scaling**: dynamic slot scaling (`scale_up`, `scale_down`, `set_target_slots`)
- **Supervisor**: background health monitoring with automatic slot restarts
- **Storage**: `InMemoryStore` (default) or `JsonFileStore` for persistence

## Examples

```bash
cargo run -p claude-pool --example basic_pool     # Single task, status, drain
cargo run -p claude-pool --example fan_out         # Parallel execution
cargo run -p claude-pool --example chain           # Sequential pipeline
cargo run -p claude-pool --example auto_route      # Auto-routing demonstration
cargo run -p claude-pool --example route_stress    # Routing accuracy test
```

## License

MIT OR Apache-2.0
