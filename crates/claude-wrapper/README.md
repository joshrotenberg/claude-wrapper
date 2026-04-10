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
- `binary_path()` - Path to `claude` binary (auto-detected by default)
- `working_dir()` - Working directory for commands
- `env()` - Set environment variables
- `timeout_secs()` - Command timeout (default: 300)
- `global_args()` - Additional global args

### Command Builders

Each command is a separate builder. Available commands:

- **QueryCommand** - Execute queries with full option coverage (28 options)
- **McpListCommand** - List MCP servers
- **McpAddCommand** - Add HTTP or stdio MCP server
- **McpAddJsonCommand** - Add server from JSON config
- **McpRemoveCommand** - Remove MCP server
- **McpAddFromDesktopCommand** - Add desktop MCP server
- **McpResetProjectChoicesCommand** - Reset project-specific choices
- **PluginListCommand** - List plugins
- **PluginInstallCommand** - Install plugin
- **PluginUninstallCommand** - Uninstall plugin
- **PluginEnableCommand** - Enable plugin
- **PluginDisableCommand** - Disable plugin
- **PluginUpdateCommand** - Update plugin
- **PluginValidateCommand** - Validate plugin
- **AuthStatusCommand** - Check authentication status
- **VersionCommand** - Get CLI version
- **DoctorCommand** - Run health check
- **AgentsListCommand** - List available agents
- **RawCommand** - Escape hatch for unsupported options

## QueryCommand: The Workhorse

Full coverage of `claude -p` (print mode) options.

### All QueryCommand Options

| Method | CLI Flag | Type | Description |
|--------|----------|------|-------------|
| `model()` | `--model` | `&str` | Model alias or full ID |
| `system_prompt()` | `--system-prompt` | `&str` | Replace default system prompt |
| `append_system_prompt()` | `--append-system-prompt` | `&str` | Append to system prompt |
| `output_format()` | `--output-format` | `OutputFormat` | text, json, stream-json |
| `max_budget_usd()` | `--max-budget-usd` | `f64` | Spending cap |
| `permission_mode()` | `--permission-mode` | `PermissionMode` | default, acceptEdits, bypassPermissions, plan, auto |
| `allowed_tools()` | `--allowed-tools` | `&[&str]` | Tool permission allow list |
| `disallowed_tools()` | `--disallowed-tools` | `&[&str]` | Tool permission deny list |
| `tools()` | `--tools` | `&[&str]` | Restrict available tools |
| `mcp_config()` | `--mcp-config` | `&str` | MCP server config file |
| `strict_mcp_config()` | `--strict-mcp-config` | - | Only use MCP from config |
| `add_dir()` | `--add-dir` | `&str` | Additional accessible directories |
| `effort()` | `--effort` | `Effort` | low, medium, high |
| `max_turns()` | `--max-turns` | `u32` | Conversation turn limit |
| `json_schema()` | `--json-schema` | `&str` | Structured output validation |
| `agent()` | `--agent` | `&str` | Agent for session |
| `agents_json()` | `--agents` | `&str` | Custom agents JSON |
| `continue_session()` | `--continue` | - | Resume most recent session |
| `resume()` | `--resume` | `&str` | Resume by session ID |
| `session_id()` | `--session-id` | `&str` | Use specific session ID |
| `fork_session()` | `--fork-session` | - | Fork when resuming |
| `fallback_model()` | `--fallback-model` | `&str` | Fallback model |
| `no_session_persistence()` | `--no-session-persistence` | - | Don't save session |
| `dangerously_skip_permissions()` | `--dangerously-skip-permissions` | - | Bypass permissions |
| `file()` | `--file` | `&str` | File resources to download |
| `input_format()` | `--input-format` | `InputFormat` | text or stream-json |
| `include_partial_messages()` | `--include-partial-messages` | - | Partial chunks |
| `settings()` | `--settings` | `&str` | Settings JSON file |

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

Session management (low-level):

```rust
// Start new session
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

Session management (type-safe):

```rust
use claude_wrapper::Session;

// Create from a previous query result
let mut session = Session::from_result(&claude, &result);

// Auto-resumes the session
let next = session.query("what did we find?").execute().await?;

// Fork to branch the conversation
let mut forked = session.fork();
let alt = forked.query("try a different approach").execute().await?;

// Track cumulative cost and turns
println!("Total cost: ${}", session.cumulative_cost_usd());
println!("Total turns: {}", session.cumulative_turns());
```

## Streaming NDJSON Events

For real-time output, use `stream_query()`:

```rust
use claude_wrapper::streaming::stream_query;

let output = stream_query(&claude, &cmd, |event| {
    if event.is_result() {
        println!("Result: {}", event.result_text().unwrap_or(""));
    }
    if event.is_error() {
        eprintln!("Error: {}", event.error().unwrap_or(""));
    }
}).await?;
```

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

## MCP Config Builder

Generate `.mcp.json` files programmatically:

```rust
use claude_wrapper::McpConfigBuilder;

McpConfigBuilder::new()
    .http_server("hub", "http://127.0.0.1:9090")
    .stdio_server("tool", "npx", ["my-server"])
    .write_to("/tmp/my-project/.mcp.json")?;
```

Also supports:
- `env()` - Environment variables for servers
- `environment_file()` - Load from env file

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

## Escape Hatch: RawCommand

For unsupported options, use `RawCommand`:

```rust
let output = RawCommand::new()
    .arg("custom-subcommand")
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
