# claude-wrapper

A type-safe Rust wrapper around the Claude Code CLI.

[![Crates.io](https://img.shields.io/crates/v/claude-wrapper.svg)](https://crates.io/crates/claude-wrapper)
[![Documentation](https://docs.rs/claude-wrapper/badge.svg)](https://docs.rs/claude-wrapper)
[![CI](https://github.com/joshrotenberg/claude-wrapper/actions/workflows/ci.yml/badge.svg)](https://github.com/joshrotenberg/claude-wrapper/actions/workflows/ci.yml)
[![License](https://img.shields.io/crates/l/claude-wrapper.svg)](LICENSE-MIT)

## Overview

`claude-wrapper` gives you a builder-pattern interface for the `claude`
CLI. Each subcommand is a typed builder that produces a typed output.
The design follows the same shape as
[`docker-wrapper`](https://crates.io/crates/docker-wrapper) and
[`terraform-wrapper`](https://crates.io/crates/terraform-wrapper).

Features:

- Full coverage of `claude -p` via `QueryCommand`
- Async (tokio) and blocking (`std::thread` + `wait-timeout`) APIs behind feature flags
- Multi-turn `Session` with auto-resume, streaming, cumulative cost, and optional `BudgetTracker` hard-stops
- NDJSON streaming events (`stream_query` / `stream_query_sync`)
- Typed tool-permission patterns (`ToolPattern`)
- MCP server management: list, get, add, add-json, remove, add-from-desktop, serve, reset-project-choices
- Plugin and marketplace subcommands
- Auth: `status`, `login`, `logout`, `setup-token`
- `McpConfigBuilder` for programmatic `.mcp.json` generation (with optional tempfile backing)
- Retry policy with fixed or exponential backoff
- Opt-in `DangerousClient` for bypass-permissions mode (env-var gated)
- `RawCommand` escape hatch for subcommands not yet wrapped

## Installation

```bash
cargo add claude-wrapper
```

Default features: `["async", "json", "tempfile"]`. To drop `tokio`
entirely for a sync-only build:

```toml
claude-wrapper = { version = "0.6", default-features = false, features = ["json", "sync"] }
```

## Quick start (async)

```rust
use claude_wrapper::{Claude, QueryCommand};

#[tokio::main]
async fn main() -> claude_wrapper::Result<()> {
    let claude = Claude::builder().build()?;
    let output = QueryCommand::new("explain this error: file not found")
        .model("sonnet")
        .execute(&claude)
        .await?;
    println!("{}", output.stdout);
    Ok(())
}
```

## Quick start (sync)

Enable the `sync` feature and `use ClaudeCommandSyncExt`:

```rust
use claude_wrapper::{Claude, ClaudeCommandSyncExt, QueryCommand};

fn main() -> claude_wrapper::Result<()> {
    let claude = Claude::builder().build()?;
    let output = QueryCommand::new("explain this error")
        .model("sonnet")
        .execute_sync(&claude)?;
    println!("{}", output.stdout);
    Ok(())
}
```

The sync API uses `std::process::Command` plus `wait-timeout` for the
SIGKILL-on-deadline path, with dedicated reader threads for stdout and
stderr so the child never blocks on pipe-buffer backpressure. It matches
the async version's behaviour feature-for-feature (timeouts, retries,
large-output drain, non-`Send` handlers for streaming).

## Two-layer builder

The `Claude` client holds shared configuration (binary path,
environment, timeout, default retry policy). Command builders hold
per-invocation options and call `execute(&claude)` (or `execute_sync`).

```rust
let claude = Claude::builder()
    .env("AWS_REGION", "us-west-2")
    .timeout_secs(300)
    .retry(RetryPolicy::new().max_attempts(3).exponential())
    .build()?;
```

`Claude` options:

- `binary()` -- path to the `claude` binary (auto-detected from PATH by default)
- `working_dir()` / `with_working_dir()` -- working directory for commands
- `env()` / `envs()` -- environment variables applied to every command
- `arg()` -- global arg applied before every subcommand
- `timeout_secs()` / `timeout()` -- command timeout (no default)
- `retry()` -- default `RetryPolicy` applied to every command

`Claude` also exposes:

- `cli_version()` / `cli_version_sync()` -- parsed `CliVersion`
- `check_version()` / `check_version_sync()` -- assert a minimum version

## Command builders

| Category | Builders |
|---|---|
| Query | `QueryCommand` |
| MCP | `McpListCommand`, `McpGetCommand`, `McpAddCommand`, `McpAddJsonCommand`, `McpRemoveCommand`, `McpAddFromDesktopCommand`, `McpServeCommand`, `McpResetProjectChoicesCommand` |
| Plugins | `PluginListCommand`, `PluginInstallCommand`, `PluginUninstallCommand`, `PluginEnableCommand`, `PluginDisableCommand`, `PluginUpdateCommand`, `PluginValidateCommand` |
| Marketplace | `MarketplaceListCommand`, `MarketplaceAddCommand`, `MarketplaceRemoveCommand`, `MarketplaceUpdateCommand` |
| Auth | `AuthStatusCommand`, `AuthLoginCommand`, `AuthLogoutCommand`, `SetupTokenCommand` |
| Misc | `VersionCommand`, `DoctorCommand`, `AgentsCommand`, `RawCommand` |

Every builder implements `ClaudeCommand` (`args()`, async `execute`).
With the `sync` feature, `ClaudeCommandSyncExt` provides a blanket
`execute_sync` on every command that returns `CommandOutput`.
`QueryCommand` additionally has inherent `execute_sync` /
`execute_json_sync` that honour the retry policy;
`AuthStatusCommand` has `execute_json_sync`.

## QueryCommand

Full coverage of `claude -p`. The common options:

| Method | CLI flag | Purpose |
|---|---|---|
| `model()` | `--model` | Model alias or full ID |
| `system_prompt()` / `append_system_prompt()` | `--system-prompt` / `--append-system-prompt` | Override or extend the system prompt |
| `output_format()` | `--output-format` | `text`, `json`, `stream-json` |
| `effort()` | `--effort` | `low`, `medium`, `high` |
| `max_turns()` | `--max-turns` | Turn cap |
| `max_budget_usd()` | `--max-budget-usd` | Per-call spend cap (CLI-side) |
| `permission_mode()` | `--permission-mode` | `default`, `acceptEdits`, `dontAsk`, `plan`, `auto` |
| `allowed_tool()` / `allowed_tools()` / `disallowed_tool()` / `disallowed_tools()` | tool filter flags | Allow/deny tool patterns |
| `mcp_config()` / `strict_mcp_config()` | `--mcp-config` / `--strict-mcp-config` | MCP server config |
| `json_schema()` | `--json-schema` | Structured output validation |
| `agent()` / `agents_json()` | `--agent` / `--agents` | Agent or custom agents JSON |
| `no_session_persistence()` | `--no-session-persistence` | Don't save the session to disk |
| `input_format()` / `include_partial_messages()` | `--input-format` / `--include-partial-messages` | Input/streaming shape |
| `settings()` / `fallback_model()` / `add_dir()` / `file()` | various | Misc per-turn knobs |
| `retry()` | (client-side) | Per-command retry override |

Raw session flags (prefer `Session` for multi-turn):

| Method | CLI flag |
|---|---|
| `continue_session()` | `--continue` |
| `resume()` | `--resume` |
| `session_id()` | `--session-id` |
| `fork_session()` | `--fork-session` |

Bypass-permissions mode is **only** available via
[`DangerousClient`](#dangerous-bypass-mode); see below.

### JSON output

```rust
let result = QueryCommand::new("generate a user struct")
    .output_format(OutputFormat::Json)
    .json_schema(r#"{"type":"object","properties":{"id":{"type":"integer"}}}"#)
    .execute_json(&claude)
    .await?;
println!("{}", result.result);
println!("cost: ${:.4}", result.cost_usd.unwrap_or(0.0));
```

### Tool permissions

Use `ToolPattern` for first-class, validated patterns:

```rust
use claude_wrapper::{QueryCommand, ToolPattern};

let cmd = QueryCommand::new("review src/main.rs")
    .allowed_tool(ToolPattern::tool("Read"))
    .allowed_tool(ToolPattern::tool_with_args("Bash", "git log:*"))
    .allowed_tool(ToolPattern::all("Write"))
    .allowed_tool(ToolPattern::mcp("my-server", "*"))
    .disallowed_tool(ToolPattern::tool_with_args("Bash", "rm*"));
```

Bare strings still work via `From<&str>`:

```rust
let cmd = QueryCommand::new("review").allowed_tools(["Read", "Bash(git log:*)"]);
```

`ToolPattern::parse(s)` validates shape (balanced parens, no comma,
non-empty name) and returns a typed `PatternError` on malformed input.

## Session management

For multi-turn conversations, wrap the client in an `Arc` and use
`Session`. It threads `session_id` across turns, tracks cumulative cost
+ history, and supports streaming.

```rust
use std::sync::Arc;
use claude_wrapper::{Claude, QueryCommand};
use claude_wrapper::session::Session;

let claude = Arc::new(Claude::builder().build()?);
let mut session = Session::new(Arc::clone(&claude));

let first = session.send("what's 2 + 2?").await?;
let second = session
    .execute(QueryCommand::new("explain").model("opus"))
    .await?;

println!("cost: ${:.4}", session.total_cost_usd());
println!("turns: {}", session.total_turns());
```

Resume an existing session:

```rust
let mut resumed = Session::resume(claude, "sess-abc123");
let next = resumed.send("pick up where we left off").await?;
```

`Session` is `Send + Sync`, holds `Arc<Claude>`, and can move between
tasks or sit in long-lived actor state. Streaming turns use
`Session::stream` / `Session::stream_execute` and capture the session
id from the first event that carries one -- persisted even if the
stream errors partway through.

`Session` is currently async-only. Sync callers compose equivalent
state using `execute_sync` / `execute_json_sync` and `BudgetTracker`.

## Budget tracking

Attach a `BudgetTracker` to a session (or share one across several
sessions) to enforce USD ceilings:

```rust
use claude_wrapper::BudgetTracker;

let budget = BudgetTracker::builder()
    .max_usd(5.00)
    .warn_at_usd(4.00)
    .on_warning(|total| eprintln!("warning: ${total:.2} spent"))
    .on_exceeded(|total| eprintln!("budget hit: ${total:.2}"))
    .build();

let mut session = Session::new(claude).with_budget(budget.clone());
session.send("hello").await?;
println!("spent: ${:.4}", budget.total_usd());
println!("remaining: ${:.4}", budget.remaining_usd().unwrap());
```

Callbacks fire exactly once at their thresholds. When `max_usd` is
set, `Session::execute` / `Session::stream_execute` short-circuit with
`Error::BudgetExceeded` before dispatching any turn that would run
with the ceiling already hit -- a hard stop, not just a callback.

Clone a tracker across several sessions to share the running total.
`BudgetTracker` is internally `Arc<Mutex<_>>` so clones are cheap and
the total is coherent.

## Streaming NDJSON

```rust
use claude_wrapper::{Claude, QueryCommand, OutputFormat};
use claude_wrapper::streaming::{StreamEvent, stream_query};

let claude = Claude::builder().build()?;
let cmd = QueryCommand::new("explain quicksort")
    .output_format(OutputFormat::StreamJson);

stream_query(&claude, &cmd, |event: StreamEvent| {
    if event.is_result() {
        println!("result: {}", event.result_text().unwrap_or(""));
        println!("session: {}", event.session_id().unwrap_or(""));
    }
}).await?;
```

Sync: `stream_query_sync`. The handler runs on the caller's thread, so
it can capture non-`Send` state (`Rc<RefCell<_>>`, etc.).

## MCP servers

```rust
// List
McpListCommand::new().execute(&claude).await?;

// HTTP
McpAddCommand::new("sentry", "https://mcp.sentry.dev/mcp")
    .transport(Transport::Http)
    .execute(&claude)
    .await?;

// Stdio
McpAddCommand::new("my-tool", "npx")
    .server_args(["my-mcp-server"])
    .env("API_KEY", "secret")
    .execute(&claude)
    .await?;

// From JSON
McpAddJsonCommand::new("config.json").execute(&claude).await?;

// Remove
McpRemoveCommand::new("old-server").execute(&claude).await?;
```

### Programmatic `.mcp.json`

```rust
use claude_wrapper::McpConfigBuilder;

McpConfigBuilder::new()
    .http_server("hub", "http://127.0.0.1:9090")
    .stdio_server("tool", "npx", ["my-server"])
    .write_to("/tmp/my-project/.mcp.json")?;
```

With the `tempfile` feature (default), `build_temp()` returns a
`TempMcpConfig` that writes to a temp file and deletes it on drop --
useful for one-shot queries.

## Dangerous: bypass mode

`--permission-mode bypassPermissions` is isolated behind
`DangerousClient`, which requires an env-var acknowledgement at
process start:

```rust
use claude_wrapper::{Claude, QueryCommand};
use claude_wrapper::dangerous::DangerousClient;

// Set CLAUDE_WRAPPER_ALLOW_DANGEROUS=1 at process start
let dangerous = DangerousClient::new(Claude::builder().build()?)?;
let output = dangerous
    .query_bypass(QueryCommand::new("do the risky thing"))
    .await?;
```

Construction fails with `Error::DangerousNotAllowed` if the env var is
absent. There is no equivalent on plain `Claude` -- the type-level
isolation is deliberate.

## Retry policy

```rust
use std::time::Duration;
use claude_wrapper::RetryPolicy;

let policy = RetryPolicy::new()
    .max_attempts(3)
    .initial_backoff(Duration::from_secs(1))
    .exponential()
    .retry_on_timeout(true)
    .retry_on_exit_codes([1, 2]);

let claude = Claude::builder().retry(policy).build()?;
```

Retry wraps every `QueryCommand::execute` and `execute_sync` (other
commands don't retry). A per-command override is available via
`QueryCommand::retry(policy)`.

## Error handling

```rust
use claude_wrapper::Error;

match QueryCommand::new("test").execute(&claude).await {
    Ok(output) => println!("{}", output.stdout),
    Err(Error::CommandFailed { stderr, exit_code, .. }) => {
        eprintln!("Command failed (exit {}): {}", exit_code, stderr);
    }
    Err(Error::Timeout { .. }) => eprintln!("Command timed out"),
    Err(Error::BudgetExceeded { total_usd, max_usd }) => {
        eprintln!("Budget ${:.2} / ${:.2}", total_usd, max_usd);
    }
    Err(Error::VersionMismatch { found, minimum }) => {
        eprintln!("Claude CLI {} < required {}", found, minimum);
    }
    Err(e) => eprintln!("Other error: {}", e),
}
```

## Escape hatch: RawCommand

For subcommands not yet wrapped:

```rust
let output = RawCommand::new("custom-subcommand")
    .arg("--unsupported-flag")
    .arg("value")
    .execute(&claude)
    .await?;
```

## Cargo features

| Feature | Default | Purpose |
|---|---|---|
| `async` | yes | tokio-backed async API. Disabling drops tokio from the runtime dep tree. |
| `json` | yes | JSON output parsing (`execute_json`, `StreamEvent`, `Session`, `stream_query`). |
| `tempfile` | yes | `TempMcpConfig` for one-shot MCP config files. |
| `sync` | no | Blocking API: `*_sync` methods on `exec`, `retry`, each command builder, and `Claude`. Pulls in `wait-timeout`. |

Sync-only build with no tokio:

```toml
claude-wrapper = { version = "0.6", default-features = false, features = ["json", "sync"] }
```

## Examples

```bash
cargo run --example oneshot           # Single query
cargo run --example stream_query      # Streaming NDJSON
cargo run --example json_output       # Structured JSON response
cargo run --example mcp_config        # MCP config generation
cargo run --example health_check      # CLI diagnostics
cargo run --example agent_worker      # Agent worker setup
cargo run --example supervised_worker # Restart loop with budget tracking
```

All examples use the async API and require `--features async` (on by
default).

## Testing

```bash
# Unit + doc tests
cargo test --lib --all-features
cargo test --doc --all-features

# Integration tests against the bundled fake-claude.sh (no real CLI needed)
cargo test --test fake_claude --all-features
cargo test --test fake_claude_sync --all-features

# Sync-only build + tests (no tokio)
cargo test --test fake_claude_sync \
    --no-default-features --features "json,sync"

# Integration tests against a real `claude` binary (requires auth)
cargo test --test integration -- --ignored
```

## License

MIT OR Apache-2.0
