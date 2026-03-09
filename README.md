# claude-wrapper

A type-safe Claude Code CLI wrapper for Rust.

`claude-wrapper` provides a builder-pattern interface for invoking the `claude` CLI programmatically. Same design philosophy as [docker-wrapper](https://crates.io/crates/docker-wrapper) and [terraform-wrapper](https://crates.io/crates/terraform-wrapper): each CLI subcommand is a builder struct that produces typed output.

## Quick Start

```rust
use claude_wrapper::{Claude, ClaudeCommand, QueryCommand, OutputFormat};

#[tokio::main]
async fn main() -> claude_wrapper::Result<()> {
    let claude = Claude::builder().build()?;

    // Simple oneshot query
    let output = QueryCommand::new("explain this error: file not found")
        .model("sonnet")
        .output_format(OutputFormat::Json)
        .execute(&claude)
        .await?;

    println!("{}", output.stdout);
    Ok(())
}
```

## Design

Two-layer builder pattern:

1. **`Claude` client** - shared config (binary path, env vars, timeouts)
2. **Command builders** - per-subcommand options, `execute(&claude)` returns typed output

The `CLAUDECODE` environment variable is automatically removed when spawning processes, preventing the "cannot be launched inside another Claude Code session" error.

## Commands

### QueryCommand

The primary command. Wraps `claude -p` (print mode) with all options:

```rust
let output = QueryCommand::new("implement the feature described in TASK.md")
    .model("sonnet")
    .system_prompt("You are a senior Rust developer")
    .output_format(OutputFormat::Json)
    .max_budget_usd(1.00)
    .permission_mode(PermissionMode::AcceptEdits)
    .allowed_tools(["Bash", "Read", "Edit", "Write"])
    .mcp_config("/tmp/project/.mcp.json")
    .effort(Effort::High)
    .max_turns(10)
    .no_session_persistence()
    .execute(&claude)
    .await?;
```

With structured JSON output:

```rust
let result = QueryCommand::new("what is 2+2?")
    .execute_json(&claude)
    .await?;

println!("answer: {}", result.result);
println!("cost: ${:.4}", result.cost_usd.unwrap_or(0.0));
println!("session: {}", result.session_id);
```

Full option coverage:

| Method | CLI Flag | Description |
|--------|----------|-------------|
| `model()` | `--model` | Model alias or full ID |
| `system_prompt()` | `--system-prompt` | Replace default system prompt |
| `append_system_prompt()` | `--append-system-prompt` | Append to default system prompt |
| `output_format()` | `--output-format` | text, json, stream-json |
| `max_budget_usd()` | `--max-budget-usd` | Spending cap |
| `permission_mode()` | `--permission-mode` | default, acceptEdits, bypassPermissions, plan, auto |
| `allowed_tools()` / `allowed_tool()` | `--allowed-tools` | Tool permission allow list |
| `disallowed_tools()` | `--disallowed-tools` | Tool permission deny list |
| `tools()` | `--tools` | Restrict available built-in tools |
| `mcp_config()` | `--mcp-config` | MCP server config file path |
| `strict_mcp_config()` | `--strict-mcp-config` | Only use MCP from --mcp-config |
| `add_dir()` | `--add-dir` | Additional directories for tool access |
| `effort()` | `--effort` | low, medium, high |
| `max_turns()` | `--max-turns` | Maximum conversation turns |
| `json_schema()` | `--json-schema` | Structured output validation |
| `agent()` | `--agent` | Agent for the session |
| `agents_json()` | `--agents` | Custom agents JSON definition |
| `continue_session()` | `--continue` | Continue most recent conversation |
| `resume()` | `--resume` | Resume by session ID |
| `session_id()` | `--session-id` | Use specific session ID |
| `fork_session()` | `--fork-session` | Fork when resuming |
| `fallback_model()` | `--fallback-model` | Fallback when primary is overloaded |
| `no_session_persistence()` | `--no-session-persistence` | Don't save session to disk |
| `dangerously_skip_permissions()` | `--dangerously-skip-permissions` | Bypass permissions (sandbox only) |
| `file()` | `--file` | File resources to download |
| `input_format()` | `--input-format` | text or stream-json |
| `include_partial_messages()` | `--include-partial-messages` | Partial chunks in stream-json |
| `settings()` | `--settings` | Path to settings JSON |

### MCP Commands

```rust
// List servers
let output = McpListCommand::new().execute(&claude).await?;

// Get server details
let output = McpGetCommand::new("my-server").execute(&claude).await?;

// Add HTTP server
McpAddCommand::new("sentry", "https://mcp.sentry.dev/mcp")
    .transport("http")
    .scope(Scope::User)
    .execute(&claude).await?;

// Add stdio server with env vars
McpAddCommand::new("my-server", "npx")
    .server_args(["my-mcp-server"])
    .env("API_KEY", "xxx")
    .execute(&claude).await?;

// Add from raw JSON
McpAddJsonCommand::new("hub", r#"{"type":"http","url":"http://localhost:9090"}"#)
    .scope(Scope::Local)
    .execute(&claude).await?;

// Remove
McpRemoveCommand::new("old-server")
    .scope(Scope::Project)
    .execute(&claude).await?;
```

### MCP Config Builder

Generate `.mcp.json` files programmatically:

```rust
use claude_wrapper::McpConfigBuilder;

let path = McpConfigBuilder::new()
    .http_server("my-hub", "http://127.0.0.1:9090")
    .stdio_server("my-tool", "npx", ["my-mcp-server"])
    .stdio_server_with_env(
        "secure-tool", "node", ["server.js"],
        [("API_KEY", "secret")]
    )
    .write_to("/tmp/my-project/.mcp.json")?;
```

### Other Commands

```rust
// Version
let output = VersionCommand::new().execute(&claude).await?;

// Auth status
let status = AuthStatusCommand::new().execute_json(&claude).await?;
assert!(status.authenticated);

// Doctor (health check)
let output = DoctorCommand::new().execute(&claude).await?;

// Raw escape hatch
let output = RawCommand::new("agents")
    .arg("--setting-sources")
    .arg("user,project")
    .execute(&claude).await?;
```

### Streaming

For `--output-format stream-json` with real-time event processing:

```rust
use claude_wrapper::streaming::{StreamEvent, stream_query};

let cmd = QueryCommand::new("explain quicksort")
    .output_format(OutputFormat::StreamJson);

let output = stream_query(&claude, &cmd, |event: StreamEvent| {
    if event.is_result() {
        println!("Result: {}", event.result_text().unwrap_or(""));
    }
}).await?;
```

## Features

| Feature | Default | Description |
|---------|---------|-------------|
| `json` | Yes | JSON output parsing, `execute_json()`, `McpConfigBuilder`, streaming events |

## Status

This is an early-stage spike. The API is functional but may change.

### Implemented

- `Claude` client builder (binary, working dir, env, timeout, global args)
- `ClaudeCommand` trait
- `QueryCommand` with full `-p` option coverage (28 options)
- `McpListCommand`, `McpGetCommand`, `McpAddCommand`, `McpAddJsonCommand`, `McpRemoveCommand`, `McpAddFromDesktopCommand`, `McpResetProjectChoicesCommand`
- `PluginListCommand`, `PluginInstallCommand`, `PluginUninstallCommand`, `PluginEnableCommand`, `PluginDisableCommand`, `PluginUpdateCommand`, `PluginValidateCommand`
- `MarketplaceListCommand`, `MarketplaceAddCommand`, `MarketplaceRemoveCommand`, `MarketplaceUpdateCommand`
- `AgentsCommand`
- `AuthStatusCommand` with `execute_json()`
- `DoctorCommand`, `VersionCommand`
- `RawCommand` escape hatch
- `McpConfigBuilder` for generating `.mcp.json` files
- `stream_query()` for streaming NDJSON output
- Automatic `CLAUDECODE` env var removal
- Working directory support (`Claude::builder().working_dir()` and `claude.with_working_dir()`)

### Not Yet Implemented

- Retry/backoff policies
- Process lifecycle management (restart loops for agent patterns)

## License

MIT OR Apache-2.0
