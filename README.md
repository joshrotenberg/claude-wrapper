# claude-wrapper

Rust tooling for the Claude Code CLI.

[![Crates.io](https://img.shields.io/crates/v/claude-wrapper.svg)](https://crates.io/crates/claude-wrapper)
[![Documentation](https://docs.rs/claude-wrapper/badge.svg)](https://docs.rs/claude-wrapper)
[![CI](https://github.com/joshrotenberg/claude-wrapper/actions/workflows/ci.yml/badge.svg)](https://github.com/joshrotenberg/claude-wrapper/actions/workflows/ci.yml)
[![License](https://img.shields.io/crates/l/claude-wrapper.svg)](LICENSE-MIT)

## Crates

| Crate | Purpose | Status |
|---|---|---|
| **[claude-wrapper](crates/claude-wrapper/)** | Type-safe Rust interface to Claude Code CLI | Stable |
| **[claudes](crates/claudes/)** | Manifest-driven parallel execution engine | Active development |
| **[claude-pool](crates/claude-pool/)** | Coordinator/worker orchestration | Deprecated (use claudes) |
| **[claude-pool-mcp](crates/claude-pool-mcp/)** | MCP server exposing pool as tools | Deprecated (use claudes) |

## claudes

Run headless Claude Code sessions in parallel from a manifest. Write a JSON or TOML
document describing your tasks, and claudes runs them concurrently in isolated git
worktrees with streaming output and post-hook validation.

```bash
# Run tasks from prompts
claudes run -p "fix the pagination bug" -p "add unit tests" -v

# Use AI to generate a manifest
claudes generate -p "work on the three open bugs as separate tasks"

# Run from a manifest file
claudes run --manifest plan.json -v

# Check results
claudes status
claudes metrics
```

### Example manifest (TOML)

```toml
[shared]
model = "sonnet"
max_turns = 30
post_hooks = ["cargo fmt --check", "cargo test --lib"]

[[tasks]]
name = "fix-pagination"
prompt = "Fix the pagination bug in src/api/list.rs"

[[tasks]]
name = "add-tests"
prompt = "Add unit tests for the auth module"
```

### Features

- Parallel execution in isolated git worktrees
- JSON and TOML manifest formats
- Shared blocks and named profiles
- Pre/post/finally hooks for validation and cleanup
- Streaming output with per-task colors (`-v`, `-vv`)
- Run state with timestamped IDs and cost tracking
- Auto-discovery of project manifest files
- `claudes fix` to retry failed tasks with error context
- `claudes generate` for AI-assisted manifest creation
- `claudes metrics` for historical analysis

See the [claudes README](crates/claudes/README.md) for full documentation and the
[prompt guide](crates/claudes/PROMPTING.md) for best practices.

## claude-wrapper

Type-safe Rust wrapper around the Claude Code CLI with builder pattern, typed outputs,
and async execution.

```rust
use claude_wrapper::{Claude, ClaudeCommand, QueryCommand};

#[tokio::main]
async fn main() -> claude_wrapper::Result<()> {
    let claude = Claude::builder().build()?;
    let output = QueryCommand::new("explain this error")
        .model("sonnet")
        .max_turns(1)
        .no_session_persistence()
        .execute(&claude)
        .await?;
    println!("{}", output.stdout);
    Ok(())
}
```

See the [claude-wrapper README](crates/claude-wrapper/README.md) for full API docs.

## Development

```bash
cargo fmt --all -- --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --lib --all-features
cargo test --doc --all-features

# claudes integration tests (requires fake-claude binary)
cargo test --test fake_claude -p claudes -- --ignored
```

## License

MIT OR Apache-2.0
