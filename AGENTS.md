# AGENTS.md

## Project Overview

Rust workspace providing a type-safe wrapper around the Claude Code CLI, a slot-based pool for parallel task execution, and an MCP server that exposes pool operations as tools.

## Workspace Structure

```
crates/
  claude-wrapper/   # Core CLI wrapper (builder pattern, typed outputs, async)
  claude-pool/      # Pool library (slots, chains, skills, messaging, worktree isolation)
  claude-pool-server/ # MCP server binary (stdio + HTTP transports)
```

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

# Integration tests (requires real claude binary)
cargo test --test '*' --all-features -- --ignored

# Docs
cargo doc --no-deps --all-features
```

Run all four non-integration checks before every commit.

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
- `SkillRegistry` loads SKILL.md-format skills (builtins, global, project)
- `WorktreeManager` handles git worktree/clone isolation for parallel work
- `MessageBus` provides inter-slot messaging with broadcast support

**claude-pool-server** exposes pool as MCP tools via `tower-mcp`:
- 30+ tools: run, submit, chain, fan-out, claim, broadcast, find-slots, skills, context, messaging
- Resources and prompts for skill discovery
- HTTP transport with bearer token auth (behind `http` feature flag)

## Git Workflow

- Branch naming: `feat/`, `fix/`, `docs/`, `refactor/`, `test/`
- Conventional commits: `feat:`, `fix:`, `docs:`, etc.
- Never commit directly to main
- Create PRs but do not merge them

## Testing Patterns

- Unit tests live in the same file as the code (`#[cfg(test)] mod tests`)
- Integration tests in `tests/` directories require a real `claude` binary and use `#[ignore]`
- Use `InMemoryStore` for pool tests
- MCP tool tests validate JSON schemas and error handling

## Key Dependencies

- `tower-mcp` 0.8 -- MCP server framework
- `tokio` -- async runtime
- `serde` / `serde_json` / `serde_yaml` -- serialization
- `dashmap` -- concurrent hash maps (store, message bus)
- `clap` -- CLI argument parsing
- `schemars` -- JSON schema generation for MCP tool inputs
