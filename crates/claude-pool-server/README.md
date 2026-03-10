# claude-pool-server

MCP server binary for managing Claude worker pools

[![Crates.io](https://img.shields.io/crates/v/claude-pool-server.svg)](https://crates.io/crates/claude-pool-server)
[![Documentation](https://docs.rs/claude-pool-server/badge.svg)](https://docs.rs/claude-pool-server)
[![CI](https://github.com/joshrotenberg/claude-wrapper/actions/workflows/ci.yml/badge.svg)](https://github.com/joshrotenberg/claude-wrapper/actions/workflows/ci.yml)
[![License](https://img.shields.io/crates/l/claude-pool-server.svg)](LICENSE-MIT)

## Overview

`claude-pool-server` is a standalone binary that exposes `claude-pool` as an MCP server over stdio transport. Add it to your `.mcp.json` and interact with worker pools directly from interactive Claude sessions.

Perfect for:
- Delegating work to background Claude workers
- Scaling tasks in parallel
- Long-running analysis jobs
- Budget-aware task orchestration

## Installation

### From crates.io

```bash
cargo install claude-pool-server
```

### From source

```bash
git clone https://github.com/joshrotenberg/claude-wrapper
cargo install --path crates/claude-pool-server
```

## Quick Start

```bash
# Start server with 4 workers, $10 budget, Sonnet model
claude-pool-server -n 4 --budget-usd 10.0 --model sonnet
```

Add to `.mcp.json`:

```json
{
  "mcpServers": {
    "claude-pool": {
      "command": "claude-pool-server",
      "args": ["-n", "4", "--budget-usd", "10.0", "--model", "sonnet"]
    }
  }
}
```

Reload your Claude Code session to enable the server.

## CLI Flags

### Worker Configuration

| Flag | Type | Default | Description |
|------|------|---------|-------------|
| `-n, --workers` | `NUMBER` | 2 | Number of workers to spawn |
| `--model` | `STRING` | - | Default model for all workers (e.g. `sonnet`, `opus`) |

### Default Settings

| Flag | Type | Description |
|------|------|-------------|
| `--effort` | `LEVEL` | Default effort: min, low, medium, high, max |
| `--system-prompt` | `TEXT` | System prompt for all workers |
| `--permission-mode` | `MODE` | default, acceptEdits, bypassPermissions, plan, auto (default: plan) |

### Budget Control

| Flag | Type | Description |
|------|------|-------------|
| `--budget-usd` | `AMOUNT` | Total budget cap in USD (e.g. 10.0) |

### Advanced Options

| Flag | Type | Description |
|------|------|-------------|
| `-w, --worktree` | - | Enable Git worktree isolation per worker |
| `--no-builtins` | - | Disable built-in skills (code_review, refactor, etc.) |

### Examples

```bash
# 8 workers, $50 budget, high effort
claude-pool-server -n 8 --budget-usd 50 --effort high

# Lightweight setup with worktree isolation
claude-pool-server -n 2 --budget-usd 5.0 --worktree

# Custom system prompt
claude-pool-server -n 4 --system-prompt "You are a Rust expert. Focus on idiomatic code."

# Plan mode (no auto-execution, requires approval)
claude-pool-server -n 4 --permission-mode plan
```

## .mcp.json Configuration

Full example with all options:

```json
{
  "mcpServers": {
    "claude-pool": {
      "command": "claude-pool-server",
      "args": [
        "-n", "4",
        "--budget-usd", "20.0",
        "--model", "sonnet",
        "--effort", "high",
        "--permission-mode", "plan"
      ],
      "env": {
        "RUST_LOG": "info"
      }
    }
  }
}
```

## MCP Tools

All tools are async and return JSON results.

### Task Execution

#### pool_run

Execute a task synchronously (wait for completion):

```
Prompt: Execute this task and wait for result
@mcp pool_run prompt: "write a haiku about rust"

Returns: { output: "...", spend_usd: 0.15, tokens_used: { input: 50, output: 20 } }
```

Parameters:
- `prompt` (string, required) - The task to execute

Response includes:
- `output` - Claude's response
- `spend_usd` - Cost of this task
- `tokens_used` - Input and output tokens

#### pool_submit

Submit a task asynchronously (returns immediately with task ID):

```
@mcp pool_submit prompt: "long-running analysis"

Returns: { task_id: "task_abc123" }
```

Parameters:
- `prompt` (string, required) - The task to submit

#### pool_result

Retrieve result by task ID:

```
@mcp pool_result task_id: "task_abc123"

Returns: { output: "...", spend_usd: 0.15, status: "completed" }
```

Parameters:
- `task_id` (string, required) - Task ID from pool_submit

#### pool_cancel

Cancel a pending or running task:

```
@mcp pool_cancel task_id: "task_abc123"

Returns: { status: "cancelled" }
```

Parameters:
- `task_id` (string, required) - Task ID to cancel

### Parallel Execution

#### pool_fan_out

Execute multiple prompts in parallel:

```
@mcp pool_fan_out prompts: ["write a poem", "write a haiku", "write a limerick"]

Returns: [
  { output: "Roses are red...", spend_usd: 0.10 },
  { output: "A haiku here...", spend_usd: 0.08 },
  { output: "A limerick...", spend_usd: 0.12 }
]
```

Parameters:
- `prompts` (array of strings, required) - Tasks to execute in parallel

All tasks run concurrently. Returns when all complete.

### Sequential Chains

#### pool_chain

Execute a sequential pipeline synchronously:

```
@mcp pool_chain steps: [
  { prompt: "analyze the error" },
  { prompt: "write a fix" },
  { prompt: "write tests" }
]

Returns: { final_output: "...", steps: [...], total_spend: 0.45 }
```

Parameters:
- `steps` (array, required) - Steps to execute in order
  - `prompt` (string) - The step task
  - `failure_policy` (string, optional) - retry, skip, or abort (default: abort)

#### pool_submit_chain

Submit a chain for async execution:

```
@mcp pool_submit_chain steps: [...]

Returns: { chain_id: "chain_xyz789" }
```

#### pool_chain_result

Get chain progress and results:

```
@mcp pool_chain_result chain_id: "chain_xyz789"

Returns: {
  status: "in_progress",
  completed_steps: 1,
  total_steps: 3,
  current_step: { name: "write a fix", status: "running" },
  steps: [...]
}
```

### Context Management

Share key-value data with all workers (injected into system prompts).

#### context_set

Inject a key-value pair:

```
@mcp context_set key: "language" value: "rust"
@mcp context_set key: "framework" value: "tokio"

Returns: { key: "language", value: "rust" }
```

#### context_get

Retrieve a value:

```
@mcp context_get key: "language"

Returns: { value: "rust" }
```

#### context_list

List all context keys:

```
@mcp context_list

Returns: {
  language: "rust",
  framework: "tokio",
  style: "idiomatic"
}
```

#### context_delete

Remove a context key:

```
@mcp context_delete key: "framework"

Returns: { deleted: true }
```

### Worker Management

#### pool_status

Get current pool state:

```
@mcp pool_status

Returns: {
  workers: [
    { id: "worker-0", status: "idle", active_tasks: 0 },
    { id: "worker-1", status: "busy", active_tasks: 1 }
  ],
  active_tasks: 1,
  pending_tasks: 3,
  spend_usd: 2.50,
  budget_usd: 10.0,
  budget_remaining: 7.50
}
```

#### pool_drain

Graceful shutdown (cancel pending tasks, wait for active tasks):

```
@mcp pool_drain

Returns: {
  total_tasks: 15,
  completed: 12,
  errors: 1,
  total_spend: 3.45
}
```

### Skills

Skills are reusable task templates with argument validation.

#### pool_skill_run

Execute a named skill:

```
@mcp pool_skill_run skill: "code_review" arguments: { path: "src/main.rs", criteria: "bugs" }

Returns: { output: "Review findings...", spend_usd: 0.20 }
```

Parameters:
- `skill` (string, required) - Skill name
- `arguments` (object, optional) - Skill arguments by name

## MCP Resources

Access pool state and task details via resources.

| Resource | Description |
|----------|-------------|
| `pool://status` | Current pool state (workers, tasks, spend) |
| `pool://workers` | List of all workers |
| `pool://workers/{id}` | Single worker details by ID |
| `pool://budget` | Budget info (total, spent, remaining) |
| `pool://context` | All context key-value pairs |
| `pool://results/{task_id}` | Task result by ID |
| `pool://chains/{chain_id}` | Chain progress and results |

## Usage Patterns from Claude

### Example 1: Parallel Code Review

```
I need to review 3 files in parallel.

Use pool/fan-out to review them:
@mcp pool_fan_out prompts: [
  "Review src/main.rs for bugs",
  "Review src/lib.rs for style",
  "Review src/error.rs for clarity"
]

Then summarize:
@mcp pool_run prompt: "Summarize the reviews"
```

### Example 2: Sequential Workflow

```
Let's implement and test a feature:

@mcp pool_chain steps: [
  { prompt: "Implement login feature in src/auth.rs" },
  { prompt: "Write integration tests for authentication" },
  { prompt: "Review code for security issues" }
]
```

### Example 3: Async Long-Running Task

```
Start a long analysis:
@mcp pool_submit prompt: "Analyze entire codebase for refactoring opportunities"

# ... do other work ...

Check progress:
@mcp pool_status

Get result:
@mcp pool_result task_id: "task_abc123"
```

### Example 4: Budget-Aware Work

```
Set context for all workers:
@mcp context_set key: "budget_constraint" value: "optimize for cost"

Then submit work:
@mcp pool_run prompt: "Write a minimal test suite"
```

## Built-in Skills

Pre-registered skills available by default (unless `--no-builtins`):

- **code_review** - Review code for bugs, style, and clarity
- **write_tests** - Generate tests for code
- **refactor** - Refactor code for clarity and performance
- **summarize** - Summarize text or code

Use with `pool_skill_run`.

## Troubleshooting

### Server Won't Start

**"command not found: claude-pool-server"**
- Ensure installation: `cargo install claude-pool-server`
- Or run from source: `cargo run -p claude-pool-server -- ...`

**"claude: command not found"**
- Install Claude CLI: https://claude.ai/download
- Or specify path: needs investigation

### Workers Not Responding

Check worker status:
```
@mcp pool_status
```

If workers are offline, restart the server.

### Budget Exceeded

Tasks are rejected when budget cap is reached:
```
@mcp pool_status
# Check budget_remaining

# Increase budget:
# Restart server with larger --budget-usd
```

### High Latency

Consider:
- Reducing worker count (fewer concurrent tasks)
- Increasing timeout settings
- Using async submission for non-urgent tasks

## Performance Tuning

### Worker Count

- **2 workers** - Light tasks, minimal resource usage
- **4 workers** - Balanced for typical workflows
- **8+ workers** - Heavy parallelization, requires higher budget

### Effort Level

- `low` - Fast responses, fewer resources
- `medium` - Balanced speed and quality (default)
- `high` - Thorough analysis, higher cost

### Worktree Isolation

`--worktree` adds overhead but enables safe parallel file edits.

## License

MIT OR Apache-2.0
