# claude-pool

Slot pool orchestration library for Claude CLI

[![Crates.io](https://img.shields.io/crates/v/claude-pool.svg)](https://crates.io/crates/claude-pool)
[![Documentation](https://docs.rs/claude-pool/badge.svg)](https://docs.rs/claude-pool)
[![CI](https://github.com/joshrotenberg/claude-wrapper/actions/workflows/ci.yml/badge.svg)](https://github.com/joshrotenberg/claude-wrapper/actions/workflows/ci.yml)
[![License](https://img.shields.io/crates/l/claude-pool.svg)](LICENSE-MIT)

## Overview

`claude-pool` manages N Claude CLI slots behind a unified interface. A coordinator (typically an interactive Claude session) submits work, and the pool routes tasks by availability, tracks budgets, and handles slot lifecycle and session management.

Perfect for:
- Scaling Claude work across multiple slots
- Budget-aware task distribution
- Parallel and sequential task orchestration
- Slot isolation with optional Git worktrees

## Architecture

```
Coordinator (your app or interactive session)
  │
  ├─ pool.run("task")           → synchronous
  ├─ pool.submit("task")        → async, returns task ID
  ├─ pool.fan_out([tasks])      → parallel execution
  └─ execute_chain(steps)       → sequential pipeline
        │
        ├── Pool (task queue, context, budget)
        │
        ├── Slot-0 (Claude instance)
        ├── Slot-1 (Claude instance)
        └── Slot-N (Claude instance)
```

## Installation

```bash
cargo add claude-pool
```

Requires: `claude-wrapper` (included as dependency)

## Quick Start

```rust
use claude_pool::Pool;
use claude_wrapper::Claude;

#[tokio::main]
async fn main() -> claude_pool::Result<()> {
    let claude = Claude::builder().build()?;
    let pool = Pool::builder(claude)
        .slots(4)
        .build()
        .await?;

    let result = pool.run("write a haiku about rust").await?;
    println!("{}", result.output);

    pool.drain().await?;
    Ok(())
}
```

## Core Concepts

### Synchronous vs Asynchronous Tasks

**Synchronous (blocking):**
```rust
let result = pool.run("your task here").await?;
println!("{}", result.output);
```

**Asynchronous (non-blocking):**
```rust
let task_id = pool.submit("long-running task").await?;
// Do other work...
let result = pool.result(&task_id).await??;
```

### Budget Control

Track and limit spending:

```rust
let pool = Pool::builder(claude)
    .slots(4)
    .config(
        PoolConfig::default()
            .with_budget_usd(50.0)  // Pool-level cap
    )
    .build()
    .await?;
```

Budget is tracked atomically per task. When the pool reaches its cap, subsequent tasks are rejected.

### Slot Identity

Each slot has metadata for coordination:

```rust
pool.configure_slot("slot-0", "analyzer", "Code review specialist")
    .await?;
pool.configure_slot("slot-1", "writer", "Code generation specialist")
    .await?;
```

Access slot info:
```rust
let status = pool.status().await?;
for slot in status.slots {
    println!("{}: {} ({} active)", slot.id, slot.role, slot.busy_tasks);
}
```

### Shared Context

Inject key-value pairs into all slot system prompts:

```rust
pool.context_set("language", "rust").await?;
pool.context_set("framework", "tokio").await?;
pool.context_set("style", "idiomatic").await?;

// All slots now see these in their system prompts
```

Access context:
```rust
let value = pool.context_get("language").await??;
pool.context_delete("framework").await?;
let all = pool.context_list().await?;
```

## Pool Builder Configuration

```rust
use claude_pool::{Pool, PoolConfig, Effort, PermissionMode};

let pool = Pool::builder(claude)
    .slots(8)
    .config(
        PoolConfig::default()
            .with_model("sonnet")
            .with_effort(Effort::High)
            .with_budget_usd(100.0)
            .with_permission_mode(PermissionMode::Plan)
            .with_system_prompt("You are a Rust expert")
            .with_worktree(true)
    )
    .build()
    .await?;
```

Available config options:
- `with_model(name)` - Default model for all slots
- `with_effort(level)` - Effort: Min, Low, Medium, High, Max
- `with_budget_usd(amount)` - Total pool budget
- `with_permission_mode(mode)` - Permission defaults
- `with_system_prompt(text)` - Base system prompt
- `with_worktree(true)` - Enable Git worktree per slot

## Execution Patterns

### Single Task (Synchronous)

```rust
let result = pool.run("fix the bug in main.rs").await?;
println!("Output:\n{}", result.output);
println!("Spend: ${}", result.spend_usd);
```

Result includes:
- `output` - Claude's response
- `spend_usd` - Cost of this task
- `tokens_used` - Input and output tokens

### Async Task Submission

```rust
// Submit and get task ID immediately
let task_id = pool.submit("long-running analysis").await?;

// Do other work...

// Poll for result later
let result = pool.result(&task_id).await??;
```

### Parallel Fan-Out

Execute multiple prompts in parallel, all at once:

```rust
let prompts = vec![
    "write a poem",
    "write a haiku",
    "write a limerick",
];

let results = pool.fan_out(&prompts).await?;
for (i, result) in results.iter().enumerate() {
    println!("Result {}: {}", i, result.output);
}
```

All tasks run concurrently. Returns when all complete (or timeout).

### Sequential Chains with Failure Policies

Execute steps in order, with control over failures:

```rust
use claude_pool::{ChainStep, StepAction, StepFailurePolicy};

let steps = vec![
    ChainStep {
        name: "analyze".into(),
        action: StepAction::Prompt { prompt: "analyze the error".into() },
        config: None,
        failure_policy: StepFailurePolicy::default(),
        output_vars: Default::default(),
    },
    ChainStep {
        name: "fix".into(),
        action: StepAction::Prompt { prompt: "write a fix based on {previous_output}".into() },
        config: None,
        failure_policy: StepFailurePolicy { retries: 2, recovery_prompt: None },
        output_vars: Default::default(),
    },
];

let task_id = pool.submit_chain(steps, &skills, ChainOptions::default()).await?;
let result = pool.result(&task_id).await?;
```

Failure policies:
- **retries** - Number of retries before failing (default: 0)
- **recovery_prompt** - Optional prompt to run on failure instead of aborting

Access chain progress:
```rust
let progress = pool.chain_result(&chain_id).await?;
for step in progress.steps {
    println!("{}: {}", step.name, step.status);
}
```

## Skills Registry

Register reusable task patterns with argument validation:

```rust
use claude_pool::{Skill, SkillArgument, SkillRegistry, SkillSource};

let mut registry = SkillRegistry::new();
registry.register(
    Skill {
        name: "code_review".to_string(),
        description: "Review code for bugs and style".to_string(),
        prompt: "Review the code at {path} for {criteria}".to_string(),
        arguments: vec![
            SkillArgument {
                name: "path".to_string(),
                description: "File to review".to_string(),
                required: true,
            },
            SkillArgument {
                name: "criteria".to_string(),
                description: "What to focus on (bugs, style, performance)".to_string(),
                required: false,
            },
        ],
        config: None,
        scope: Default::default(),
    },
    SkillSource::Runtime,
);
```

Skills can be triggered via the MCP server or called programmatically.

## Worktree Isolation

Enable optional Git worktree per slot for safe, isolated execution:

```rust
let pool = Pool::builder(claude)
    .slots(4)
    .config(
        PoolConfig::default()
            .with_worktree(true)
    )
    .build()
    .await?;
```

Each slot gets an isolated worktree:
- Independent filesystem
- Safe for parallel edits
- Cleanup on drain

Benefits:
- Parallel file edits without conflicts
- Isolated git state
- Safe cleanup

## Slot Lifecycle

### Spawning

Slots are created during `build()` and remain alive until `drain()`.

### Session Resumption

Slots automatically resume sessions if available, reducing startup cost.

### Graceful Shutdown

```rust
let summary = pool.drain().await?;
println!("Processed {} tasks", summary.total_tasks);
println!("Total spend: ${}", summary.total_spend_usd);
println!("Errors: {}", summary.error_count);
```

All pending tasks are cancelled. Active tasks complete gracefully.

## Status & Monitoring

Get current pool state:

```rust
let status = pool.status().await?;
println!("Slots: {}", status.slots.len());
println!("Active tasks: {}", status.active_tasks);
println!("Budget: ${} / ${}", status.spend_usd, status.budget_usd);
println!("Remaining: ${}", status.budget_usd - status.spend_usd);
```

Status includes:
- Slot list with ID, status, and active task count
- Active and pending task counts
- Total spend and budget
- Budget remaining

## Error Handling

All operations return `Result<T>`:

```rust
use claude_pool::Error;

match pool.run("task").await {
    Ok(result) => println!("{}", result.output),
    Err(Error::TaskFailed(msg)) => eprintln!("Task error: {}", msg),
    Err(Error::BudgetExceeded) => eprintln!("Out of budget"),
    Err(Error::NoSlotsAvailable) => eprintln!("All slots busy"),
    Err(e) => eprintln!("Other error: {}", e),
}
```

Common errors:
- `TaskFailed` - Task execution failed
- `BudgetExceeded` - Pool exceeded spending cap
- `NoSlotsAvailable` - All slots busy/offline
- `TaskNotFound` - Invalid task ID

## Testing

Requires the `claude` CLI binary:

```bash
cargo test --lib --all-features
```

## License

MIT OR Apache-2.0
