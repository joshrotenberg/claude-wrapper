# AGENTS.md

Guidance for AI assistants working on this repo.

## Project overview

This repo publishes a single crate: [`claude-wrapper`](crates/claude-wrapper/), a type-safe Rust wrapper around the Claude Code CLI. Builder pattern for each subcommand, typed outputs, async execution via tokio, and a multi-turn `Session` API with streaming support.

Four other crates (`claude-pool`, `claude-pool-mcp`, `claudes`, `claude-runner`) live in-tree for git history but are `publish = false` and excluded from the workspace build via `[workspace] exclude`. They will not receive updates. Do not touch them in normal work. If someone asks to revive one, move it from `exclude` back into `members` in the root `Cargo.toml`.

## Workspace layout

```
crates/
  claude-wrapper/     # The published crate (edit this)
  test-helpers/       # fake-claude.sh script used by claude-wrapper integration tests
  claude-pool/        # DEPRECATED (excluded from workspace)
  claude-pool-mcp/    # DEPRECATED (excluded from workspace)
  claudes/            # DEPRECATED (excluded from workspace)
  claude-runner/      # DEPRECATED (excluded from workspace)
```

## Build and test

Run the full pre-commit checklist before every commit:

```bash
cargo fmt --all -- --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --lib --all-features
cargo test --doc --all-features
```

CI also runs `cargo build --no-default-features`, which exercises the `#[cfg(not(feature = "json"))]` branches. If you add code behind a feature gate, verify it compiles both ways.

Integration tests against the bundled `fake-claude.sh` (no real CLI required):

```bash
cargo test -p claude-wrapper --test fake_claude --all-features
```

Integration tests against a real `claude` binary (ignored by default, requires auth):

```bash
cargo test -p claude-wrapper --test integration -- --ignored
```

## Architecture

`claude-wrapper` is a two-layer builder:

1. **`Claude` client** -- shared config (binary path, working dir, env, timeout, global args, default retry policy). Built via `Claude::builder()`.
2. **Command builders** -- per-subcommand options. Each implements the `ClaudeCommand` trait and is invoked with `cmd.execute(&claude).await` returning a typed `CommandOutput` or, for JSON-returning commands, `execute_json(&claude)` returning a parsed struct.

Key modules:

- `src/lib.rs` -- `Claude` + `ClaudeBuilder`, top-level re-exports
- `src/command/mod.rs` -- `ClaudeCommand` trait
- `src/command/query.rs` -- `QueryCommand`, the workhorse
- `src/command/mcp.rs` -- MCP server management
- `src/command/plugin.rs`, `marketplace.rs`, `auth.rs`, `doctor.rs`, `agents.rs`, `version.rs`, `raw.rs` -- other subcommands
- `src/exec.rs` -- process spawning, timeout, child cleanup
- `src/streaming.rs` -- `stream_query()` for NDJSON streaming
- `src/session.rs` -- `Session` for multi-turn conversations (holds `Arc<Claude>`, auto-threads session_id, tracks history/cost, supports streaming)
- `src/mcp_config.rs` -- `McpConfigBuilder` for generating `.mcp.json` files
- `src/retry.rs` -- `RetryPolicy`, `BackoffStrategy`, `with_retry()` wrapper
- `src/types.rs` -- `QueryResult`, `Transport`, `PermissionMode`, `Effort`, etc.
- `src/error.rs` -- `Error` enum (thiserror)

## Code conventions

- Rust 2024 edition, MSRV 1.90.0
- `thiserror` for library errors, `anyhow` only in examples/integration tests
- All public APIs must have doc comments with runnable examples where reasonable
- No emojis in code, commits, or documentation
- No em dashes in documentation, code comments, or commit messages -- use double hyphens
- Builder pattern: methods return `Self` (by value), take `impl Into<String>` / `impl Into<PathBuf>` for string-ish args
- Prefer editing existing files over creating new ones
- Feature gates: `json` is on by default; `tempfile` is on by default. Anything touching `serde_json` or `StreamEvent` must be `#[cfg(feature = "json")]`. Anything touching tempfile-backed MCP config must be `#[cfg(feature = "tempfile")]`.

## Session API notes

`Session` is the preferred multi-turn interface. Key points:

- Holds `Arc<Claude>` -- `Send + Sync`, can move between tasks, store in long-lived actor state
- `Session::new(arc)` starts fresh; `Session::resume(arc, id)` reattaches to an existing id
- `send(prompt)` for simple turns; `execute(cmd)` when the caller supplies a configured `QueryCommand`
- Streaming via `stream(prompt, handler)` / `stream_execute(cmd, handler)` -- session id is captured from the first event that carries one and persists even on stream error
- `execute` / `stream_execute` internally call `QueryCommand::replace_session` to override any conflicting session flags on the caller's command
- Tracks `history()`, `total_cost_usd()`, `total_turns()`, `last_result()`

Do not add a `SessionQuery`-style delegated builder. The old version mirrored 30 QueryCommand methods and was deleted in the 0.5.0 reshape for good reasons. If a caller needs per-turn options, they construct a `QueryCommand` and pass it to `execute` / `stream_execute`.

## Git workflow

- Never commit directly to `main`
- Branch naming: `feat/`, `fix/`, `docs/`, `refactor/`, `test/`, `chore/`
- Conventional commits: `feat:`, `fix:`, `docs:`, etc. Use `feat!:` or `fix!:` for breaking changes (triggers a minor version bump via release-plz).
- Reference issues in PR bodies with `Closes #N` for auto-close
- Do not merge PRs -- the human will do it
- Never include "Generated with Claude Code" or `Co-Authored-By` signatures in commits or PRs

## Release process

Releases are driven by [`release-plz`](https://github.com/release-plz/release-plz):

- `release-plz.toml` at the workspace root configures the release for `claude-wrapper` only
- On every push to `main`, the `release-plz` workflow updates a long-running "chore: release" PR with the pending version bump + changelog
- Merging that PR tags `v{version}`, creates a GitHub release, and publishes to crates.io

The deprecated crates are all `publish = false`, so they are not touched by release-plz.

## What to avoid

- Don't add new features to the deprecated crates.
- Don't re-introduce `cargo-dist` or `Dockerfile` infrastructure -- both were removed deliberately in PR #524 when the repo pivoted to pure-wrapper.
- Don't add a `SessionQuery`-style delegated builder (see Session API notes above).
- Don't validate conflicting `QueryCommand` flags at the builder layer -- the CLI is the source of truth, and the `Session` layer handles conflict resolution via `replace_session`.
- Don't add `#![deny(missing_docs)]` to the crate root without checking that every public item has a doc comment first.
