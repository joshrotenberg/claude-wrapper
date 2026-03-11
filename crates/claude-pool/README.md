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

## Skills System

Skills are reusable prompt templates that define how to approach a task. The pool discovers and references them by name in chains or direct calls.

### SKILL.md Format

Skills follow the [Agent Skills](https://agentskills.io) standard. Each skill lives in its own directory with a `SKILL.md` file:

```
.claude-pool/skills/
  code_review/
    SKILL.md          # Required: frontmatter + prompt
    scripts/          # Optional: bundled scripts
      lint.sh
    templates/        # Optional: templates
      report.md
    examples/         # Optional: examples
      input.py
```

The `SKILL.md` file contains YAML frontmatter followed by the prompt body:

```yaml
---
name: code_review
description: Review code for bugs and style issues
argument-hint: "<path> [criteria]"
allowed-tools: Read, Grep, Glob, Bash
metadata:
  arguments:
    - name: path
      description: File path to review
      required: true
    - name: criteria
      description: What to focus on (bugs, style, performance)
      required: false
---

Review the code at {path} for the following criteria: {criteria}

Report issues found with severity and suggestions.
```

Standard fields (`argument-hint`, `allowed-tools`) live at the top level.
Pool-specific extensions (`scope`, `arguments`, `config`) live under `metadata`.
Arguments are available as `{arg_name}` or `$ARGUMENTS` / `$0` / `$1` in the prompt.

### Skill Resolution

Skills are discovered in priority order (first match wins):

1. **Runtime skills** - Added via code (ephemeral, lost on restart)
2. **Project skills** - Loaded from `.claude-pool/skills/` (checked into repo)
3. **Global skills** - Loaded from `~/.claude-pool/skills/` (user-wide)
4. **Builtin skills** - Shipped with the pool binary

### CLAUDE_SKILL_DIR Substitution

Skills can reference supporting files using the `${CLAUDE_SKILL_DIR}` variable:

```
Run linting:
bash ${CLAUDE_SKILL_DIR}/scripts/lint.sh .

Generate report from template:
python -c "..." < ${CLAUDE_SKILL_DIR}/templates/report.md
```

The variable resolves to the skill's directory path at render time. Available for project and global skills only (not builtins or runtime skills).

### Using Skills in Chains

Reference skills in chain steps:

```rust
use claude_pool::{ChainStep, StepAction};

let steps = vec![
    ChainStep {
        name: "review".into(),
        action: StepAction::Skill {
            skill: "code_review".into(),
            arguments: [
                ("path", "src/main.rs"),
                ("criteria", "performance"),
            ].iter().map(|(k, v)| (k.to_string(), v.to_string())).collect(),
        },
        config: None,
        failure_policy: Default::default(),
        output_vars: Default::default(),
    },
];
```

### Programmatic Registration

Register skills at runtime:

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
        ],
        config: None,
        scope: Default::default(),
        argument_hint: Some("<path> [criteria]".to_string()),
        skill_dir: None,
    },
    SkillSource::Runtime,
);
```

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

## Quality Gates

The pool supports a human-in-the-loop review workflow. Tasks submitted with
`submit_with_review` transition to `PendingReview` on completion instead of
`Completed`, allowing a coordinator to inspect results before accepting them.

```rust
// Submit a task that requires approval before it's considered done.
let task_id = pool.submit_with_review(
    "refactor the auth module",
    None,           // optional SlotConfig override
    vec![],         // tags
    Some(3),        // max_rejections (default: 3)
).await?;

// ... task runs, completes, enters PendingReview ...

// Inspect the result.
let result = pool.result(&task_id).await?;

// Approve: transitions PendingReview -> Completed.
pool.approve_result(&task_id).await?;

// Or reject with feedback: re-queues the task with feedback appended
// to the original prompt. Fails after max_rejections.
pool.reject_result(&task_id, "missing error handling for timeout case").await?;
```

### Via MCP tools

The same workflow is available through the MCP server:

- `pool_submit_with_review` -- submit a task requiring approval
- `pool_approve_result` -- accept the result
- `pool_reject_result` -- reject with feedback, task re-runs

### Task states

```
Pending -> Running -> PendingReview -> Completed  (approved)
                          |
                          +-> Running  (rejected, re-queued with feedback)
                          |
                          +-> Failed   (max rejections reached)
```

Rejection appends feedback to the original prompt so the slot sees what went
wrong and can address it on the next attempt.

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

## Feature Flags

Currently no optional features. The crate includes full functionality by default.

Future features may include:
- `redis-store` - Redis backend for distributed pool state
- `prometheus` - Metrics export for monitoring

## API Documentation

For detailed API documentation, see [docs.rs/claude-pool](https://docs.rs/claude-pool).

## Testing

Requires the `claude` CLI binary:

```bash
cargo test --lib --all-features
```

## License

MIT OR Apache-2.0
