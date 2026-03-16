# AGENTS.md

## Project Overview

Rust workspace providing a type-safe wrapper around the Claude Code CLI, a slot-based pool for parallel task execution, and an MCP server that exposes pool operations as tools.

## Workspace Structure

```
crates/
  claude-wrapper/     # Core CLI wrapper (builder pattern, typed outputs, async)
  claude-pool/        # Pool library (slots, chains, fan-out, auto-routing, messaging, worktree isolation)
  claude-pool-mcp/    # MCP server binary (stdio transport, tower-mcp based)
```

## Skills

Pool coordinator skill (follows [Agent Skills spec](https://agentskills.io/specification)): [`crates/claude-pool-mcp/skills/pool-coordinator/SKILL.md`](crates/claude-pool-mcp/skills/pool-coordinator/SKILL.md)

This skill teaches Claude to prefer pool MCP tools over built-in Agent() calls, with tool selection guidance, model recommendations, and a complete tool reference.

## Build and Test

```bash
# Format
cargo fmt --all -- --check

# Lint
cargo clippy --all-targets --all-features -- -D warnings

# Unit tests
cargo test --lib --all-features

# Doc tests
cargo test --doc --all-features

# Integration tests (requires fake-claude binary)
cargo test --test pool_integration --test auto_route_tests -p claude-pool -- --ignored

# Live routing accuracy test (requires real claude binary, burns tokens)
cargo test --test route_stress -p claude-pool -- --ignored

# Docs
cargo doc --no-deps --all-features
```

Run format, lint, unit tests, and doc tests before every commit.

## Code Style

- Rust 2024 edition, MSRV 1.90.0
- `thiserror` for library errors, `anyhow` for application/integration test errors
- All public APIs must have doc comments
- No emojis in code, commits, or documentation
- Prefer editing existing files over creating new ones

## Architecture

**claude-wrapper** uses a two-layer builder:
1. `Claude` client -- binary path, working dir, env, timeout, global args
2. Command builders (`QueryCommand`, `McpAddCommand`, etc.) -- per-subcommand options, `execute(&claude)` returns typed output

**claude-pool** manages a pool of Claude CLI slots:
- `Pool` orchestrates slot lifecycle, task assignment, chains, fan-outs
- `PoolStore` trait abstracts storage (in-memory default, pluggable)
- Auto-routing classifies tasks as single/parallel/chain via LLM
- `WorktreeManager` handles git worktree/clone isolation for parallel work
- `MessageBus` provides inter-slot messaging with broadcast support
- `RouteTestRunner` provides structured routing accuracy testing

**claude-pool-mcp** exposes pool as MCP tools via `tower-mcp`:
- 31 tools: run, submit, chain, fan-out, auto-route, review, context, messaging, scaling
- Stdio transport, configurable via CLI flags

## Git Workflow

- Branch naming: `feat/`, `fix/`, `docs/`, `refactor/`, `test/`
- Conventional commits: `feat:`, `fix:`, `docs:`, etc.
- Never commit directly to main
- Create PRs but do not merge them

## Testing Patterns

- Unit tests live in the same file as the code (`#[cfg(test)] mod tests`)
- Integration tests in `tests/` use fake-claude binary and `#[ignore]`
- Live routing tests in `tests/route_stress.rs` use real claude and `#[ignore]`
- Use `InMemoryStore` for pool tests

## Key Dependencies

- `tower-mcp` 0.8 -- MCP server framework
- `tokio` -- async runtime
- `serde` / `serde_json` -- serialization
- `dashmap` -- concurrent hash maps (store, message bus)
- `clap` -- CLI argument parsing
- `schemars` -- JSON schema generation for MCP tool inputs
