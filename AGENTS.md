# AGENTS.md

Guidance for AI assistants working on this repo.

## What this is

`claude-wrapper` is a type-safe Rust wrapper around the Claude Code CLI. Each subcommand is a builder that produces a typed result. Execution is via tokio (default) or `std::thread` (with the `sync` feature). Built in: long-lived `DuplexSession` for hosts (one child held open across turns; mid-turn interrupts, permission handlers, broadcast subscribers), transient `Session` for short-lived processes (subprocess-per-turn with `--resume`, cumulative cost + history, optional `BudgetTracker` hard-stops, streaming), typed tool-permission patterns (`ToolPattern`), retry policy, and an `McpConfigBuilder` for programmatic `.mcp.json` generation.

This is the only crate in the repo.

## Layout

Flat single-crate layout:

```
Cargo.toml        # package + deps + feature flags
src/              # library
tests/            # integration tests + fake-claude.sh
examples/         # runnable examples (async-only)
```

## Build and test

Run the full pre-commit checklist before every commit:

```bash
cargo fmt --all -- --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --lib --all-features
cargo test --doc --all-features
```

The crate has overlapping feature gates. CI exercises the interesting
combinations; if you add a new code path behind a feature, verify:

```bash
# Default: ["async", "json", "tempfile"] -- the happy path
cargo build
cargo clippy --all-targets -- -D warnings

# All features enabled (adds `sync`)
cargo build --all-features

# Sync-only, no tokio in the runtime dep tree
cargo build --no-default-features --features "json,sync"
cargo clippy --all-targets --no-default-features --features "json,sync" -- -D warnings
```

Integration tests against the bundled `fake-claude.sh`:

```bash
# Async paths
cargo test --test fake_claude --all-features

# Sync paths (exec, retry, streaming, command surface)
cargo test --test fake_claude_sync --all-features
cargo test --test fake_claude_sync --no-default-features --features "json,sync"
```

Integration tests against a real `claude` binary (ignored by default, requires auth):

```bash
cargo test --test integration -- --ignored
```

## Architecture

`claude-wrapper` is a two-layer builder:

1. **`Claude` client** -- shared config (binary path, working dir, env, timeout, global args, default retry policy). Built via `Claude::builder()`.
2. **Command builders** -- per-subcommand options. Each implements the `ClaudeCommand` trait. Async callers use `cmd.execute(&claude).await`; sync callers enable the `sync` feature, `use claude_wrapper::ClaudeCommandSyncExt`, and call `cmd.execute_sync(&claude)`.

Key modules:

- `src/lib.rs` -- `Claude` + `ClaudeBuilder`, top-level re-exports, `cli_version` / `cli_version_sync`
- `src/command/mod.rs` -- `ClaudeCommand` trait (async `execute` gated on `feature = "async"`), `ClaudeCommandSyncExt` blanket-impl trait (sync)
- `src/command/query.rs` -- `QueryCommand`, the workhorse; has both `execute` / `execute_json` and `execute_sync` / `execute_json_sync`
- `src/command/mcp.rs` -- MCP server management
- `src/command/plugin.rs`, `marketplace.rs`, `auth.rs`, `doctor.rs`, `agents.rs`, `version.rs`, `raw.rs` -- other subcommands
- `src/exec.rs` -- process spawning (tokio for async, `std::process` + `wait-timeout` for sync), concurrent pipe drain, timeout cleanup
- `src/streaming.rs` -- `stream_query` / `stream_query_sync` for NDJSON streaming
- `src/session.rs` -- `Session` for multi-turn conversations from short-lived processes (async-only; subprocess per turn; holds `Arc<Claude>`, auto-threads session_id, tracks history/cost, supports streaming, optional `BudgetTracker` attachment)
- `src/duplex.rs` -- `DuplexSession` for long-running hosts (async + json; one `claude` subprocess held open in stream-json mode across many turns; supports `subscribe`, `interrupt`, and an async `PermissionHandler` for mid-turn permission decisions)
- `src/budget.rs` -- `BudgetTracker` with fire-once warn/exceeded callbacks and `Error::BudgetExceeded` hard-stop
- `src/tool_pattern.rs` -- `ToolPattern` for typed `--allowed-tools` / `--disallowed-tools` patterns (`tool`, `tool_with_args`, `all`, `mcp` constructors; `parse()` validation; loose `From<&str>` for back-compat)
- `src/mcp_config.rs` -- `McpConfigBuilder` for generating `.mcp.json` files
- `src/retry.rs` -- `RetryPolicy`, `BackoffStrategy`, `with_retry` (async) and `with_retry_sync`
- `src/dangerous.rs` -- `DangerousClient` + env-var gate for bypass-permissions mode (isolated from the default API surface)
- `src/types.rs` -- `QueryResult`, `Transport`, `PermissionMode`, `Effort`, etc.
- `src/error.rs` -- `Error` enum (thiserror)

## Feature flags

- `async` (default) -- tokio-backed async API. Disabling drops tokio from the runtime dep tree.
- `json` (default) -- JSON output parsing via serde_json; also gates `Session`, `stream_query`, `execute_json`, `StreamEvent`.
- `tempfile` (default) -- `McpConfigBuilder::build_temp` via `TempMcpConfig`.
- `sync` -- blocking API: `*_sync` functions on `exec::`, `retry::`, each command builder, and `Claude`. Pulls in `wait-timeout` only.

The `Session` module and every `async fn execute` / `async fn execute_json` are gated on `feature = "async"`. Everything touching `serde_json` or `StreamEvent` is gated on `feature = "json"`. `ClaudeCommandSyncExt` and the `*_sync` methods are gated on `feature = "sync"`.

## Code conventions

- Rust 2024 edition, MSRV 1.90.0
- `thiserror` for library errors, `anyhow` only in examples/integration tests
- All public APIs must have doc comments with runnable examples where reasonable
- No emojis in code, commits, or documentation
- No em dashes in documentation, code comments, or commit messages -- use double hyphens
- Builder pattern: methods return `Self` (by value), take `impl Into<String>` / `impl Into<PathBuf>` for string-ish args
- Prefer editing existing files over creating new ones
- When adding code behind a feature gate, verify every build configuration compiles (see Build and test)

## Multi-turn API notes

The crate ships two multi-turn primitives. Recommend `DuplexSession`
for long-running hosts (the featureful default for agentic / UI
work), and `Session` for short-lived processes that just need
conversation continuity over a series of one-off subprocess calls.

### `DuplexSession` (recommended for long-running hosts)

Holds one `claude` subprocess open in stream-json duplex mode for
the lifetime of the conversation. Async-only, gated on `feature =
"async"` + `feature = "json"`. Key points:

- `DuplexSession::spawn(claude, opts)` -- starts the child and the
  background task that owns it.
- `send(prompt) -> TurnResult` -- one turn at a time; `send` while
  another turn is pending returns `Error::DuplexTurnInFlight`.
- `subscribe() -> broadcast::Receiver<InboundEvent>` -- typed event
  stream (`SystemInit`, `Assistant`, `StreamEvent`, `User`,
  `Other`). Multiple receivers are supported. Slow consumers see
  `RecvError::Lagged` rather than blocking.
- `interrupt() -> Result<()>` -- writes a clean
  `control_request {subtype: "interrupt"}`. The in-flight turn
  closes shortly after with a truncated `TurnResult`.
- `respond_to_permission(request_id, decision)` -- answers a
  permission prompt that was deferred from the handler.
- `close(self)` -- drops the outbound channel, closes stdin, and
  reaps the child. Drop without close also tears down (via
  `kill_on_drop` on the `Command`).
- Not `Clone`. Wrap in `Arc<DuplexSession>` if you need to call
  `&self` methods (`send`, `subscribe`, `interrupt`,
  `respond_to_permission`) from multiple tasks. `close(self)`
  requires exclusive ownership.
- TurnResult does not track cumulative cost or history across
  turns -- the events vec is per-turn. Hosts that want that can
  accumulate themselves; we may add a typed wrapper later.

### `Session` (for short-lived processes)

Holds host-side state across a series of transient `claude -p`
subprocess calls. Key points:

- Holds `Arc<Claude>` -- `Send + Sync`, can move between tasks, store in long-lived actor state
- `Session::new(arc)` starts fresh; `Session::resume(arc, id)` reattaches to an existing id
- `send(prompt)` for simple turns; `execute(cmd)` when the caller supplies a configured `QueryCommand`
- Streaming via `stream(prompt, handler)` / `stream_execute(cmd, handler)` -- session id is captured from the first event that carries one and persists even on stream error
- `execute` / `stream_execute` internally call `QueryCommand::replace_session` to override any conflicting session flags on the caller's command
- Tracks `history()`, `total_cost_usd()`, `total_turns()`, `last_result()`
- Attach a `BudgetTracker` via `with_budget(tracker)` for fleet-wide or session-scoped cost ceilings; `execute` short-circuits with `Error::BudgetExceeded` if the ceiling is hit before the turn runs

Do not add a `SessionQuery`-style delegated builder. The old version mirrored 30 QueryCommand methods and was deleted in the 0.5.0 reshape for good reasons. If a caller needs per-turn options, they construct a `QueryCommand` and pass it to `execute` / `stream_execute`.

There is no sync twin of `Session` yet. Sync callers compose `Session`-like state by hand using `execute_sync` / `execute_json_sync` and `BudgetTracker`. If a sync `Session` lands, it should use the existing `Arc<Claude>` shape and share the same history/cost accounting.

## Git workflow

- Never commit directly to `main`
- Branch naming: `feat/`, `fix/`, `docs/`, `refactor/`, `test/`, `chore/`
- Conventional commits: `feat:`, `fix:`, `docs:`, etc. Use `feat!:` or `fix!:` for breaking changes (triggers a minor version bump via release-plz).
- Reference issues in PR bodies with `Closes #N` for auto-close
- Do not merge PRs -- the human will do it
- Never include "Generated with Claude Code" or `Co-Authored-By` signatures in commits or PRs
- PR bodies that include code fences via `<<'EOF'` heredoc must use plain triple-backticks -- do not escape them. Escaping inside a single-quoted heredoc lands literally in the body and breaks GitHub's code fence rendering.

## Release process

Releases are driven by [`release-plz`](https://github.com/release-plz/release-plz):

- `release-plz.toml` at the workspace root configures the release for `claude-wrapper` only
- On every push to `main`, the `release-plz` workflow updates a long-running "chore: release" PR with the pending version bump + changelog
- Merging that PR tags `v{version}`, creates a GitHub release, and publishes to crates.io

## What to avoid

- Don't re-introduce `cargo-dist` or `Dockerfile` infrastructure -- both were removed deliberately in PR #524 when the repo pivoted to pure-wrapper.
- Don't add a `SessionQuery`-style delegated builder (see Session API notes above).
- Don't validate conflicting `QueryCommand` flags at the builder layer -- the CLI is the source of truth, and the `Session` layer handles conflict resolution via `replace_session`.
- Don't add `#![deny(missing_docs)]` to the crate root without checking that every public item has a doc comment first.
- Don't reintroduce the deprecated `claude-pool` / `claude-pool-mcp` / `claudes` / `claude-runner` crates. They were removed in favour of letting `claude-wrapper` be the single focus of this repo.
