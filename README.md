# claude-wrapper

Type-safe Rust wrapper around the Claude Code CLI.

[![Crates.io](https://img.shields.io/crates/v/claude-wrapper.svg)](https://crates.io/crates/claude-wrapper)
[![Documentation](https://docs.rs/claude-wrapper/badge.svg)](https://docs.rs/claude-wrapper)
[![CI](https://github.com/joshrotenberg/claude-wrapper/actions/workflows/ci.yml/badge.svg)](https://github.com/joshrotenberg/claude-wrapper/actions/workflows/ci.yml)
[![License](https://img.shields.io/crates/l/claude-wrapper.svg)](LICENSE-MIT)

This repo publishes a single crate: **[`claude-wrapper`](crates/claude-wrapper/)**, a builder-pattern interface for the `claude` CLI with typed outputs, retry policy, streaming NDJSON, and multi-turn sessions.

```rust
use std::sync::Arc;
use claude_wrapper::{Claude, QueryCommand};
use claude_wrapper::session::Session;

#[tokio::main]
async fn main() -> claude_wrapper::Result<()> {
    let claude = Arc::new(Claude::builder().build()?);

    // One-shot query
    let output = QueryCommand::new("explain this error")
        .model("sonnet")
        .max_turns(1)
        .execute(&claude)
        .await?;
    println!("{}", output.stdout);

    // Multi-turn session (auto-resume)
    let mut session = Session::new(claude);
    let first = session.send("what's 2 + 2?").await?;
    let second = session.send("and squared?").await?;
    println!("cost: ${:.4}", session.total_cost_usd());

    Ok(())
}
```

See the [crate README](crates/claude-wrapper/README.md) for the full API.

## Deprecated crates

The following crates live in-tree for git history but are **not published, not maintained, and excluded from the workspace build**:

| Crate | Status | Notes |
|---|---|---|
| [claudes](crates/claudes/) | Deprecated | Manifest-driven parallel execution engine. Superseded by similar tooling elsewhere. |
| [claude-pool](crates/claude-pool/) | Deprecated | Slot pool orchestration. |
| [claude-pool-mcp](crates/claude-pool-mcp/) | Deprecated | MCP server over claude-pool. |
| [claude-runner](crates/claude-runner/) | Deprecated | Autonomous GitHub issue runner. |

To revive one, move it from `[workspace] exclude` back into `[workspace] members` in the root `Cargo.toml`.

## Development

```bash
cargo fmt --all -- --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --lib --all-features
cargo test --doc --all-features

# Integration tests against the fake-claude binary (no real CLI needed)
cargo test -p claude-wrapper --test fake_claude --all-features

# Integration tests against a real claude binary (ignored by default)
cargo test -p claude-wrapper --test integration -- --ignored
```

## License

MIT OR Apache-2.0
