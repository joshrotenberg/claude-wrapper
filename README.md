# claude-wrapper

Rust tooling suite for the Claude Code CLI built around the coordinator/worker model.

[![Crates.io](https://img.shields.io/crates/v/claude-wrapper.svg)](https://crates.io/crates/claude-wrapper)
[![Documentation](https://docs.rs/claude-wrapper/badge.svg)](https://docs.rs/claude-wrapper)
[![CI](https://github.com/joshrotenberg/claude-wrapper/actions/workflows/ci.yml/badge.svg)](https://github.com/joshrotenberg/claude-wrapper/actions/workflows/ci.yml)
[![License](https://img.shields.io/crates/l/claude-wrapper.svg)](LICENSE-MIT)

## Coordinator/Worker Model

**claude-pool** implements coordinator/worker orchestration for Claude Code. A
**coordinator** -- an interactive Claude session with the pool in its MCP config --
schedules work, dispatches tasks to **worker** slots, monitors results, reviews
outputs, and decides what merges. Workers are isolated Claude instances that
execute one task at a time and return.

This is measured parallelism under human control. The human sits at the
coordinator level and decides what offloads, when, and how. Not a
leave-it-running daemon or full automation platform.

The coordinator follows a repeating rhythm: **dispatch** tasks, **monitor**
results (tick loop), **review** output, **merge** what passes. Chains compress
this into one dispatch/monitor cycle. Fan-outs run monitor in parallel.

### Key Properties

- **Session-scoped**: Slots live only as long as your process. No external state.
- **Human-in-the-loop**: The coordinator reviews and approves worker output.
- **Selective**: Choose what offloads -- chains, fan-outs, or single tasks.
- **MCP-native**: Expose the pool as an MCP server and use it directly from Claude Code.

## The Three Crates

```
+---------------------------------------------------+
|  Your Application or Interactive Claude Session    |
+------------------------+--------------------------+
                         |
                         |  MCP: claude-pool-mcp
                         v
      +--------------------------------------+
      |      claude-pool (library)           |
      |  * Task submission & routing         |
      |  * Slot pool (N slots)               |
      |  * Budget tracking                   |
      |  * Chains, fan-out, auto-routing     |
      |  * Worktree isolation                |
      +----------------+--------------------+
                        |
              +---------+---------+
              v         v         v
          Slot-0    Slot-1    Slot-N
          (Claude CLI instances)
              |         |         |
             Uses: claude-wrapper (CLI wrapper)
```

| Crate | Purpose | Docs |
|-------|---------|------|
| **[claude-wrapper](crates/claude-wrapper/)** | Type-safe Rust interface to Claude Code CLI | [README](crates/claude-wrapper/README.md) |
| **[claude-pool](crates/claude-pool/)** | Coordinator/worker orchestration | [README](crates/claude-pool/README.md) |
| **[claude-pool-mcp](crates/claude-pool-mcp/)** | MCP server exposing pool as tools | [README](crates/claude-pool-mcp/README.md) |

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

### 2. Use a slot pool in your app

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

### 3. Use as an MCP server from Claude Code

Add to your `.mcp.json`:

```json
{
  "mcpServers": {
    "claude-pool": {
      "command": "cargo",
      "args": ["run", "-p", "claude-pool-mcp", "--", "-n", "4", "--model", "sonnet"]
    }
  }
}
```

Then use `pool_run`, `pool_fan_out`, `pool_chain`, `pool_auto`, and 27 other tools directly from Claude Code. See the [coordinator skill](crates/claude-pool-mcp/skills/pool-coordinator/SKILL.md) for tool selection guidance.

## Features

- **claude-wrapper**: Type-safe CLI wrapper with full option coverage, MCP server management, plugin management, streaming NDJSON events, session management
- **claude-pool**: Multi-slot coordination, synchronous/async task execution, parallel fan-out, sequential chains with failure policies, auto-routing (LLM picks single/parallel/chain), budget control, shared context injection, worktree isolation, review gates
- **claude-pool-mcp**: 31-tool MCP server, stdio transport, configurable via CLI flags

## Installation

```bash
# Library: type-safe CLI wrapper
cargo add claude-wrapper

# Library: slot pool orchestration
cargo add claude-pool
```

## Development & Testing

```bash
cargo fmt --all -- --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --lib --all-features
cargo test --doc --all-features

# Integration tests (requires fake-claude binary)
cargo test --test pool_integration --test auto_route_tests -p claude-pool -- --ignored

# Live routing accuracy test (requires real claude binary)
cargo test --test route_stress -p claude-pool -- --ignored
```

## License

MIT OR Apache-2.0
