# claude-wrapper

A type-safe Claude Code CLI wrapper for Rust

[![Crates.io](https://img.shields.io/crates/v/claude-wrapper.svg)](https://crates.io/crates/claude-wrapper)
[![Documentation](https://docs.rs/claude-wrapper/badge.svg)](https://docs.rs/claude-wrapper)
[![CI](https://github.com/joshrotenberg/claude-wrapper/actions/workflows/ci.yml/badge.svg)](https://github.com/joshrotenberg/claude-wrapper/actions/workflows/ci.yml)
[![License](https://img.shields.io/crates/l/claude-wrapper.svg)](LICENSE-MIT)

## Overview

`claude-wrapper` provides a type-safe builder-pattern interface for invoking the `claude` CLI programmatically. It follows the same design philosophy as [`docker-wrapper`](https://crates.io/crates/docker-wrapper) and [`terraform-wrapper`](https://crates.io/crates/terraform-wrapper): each CLI subcommand is a builder struct that produces typed output.

Perfect for Rust applications that need to integrate with Claude Code CLI.

## Installation

```bash
cargo add claude-wrapper
```

## Quick Start

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

## Two-Layer Builder Architecture

The `Claude` client holds shared configuration (binary path, environment, timeout). Command builders hold per-invocation options and call `execute(&claude)`.

### Claude Client

Configure once, reuse across commands:

```rust
let claude = Claude::builder()
    .env("AWS_REGION", "us-west-2")
    .timeout_secs(300)
    .build()?;
```

Options:
- `binary()` - Path to `claude` binary (auto-detected from PATH by default)
- `working_dir()` - Working directory for commands
- `env()` / `envs()` - Set environment variables
- `timeout_secs()` / `timeout()` - Command timeout (no default)
- `arg()` - Add a global argument applied to every command
- `retry()` - Default [`RetryPolicy`] applied to every command

### Command Builders

Each CLI subcommand is a separate builder. Available commands:

**Query**
- `QueryCommand` - The workhorse; `claude -p` with full option coverage

**MCP server management**
- `McpListCommand` / `McpGetCommand` / `McpAddCommand` / `McpAddJsonCommand` / `McpRemoveCommand` / `McpAddFromDesktopCommand` / `McpResetProjectChoicesCommand` / `McpServeCommand`

**Plugins**
- `PluginListCommand` / `PluginInstallCommand` / `PluginUninstallCommand` / `PluginEnableCommand` / `PluginDisableCommand` / `PluginUpdateCommand` / `PluginValidateCommand`

**Plugin marketplace**
- `MarketplaceListCommand` / `MarketplaceAddCommand` / `MarketplaceRemoveCommand` / `MarketplaceUpdateCommand`

**Auth**
- `AuthStatusCommand` / `AuthLoginCommand` / `AuthLogoutCommand` / `SetupTokenCommand`

**Misc**
- `VersionCommand` - Get CLI version
- `DoctorCommand` - Run health check
- `AgentsCommand` - List available agents
- `RawCommand` - Escape hatch for unsupported subcommands

## QueryCommand: The Workhorse

Full coverage of `claude -p` (print mode). `QueryCommand` exposes a builder method for every CLI flag; see [`docs.rs`](https://docs.rs/claude-wrapper/latest/claude_wrapper/struct.QueryCommand.html) for the full list. The common ones:

| Method | CLI flag | Purpose |
|---|---|---|
| `model()` | `--model` | Model alias or full ID |
| `system_prompt()` / `append_system_prompt()` | `--system-prompt` / `--append-system-prompt` | Override or extend the system prompt |
| `output_format()` | `--output-format` | `text`, `json`, `stream-json` |
| `effort()` | `--effort` | `low`, `medium`, `high` |
| `max_turns()` | `--max-turns` | Turn cap |
| `max_budget_usd()` | `--max-budget-usd` | Spend cap |
| `permission_mode()` | `--permission-mode` | `default`, `acceptEdits`, `bypassPermissions`, `dontAsk`, `plan`, `auto` |
| `allowed_tools()` / `disallowed_tools()` / `tools()` | tool-filter flags | Allow/deny/restrict tools |
| `mcp_config()` / `strict_mcp_config()` | `--mcp-config` / `--strict-mcp-config` | MCP server config |
| `json_schema()` | `--json-schema` | Structured output validation |
| `agent()` / `agents_json()` | `--agent` / `--agents` | Agent or custom agents JSON |
| `no_session_persistence()` | `--no-session-persistence` | Don't save the session to disk |
| `dangerously_skip_permissions()` | `--dangerously-skip-permissions` | Sandbox escape hatch |
| `input_format()` / `include_partial_messages()` | `--input-format` / `--include-partial-messages` | Input/streaming shape |
| `settings()` / `fallback_model()` / `add_dir()` / `file()` | various | Misc per-turn knobs |

### Raw session flags (prefer `Session` for multi-turn)

| Method | CLI flag | Purpose |
|---|---|---|
| `continue_session()` | `--continue` | Resume most recent session |
| `resume()` | `--resume` | Resume a specific session id |
| `session_id()` | `--session-id` | Force a session id |
| `fork_session()` | `--fork-session` | Fork when resuming |

For multi-turn conversations you almost always want [`Session`](#session-management) instead, which threads the id across turns automatically.

### Usage Examples

Simple query with model override:

```rust
let output = QueryCommand::new("explain this error: file not found")
    .model("sonnet")
    .execute(&claude)
    .await?;
println!("{}", output.stdout);
```

JSON output with schema validation:

```rust
let output = QueryCommand::new("generate a user struct")
    .output_format(OutputFormat::Json)
    .json_schema(r#"{"type":"object","properties":{"id":{"type":"integer"}}}"#)
    .execute(&claude)
    .await?;
```

With permissions and tools:

```rust
let output = QueryCommand::new("implement the feature in TASK.md")
    .model("opus")
    .permission_mode(PermissionMode::Plan)
    .allowed_tools(["Bash", "Read", "Edit", "Write"])
    .max_budget_usd(1.0)
    .execute(&claude)
    .await?;
```

Low-level session flags on `QueryCommand` (rarely needed):

```rust
// Force a specific session id
let output = QueryCommand::new("analyze the codebase")
    .session_id("my-session")
    .execute(&claude)
    .await?;

// Resume later
let output = QueryCommand::new("what did we find?")
    .resume("my-session")
    .execute(&claude)
    .await?;
```

<a id="session-management"></a>

## Session management

For multi-turn conversations, wrap the client in an `Arc` and use `Session`. It threads the CLI `session_id` across turns automatically, tracks cumulative cost + history, and supports both plain-prompt and streaming turns:

```rust
use std::sync::Arc;
use claude_wrapper::{Claude, QueryCommand};
use claude_wrapper::session::Session;

let claude = Arc::new(Claude::builder().build()?);

// Fresh session
let mut session = Session::new(Arc::clone(&claude));

// Plain prompt: session_id is captured from the first turn
let first = session.send("what's 2 + 2?").await?;

// Full control: pass a configured QueryCommand. Any session-related
// flags on `cmd` are overridden with this session's current id so
// they can't conflict.
let second = session
    .execute(QueryCommand::new("explain").model("opus"))
    .await?;

// Reattach to an existing session id
let mut resumed = Session::resume(claude, "sess-abc123");
let next = resumed.send("pick up where we left off").await?;

// Cumulative state
println!("cost: ${:.4}", session.total_cost_usd());
println!("turns: {}", session.total_turns());
println!("history: {} turns", session.history().len());
```

`Session` is `Send + Sync` and holds an `Arc<Claude>`, so it can be
moved between tasks or stored in long-lived actor state. Streaming
turns use `Session::stream` / `Session::stream_execute`, which capture
the session id from the first event that carries one — and persist
it even if the stream errors partway through.

## Streaming NDJSON events

For real-time output, use `stream_query()`. The handler is called once per parsed NDJSON line:

```rust
use claude_wrapper::{Claude, QueryCommand, OutputFormat};
use claude_wrapper::streaming::{StreamEvent, stream_query};

let claude = Claude::builder().build()?;
let cmd = QueryCommand::new("explain quicksort")
    .output_format(OutputFormat::StreamJson);

stream_query(&claude, &cmd, |event: StreamEvent| {
    if let Some(t) = event.event_type() {
        println!("[{t}]");
    }
    if event.is_result() {
        println!("result: {}", event.result_text().unwrap_or(""));
        println!("session: {}", event.session_id().unwrap_or(""));
        println!("cost: ${:?}", event.cost_usd());
    }
}).await?;
```

For multi-turn streaming with automatic session-id capture, use `Session::stream` / `Session::stream_execute` instead.

## MCP Server Commands

### List Servers

```rust
let output = McpListCommand::new()
    .execute(&claude)
    .await?;
println!("{}", output.stdout);
```

### Add HTTP Server

```rust
McpAddCommand::new("sentry", "https://mcp.sentry.dev/mcp")
    .transport(Transport::Http)
    .execute(&claude)
    .await?;
```

### Add Stdio Server

```rust
McpAddCommand::new("my-tool", "npx")
    .server_args(["my-mcp-server"])
    .env("API_KEY", "secret")
    .execute(&claude)
    .await?;
```

### Add from JSON

```rust
McpAddJsonCommand::new("config.json")
    .execute(&claude)
    .await?;
```

### Remove Server

```rust
McpRemoveCommand::new("old-server")
    .execute(&claude)
    .await?;
```

## MCP config builder

Generate `.mcp.json` files programmatically:

```rust
use claude_wrapper::McpConfigBuilder;

McpConfigBuilder::new()
    .http_server("hub", "http://127.0.0.1:9090")
    .stdio_server("tool", "npx", ["my-server"])
    .write_to("/tmp/my-project/.mcp.json")?;
```

With the `tempfile` feature (on by default), `build_temp()` writes to a temporary file that is deleted when the returned `TempMcpConfig` is dropped — useful for one-shot queries.

## Plugin Management

```rust
// List plugins
PluginListCommand::new().execute(&claude).await?;

// Install plugin
PluginInstallCommand::new("my-plugin")
    .execute(&claude)
    .await?;

// Enable plugin
PluginEnableCommand::new("my-plugin")
    .execute(&claude)
    .await?;

// Update plugin
PluginUpdateCommand::new("my-plugin")
    .execute(&claude)
    .await?;
```

## Other Commands

```rust
// Check auth
AuthStatusCommand::new().execute(&claude).await?;

// Get version
VersionCommand::new().execute(&claude).await?;

// Health check
DoctorCommand::new().execute(&claude).await?;

// List agents
AgentsListCommand::new().execute(&claude).await?;
```

## Escape hatch: RawCommand

For subcommands not yet wrapped, use `RawCommand`. Pass the subcommand name to `new()` and any flags via `.arg()`:

```rust
let output = RawCommand::new("custom-subcommand")
    .arg("--unsupported-flag")
    .arg("value")
    .execute(&claude)
    .await?;
```

## Error Handling

All commands return `Result<T>`, where errors are typed with `thiserror`:

```rust
use claude_wrapper::Error;

match QueryCommand::new("test").execute(&claude).await {
    Ok(output) => println!("{}", output.stdout),
    Err(Error::CommandFailed { stderr, exit_code, .. }) => {
        eprintln!("Command failed (exit {}): {}", exit_code, stderr);
    }
    Err(Error::Timeout { .. }) => eprintln!("Command timed out"),
    Err(e) => eprintln!("Other error: {}", e),
}
```

## Features

Optional Cargo features (all enabled by default):

- `json` - JSON output parsing via serde_json
- `tempfile` - Temporary file support for MCP config generation

## Examples

Runnable examples in `examples/`:

```bash
cargo run -p claude-wrapper --example oneshot          # Single query
cargo run -p claude-wrapper --example stream_query     # Streaming NDJSON events
cargo run -p claude-wrapper --example json_output      # Structured JSON response
cargo run -p claude-wrapper --example mcp_config       # MCP config generation
cargo run -p claude-wrapper --example health_check     # CLI diagnostics
cargo run -p claude-wrapper --example agent_worker     # Agent worker setup
cargo run -p claude-wrapper --example supervised_worker # Restart loop with budget tracking
```

## Testing

Requires the `claude` CLI binary to be installed:

```bash
cargo test --lib --all-features
cargo test --test integration -- --ignored  # requires `claude` binary
```

## License

MIT OR Apache-2.0
