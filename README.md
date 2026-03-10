# claude-wrapper

A comprehensive Rust tooling suite for the Claude Code CLI: type-safe wrapper, worker pool orchestration, and MCP server.

[![Crates.io](https://img.shields.io/crates/v/claude-wrapper.svg)](https://crates.io/crates/claude-wrapper)
[![Documentation](https://docs.rs/claude-wrapper/badge.svg)](https://docs.rs/claude-wrapper)
[![CI](https://github.com/joshrotenberg/claude-wrapper/actions/workflows/ci.yml/badge.svg)](https://github.com/joshrotenberg/claude-wrapper/actions/workflows/ci.yml)
[![License](https://img.shields.io/crates/l/claude-wrapper.svg)](LICENSE-MIT)

## Workspace Overview

Three complementary crates:

```
┌─────────────────────────────────────────────────┐
│  Your Application or Interactive Claude Session │
└────────────────────┬────────────────────────────┘
                     │
                     │ MCP: claude-pool-server
                     ▼
      ┌──────────────────────────────────┐
      │      claude-pool (library)       │
      │  • Task submission & routing     │
      │  • Worker pool (N workers)       │
      │  • Budget tracking               │
      │  • Chains, fan-out, skills       │
      │  • Worktree isolation            │
      └────────────┬─────────────────────┘
                   │
          ┌────────┼────────┐
          ▼        ▼        ▼
      Worker-0 Worker-1 Worker-N
      (Claude CLI instances)
          │        │        │
         Uses: claude-wrapper (CLI wrapper)
```

| Crate | Purpose | Docs |
|-------|---------|------|
| **claude-wrapper** | Type-safe CLI wrapper with builder pattern | [docs.rs](https://docs.rs/claude-wrapper) |
| **claude-pool** | Worker pool, orchestration, budget control | [docs.rs](https://docs.rs/claude-pool) |
| **claude-pool-server** | MCP server exposing the pool | [docs.rs](https://docs.rs/claude-pool-server) |

---

## Quick Start

### 1. Use the CLI wrapper in your app

```rust
use claude_wrapper::{Claude, QueryCommand};

#[tokio::main]
async fn main() -> claude_wrapper::Result<()> {
    let claude = Claude::builder().build()?;
    let output = QueryCommand::new("explain this error")
        .model("sonnet")
        .execute(&claude)
        .await?;
    println!("{}", output.stdout);
    Ok(())
}
```

### 2. Use a worker pool in your app

```rust
use claude_pool::Pool;

#[tokio::main]
async fn main() -> claude_pool::Result<()> {
    let claude = Claude::builder().build()?;
    let pool = Pool::builder(claude).workers(4).build().await?;

    let result = pool.run("write a haiku about rust").await?;
    println!("{}", result.output);

    pool.drain().await?;
    Ok(())
}
```

### 3. Launch the MCP server

```bash
claude-pool-server -n 4 --budget-usd 10.0 --model sonnet
# Stdio transport. Add to .mcp.json and use from Claude.
```

---

## claude-wrapper: Type-Safe CLI Wrapper

Invoke the `claude` CLI programmatically with a builder pattern. Same philosophy as [docker-wrapper](https://crates.io/crates/docker-wrapper).

### QueryCommand

Full coverage of `claude -p` (print mode) options:

```rust
let output = QueryCommand::new("implement the feature in TASK.md")
    .model("sonnet")
    .system_prompt("You are a Rust expert")
    .output_format(OutputFormat::Json)
    .max_budget_usd(1.00)
    .permission_mode(PermissionMode::AcceptEdits)
    .allowed_tools(["Bash", "Read", "Edit", "Write"])
    .mcp_config("/tmp/project/.mcp.json")
    .effort(Effort::High)
    .max_turns(10)
    .no_session_persistence()
    .execute(&claude)
    .await?;
```

### MCP Commands

```rust
// List servers
McpListCommand::new().execute(&claude).await?;

// Add HTTP server
McpAddCommand::new("sentry", "https://mcp.sentry.dev/mcp")
    .transport("http")
    .execute(&claude).await?;

// Add stdio server
McpAddCommand::new("my-tool", "npx")
    .server_args(["my-mcp-server"])
    .env("API_KEY", "xxx")
    .execute(&claude).await?;
```

### MCP Config Builder

Generate `.mcp.json` files:

```rust
use claude_wrapper::McpConfigBuilder;

McpConfigBuilder::new()
    .http_server("hub", "http://127.0.0.1:9090")
    .stdio_server("tool", "npx", ["my-server"])
    .write_to("/tmp/my-project/.mcp.json")?;
```

### Streaming

For real-time NDJSON events:

```rust
use claude_wrapper::streaming::stream_query;

let output = stream_query(&claude, &cmd, |event| {
    if event.is_result() {
        println!("Result: {}", event.result_text().unwrap_or(""));
    }
}).await?;
```

### All QueryCommand Options

| Method | CLI Flag | Description |
|--------|----------|-------------|
| `model()` | `--model` | Model alias or full ID |
| `system_prompt()` | `--system-prompt` | Replace default system prompt |
| `append_system_prompt()` | `--append-system-prompt` | Append to system prompt |
| `output_format()` | `--output-format` | text, json, stream-json |
| `max_budget_usd()` | `--max-budget-usd` | Spending cap |
| `permission_mode()` | `--permission-mode` | default, acceptEdits, bypassPermissions, plan, auto |
| `allowed_tools()` | `--allowed-tools` | Tool permission allow list |
| `disallowed_tools()` | `--disallowed-tools` | Tool permission deny list |
| `tools()` | `--tools` | Restrict available tools |
| `mcp_config()` | `--mcp-config` | MCP server config file |
| `strict_mcp_config()` | `--strict-mcp-config` | Only use MCP from config |
| `add_dir()` | `--add-dir` | Additional accessible directories |
| `effort()` | `--effort` | low, medium, high |
| `max_turns()` | `--max-turns` | Conversation turn limit |
| `json_schema()` | `--json-schema` | Structured output validation |
| `agent()` | `--agent` | Agent for session |
| `agents_json()` | `--agents` | Custom agents JSON |
| `continue_session()` | `--continue` | Resume most recent |
| `resume()` | `--resume` | Resume by session ID |
| `session_id()` | `--session-id` | Use specific session ID |
| `fork_session()` | `--fork-session` | Fork when resuming |
| `fallback_model()` | `--fallback-model` | Fallback model |
| `no_session_persistence()` | `--no-session-persistence` | Don't save session |
| `dangerously_skip_permissions()` | `--dangerously-skip-permissions` | Bypass permissions |
| `file()` | `--file` | File resources to download |
| `input_format()` | `--input-format` | text or stream-json |
| `include_partial_messages()` | `--include-partial-messages` | Partial chunks |
| `settings()` | `--settings` | Settings JSON file |

---

## claude-pool: Worker Pool & Orchestration

A library for managing N Claude CLI workers with task routing, budget tracking, and composable execution patterns.

### Key Features

- **Worker pool**: Spawn N Claude instances, route tasks by load/availability
- **Synchronous tasks**: `pool.run(prompt)` — blocks until complete
- **Asynchronous tasks**: `pool.submit(prompt)` → task ID → `pool.result(id)` later
- **Parallel fan-out**: `pool.fan_out([prompt1, prompt2, ...])` — execute all at once
- **Sequential chains**: `pool.execute_chain(steps)` with step-by-step progress and failure policies (retry, skip, abort)
- **Budget control**: Per-pool and per-worker caps; track spend atomically
- **Worker identity**: Name, role, description fields for context and coordination
- **Shared context**: `pool.context_set(key, value)` — inject into all worker system prompts
- **Worktree isolation**: Optional Git worktree per worker for safe, isolated execution
- **Skills system**: Register reusable prompts/templates with argument validation

### Usage: Pool Builder

```rust
use claude_pool::{Pool, GlobalWorkerConfig, Effort};

let claude = Claude::builder().build()?;

let pool = Pool::builder(claude)
    .workers(4)
    .config(
        GlobalWorkerConfig::default()
            .with_model("sonnet")
            .with_effort(Effort::Medium)
            .with_budget_usd(50.0)
            .with_permission_mode(PermissionMode::Plan)
    )
    .build()
    .await?;

// Single task (sync)
let result = pool.run("fix the bug in main.rs").await?;
println!("Output:\n{}", result.output);
```

### Async Task Submission

```rust
// Submit without blocking
let task_id = pool.submit("long-running task").await?;

// Do other work...

// Poll for result later
let result = pool.result(&task_id).await??;
```

### Parallel Fan-Out

```rust
let prompts = [
    "write a poem",
    "write a haiku",
    "write a limerick",
];

let results = pool.fan_out(&prompts).await?;
for result in results {
    println!("{}", result.output);
}
```

### Sequential Chains with Failure Policies

```rust
use claude_pool::{ChainStep, StepFailurePolicy, execute_chain};

let steps = vec![
    ChainStep::new("analyze the error: file not found"),
    ChainStep::new("write a fix"),
    ChainStep::new("write unit tests"),
];

let result = execute_chain(&pool, steps, ChainOptions::default()).await?;
println!("Chain result: {}", result.final_output);
```

### Shared Context

```rust
pool.context_set("language", "rust").await?;
pool.context_set("framework", "tokio").await?;
pool.context_set("style", "idiomatic").await?;

// All workers now see these in their system prompts
```

### Skills Registry

Skills are templates for reusable task patterns. Register them with the MCP server:

```rust
use claude_pool::{SkillRegistry, Skill, SkillArgument};

let mut registry = SkillRegistry::new();
registry.register(Skill {
    name: "code_review".to_string(),
    description: "Review code for bugs and style".to_string(),
    prompt: "Review the code at {path} for bugs and style".to_string(),
    arguments: vec![
        SkillArgument {
            name: "path".to_string(),
            description: "Path to code file".to_string(),
            required: true,
        }
    ],
    config: None,
});
```

Skills are referenced via the `pool_skill_run` MCP tool when the server is running.

---

## claude-pool-server: MCP Server

A standalone binary that exposes `claude-pool` as an MCP server over stdio. Add to your `.mcp.json` and call from an interactive Claude session.

### Installation & Running

```bash
cargo install claude-pool-server
claude-pool-server -n 4 --budget-usd 10.0 --model sonnet --permission-mode plan
```

### CLI Flags

| Flag | Description |
|------|-------------|
| `-n, --workers N` | Number of workers (default: 2) |
| `--model MODEL` | Default model for all workers |
| `--effort LEVEL` | Default effort: min, low, medium, high, max |
| `--budget-usd AMOUNT` | Total budget cap |
| `--system-prompt TEXT` | System prompt for all workers |
| `--permission-mode MODE` | default, acceptEdits, bypassPermissions, plan, auto |
| `-w, --worktree` | Enable Git worktree isolation |
| `--no-builtins` | Disable built-in skills |

### Add to `.mcp.json`

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

### MCP Tools

All tools are async and return JSON results:

```
pool_run         → Submit task, wait for result
pool_submit      → Submit async (returns task ID)
pool_result      → Get result by task ID
pool_fan_out     → Execute multiple prompts in parallel
pool_chain       → Execute sequential pipeline with failure policies
pool_submit_chain → Submit chain for async execution
pool_chain_result → Get chain progress and results
pool_status      → Get pool status (workers, tasks, spend)
pool_drain       → Graceful shutdown
pool_cancel      → Cancel a pending or running task

context_set      → Inject key-value pair into worker system prompts
context_get      → Retrieve context value
context_list     → List all context keys
context_delete   → Remove context key

pool_skill_run   → Execute a skill by name
```

### MCP Resources

Access pool state and task details:

```
pool://status              → Current pool state (workers, tasks, spend)
pool://workers             → List all workers
pool://budget              → Budget breakdown (total, spent, remaining)
pool://context             → Current context key-value pairs
pool://workers/{id}        → Get a single worker by ID
pool://results/{task_id}   → Get a single task result
pool://chains/{chain_id}   → Get chain progress and results
```

### Example: From Claude

```
You are working with a claude-pool MCP server (4 workers, $10 budget).

Use pool/fan-out to parallelize:
> @mcp pool/fan-out prompts:["write test for fn A", "write test for fn B", "write test for fn C"]

Then chain results:
> @mcp pool/chain steps:[{"prompt": "review the tests"}, {"prompt": "refactor for clarity"}]

Check spend:
> @mcp pool/status
```

---

## Features

| Feature | Default | Description |
|---------|---------|-------------|
| `json` | Yes | JSON output parsing, `execute_json()`, streaming events |
| `tempfile` | Yes | Temporary file support for MCP config |

---

## Status

**Stable and actively maintained.**

All three crates are production-ready with comprehensive test coverage (85+ tests). Recent releases add chain execution, failure policies, worker identity, worktree isolation, and resource templates.

### Implemented

- **claude-wrapper**: Full CLI surface (28 options), all subcommands, MCP management, streaming
- **claude-pool**: Worker pool, task submission (sync/async), fan-out, chains, skills, context injection, budget tracking, worktree isolation
- **claude-pool-server**: MCP binary with all pool tools and resources, CLI configuration

### Not Yet Planned

- Interactive/REPL mode (print mode only)
- Direct Anthropic API calls (use [anthropic SDK](https://crates.io/crates/anthropic))

---

## Scope

### Will Do

- Maintain compatibility with Claude CLI versions
- Add new pool features (scheduling, circuit breakers, metrics)
- Improve documentation and examples
- Performance optimization for large worker pools

### Won't Do

- Claude Code IDE integration (IDE features out of scope)
- Conversation management or prompt engineering
- Token counting beyond what the CLI returns
- Interactive sessions (print mode only)

---

## Release & Testing

```bash
# Pre-commit checks
cargo fmt --all -- --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --lib --all-features
cargo test --test '*' --all-features

# Doc tests
cargo test --doc --all-features

# Workspace release (in dependency order)
cargo publish --dry-run -p claude-wrapper
cargo publish --dry-run -p claude-pool
cargo publish --dry-run -p claude-pool-server
```

---

## License

MIT OR Apache-2.0
