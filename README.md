# claude-wrapper

Rust tooling suite for the Claude Code CLI built around the coordinator/worker model.

[![Crates.io](https://img.shields.io/crates/v/claude-wrapper.svg)](https://crates.io/crates/claude-wrapper)
[![Documentation](https://docs.rs/claude-wrapper/badge.svg)](https://docs.rs/claude-wrapper)
[![CI](https://github.com/joshrotenberg/claude-wrapper/actions/workflows/ci.yml/badge.svg)](https://github.com/joshrotenberg/claude-wrapper/actions/workflows/ci.yml)
[![License](https://img.shields.io/crates/l/claude-wrapper.svg)](LICENSE-MIT)

## Coordinator/Worker Model

**claude-pool** implements coordinator/worker orchestration for Claude Code. A
**coordinator** — an interactive Claude session with the pool in its MCP config —
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
- **Selective**: Choose what offloads — chains, fan-outs, or single tasks.
- **MCP-native**: Expose the pool as an MCP server and use it directly from Claude Code.

## The Three Crates

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
      │  • Slot pool (N slots)       │
      │  • Budget tracking               │
      │  • Chains, fan-out, skills       │
      │  • Worktree isolation            │
      └────────────┬─────────────────────┘
                   │
          ┌────────┼────────┐
          ▼        ▼        ▼
      Slot-0 Slot-1 Slot-N
      (Claude CLI instances)
          │        │        │
         Uses: claude-wrapper (CLI wrapper)
```

| Crate | Purpose | Docs |
|-------|---------|------|
| **[claude-wrapper](crates/claude-wrapper/)** | Type-safe Rust interface to Claude Code CLI | [README](crates/claude-wrapper/README.md) ⟡ [docs.rs](https://docs.rs/claude-wrapper) |
| **[claude-pool](crates/claude-pool/)** | Coordinator/worker orchestration | [README](crates/claude-pool/README.md) ⟡ [docs.rs](https://docs.rs/claude-pool) |
| **[claude-pool-server](crates/claude-pool-server/)** | MCP + REST server | [README](crates/claude-pool-server/README.md) ⟡ [docs.rs](https://docs.rs/claude-pool-server) |

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

### 3. Launch the MCP server

```bash
claude-pool-server -n 4 --budget-usd 10.0 --model sonnet
# Stdio transport. Add to .mcp.json and use from Claude.
```

## Features

- **claude-wrapper**: Type-safe CLI wrapper with full option coverage (28 options), MCP server management, plugin management, streaming NDJSON events, session management
- **claude-pool**: Multi-slot coordination, synchronous/async task execution, parallel fan-out, sequential chains with failure policies, budget control, shared context injection, optional worktree isolation, reusable skills registry
- **claude-pool-server**: Standalone MCP server binary, configurable via CLI flags, full pool tool exposure, MCP resources for state inspection

## Installation

```bash
# Library: type-safe CLI wrapper
cargo add claude-wrapper

# Library: slot pool orchestration
cargo add claude-pool

# Binary: MCP server for the pool
cargo install claude-pool-server
```

## Documentation

- **API Docs**: [docs.rs/claude-wrapper](https://docs.rs/claude-wrapper), [docs.rs/claude-pool](https://docs.rs/claude-pool), [docs.rs/claude-pool-server](https://docs.rs/claude-pool-server)
- **Crate READMEs**: See individual crate directories above
- **Examples**: Each crate README contains detailed examples

## Status

**Production-ready.** All three crates are actively maintained with comprehensive test coverage (168+ lib tests, plus integration and doc tests).

### Implemented

- Full CLI wrapper (28 QueryCommand options + all subcommands)
- Slot pool with task routing, budgets, and slot identity
- MCP server binary with tools and resources
- Sequential chains with failure policies
- Parallel fan-out execution
- Shared context injection across slots
- Worktree isolation per slot
- Reusable skills registry

## Development & Testing

Pre-commit checks:

```bash
cargo fmt --all -- --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --lib --all-features
cargo test --test '*' --all-features
```

Doc tests:

```bash
cargo test --doc --all-features
```

Full release checklist (before merging release PR):

```bash
cargo doc --no-deps --all-features  # Docs build without warnings
cargo test --doc --all-features     # Doc tests pass
cargo publish --dry-run -p claude-wrapper
cargo publish --dry-run -p claude-pool
cargo publish --dry-run -p claude-pool-server
```

## Contributing

Issues and PRs welcome. Please ensure all checks pass before submitting.

## License

MIT OR Apache-2.0
