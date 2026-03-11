# Design: headless `--run` mode for unattended pool execution

## Summary

Add a `--run "prompt"` mode to `claude-pool-server` that starts a pool, spawns a coordinator Claude with pool MCP tools, passes through the user's prompt, and exits when done. Same mental model as `claude -p "do the thing"` but with N worker slots behind a coordinator.

```
User runs:  claude-pool-server --run "implement feature X" -n 4 -b 5.00
                    │
                    ▼
         ┌─────────────────────┐
         │  Coordinator Claude  │  ← single Claude with pool MCP tools
         │  (the "brain")       │
         └────────┬────────────┘
                  │ MCP calls: pool_run, pool_chain, pool_fan_out...
                  ▼
         ┌─────────────────────┐
         │   Pool (N slots)     │  ← worker Claude instances
         └─────────────────────┘
```

## Motivation

Today you can get this behavior manually: start the pool server as an MCP server, configure a `.mcp.json` pointing at it, and run `claude -p`. The infrastructure is all there. But it requires manual setup and there's no single-command way to do it. A `--run` mode makes the pool usable in CI, scripts, and unattended workflows.

## Proposed Architecture

### Transport: HTTP loopback

The coordinator needs to talk to the pool via MCP. The options are:

| Approach | Pros | Cons |
|----------|------|------|
| **(a)** Spawn pool-server as child, stdio MCP | Simple | Two processes, lifecycle management |
| **(b)** In-process HTTP on localhost | Single process, clean | Needs port management |
| **(c)** Skip MCP, call pool directly | Fastest | Coordinator loses MCP tool descriptions |

**Recommendation: (b) HTTP loopback.** The pool starts its HTTP transport on `127.0.0.1:0` (random port), discovers the assigned port via `listener.local_addr()`, generates a `TempMcpConfig` pointing at that URL, and passes it to the coordinator Claude. Single process, clean shutdown, and the coordinator sees the full MCP tool surface including instructions.

### Coordinator setup

```rust
// Pseudocode
let listener = TcpListener::bind("127.0.0.1:0").await?;
let port = listener.local_addr()?.port();

// Start HTTP MCP server in background task
let app = build_http_app(router, BearerTokens::new(vec![]));
tokio::spawn(async move { axum::serve(listener, app).await });

// Generate temp MCP config for coordinator
let mcp_config = McpConfigBuilder::new()
    .http_server("claude-pool", format!("http://127.0.0.1:{port}"))
    .build_temp()?;

// Run coordinator
let result = QueryCommand::new(&prompt)
    .mcp_config(mcp_config.path())
    .permission_mode(coordinator_permission_mode)
    .output_format(OutputFormat::Json)
    .system_prompt(headless_system_prompt)
    .execute(&coordinator_claude)
    .await;

// Cleanup
let summary = pool.drain().await?;
```

### Coordinator permission mode

The coordinator only calls MCP tools (pool operations) — it doesn't edit files directly. Needs its own permission mode flag, probably defaulting to something non-interactive. The slots already get their permission mode from pool config.

New flag: `--coordinator-permission-mode` (default: whatever makes sense for "only calling MCP tools").

### Coordinator system prompt

Prepend headless-specific guidance to the existing pool instructions:

> You are running unattended without human supervision. Do not ask clarifying questions — make reasonable choices and document your reasoning. If a task is ambiguous, prefer the most conservative interpretation. Report what you did, what succeeded, and what failed.

### Streaming output

`stream_query()` supports a handler callback. In `--run` mode, stream the coordinator's output to stderr (so stdout can be reserved for structured results). This gives the user real-time visibility into what the coordinator is doing.

### Result output

On completion, write structured JSON to stdout:

```json
{
  "success": true,
  "output": "...",
  "cost_usd": 2.34,
  "tasks_completed": 7,
  "duration_secs": 180,
  "exit_code": 0
}
```

Exit code: 0 on success, 1 on failure, 2 on timeout.

## Issues to Solve

### 1. Global timeout (`--timeout`)

Neither `drain()` nor `fan_out()` have timeouts — both can block indefinitely. Need:

- A `--timeout` flag for the overall `--run` invocation (e.g., `--timeout 30m`)
- `tokio::time::timeout` wrapping the coordinator execution
- On timeout: cancel the coordinator, drain the pool (with its own bounded wait), exit with code 2
- `drain()` itself needs a timeout parameter — today it polls every 100ms with no upper bound (`pool.rs:1019-1032`)

### 2. Signal handling (SIGTERM/SIGINT)

**There is currently zero signal handling in the codebase.** If the process receives SIGTERM:

- No worktree cleanup (no `Drop` impl on `WorktreeManager`)
- No budget finalization
- No graceful drain of in-flight tasks
- Spawned `tokio::spawn` tasks are orphaned

Need a `tokio::signal` handler that:
1. Sets the pool's shutdown flag
2. Calls `drain()` with a bounded timeout
3. Cleans up worktrees
4. Exits

This is needed for `--run` mode but is also a general correctness issue for the existing server.

### 3. Supervisor interference during drain

`drain()` does **not** stop the supervisor. The supervisor may restart errored slots while drain is trying to wind down. `drain()` should stop the supervisor first (or the supervisor should respect the shutdown flag).

### 4. Orphaned tasks on coordinator crash

If the coordinator Claude times out or crashes mid-`fan_out`:
- Spawned `tokio::spawn` tasks continue running (detached)
- Their costs update the atomic `total_spend` if they complete
- But if the process exits, everything is lost (in-memory store)
- Task state stuck as `Running` in the store — supervisor only restarts `Errored` slots, not stuck tasks

For `--run` mode this is manageable (we control the lifecycle), but worth noting.

### 5. Worktree cleanup robustness

Current state:
- No `Drop` impl on `WorktreeManager`
- Cleanup only happens via explicit `cleanup_all()` call in `drain()`
- If killed, stale worktrees accumulate in `/tmp/claude-pool/worktrees/`
- Next pool startup does clean up stale worktrees for reused slot IDs (`pool.rs:76-79`)

For `--run` mode: signal handler should call `cleanup_all()`. Consider also a startup sweep that prunes any stale `claude-pool` worktrees.

## New CLI Flags

```
--run <PROMPT>                    Run prompt with coordinator and exit
--timeout <DURATION>              Global timeout (e.g. "30m", "2h") [default: none]
--coordinator-permission-mode     Permission mode for the coordinator [default: TBD]
--output-file <PATH>              Write result JSON to file instead of stdout
--quiet                           Suppress streaming output to stderr
```

All existing flags (`-n`, `-m`, `-e`, `-b`, `-w`, `--mcp-config`, etc.) continue to work and configure the pool as usual.

## Scope / Non-Goals

**In scope:**
- `--run` mode: single prompt -> coordinator -> pool -> result -> exit
- Global timeout
- Signal handling for graceful shutdown
- Structured result output
- Streaming coordinator output to stderr

**Out of scope (future work):**
- Startup plan files (JSON/YAML defining chains/workflows to run)
- Persistent state across runs (Redis/SQLite store)
- Webhook/callback notifications
- Scheduled/cron execution
- Per-task or per-slot budget caps
- Metrics export (Prometheus)

## Implementation Plan

1. Add signal handling + drain timeout (prerequisite, benefits existing server too)
2. Fix supervisor/drain coordination
3. Add `--run` flag and HTTP loopback setup
4. Add coordinator system prompt and permission mode
5. Add streaming output and structured result JSON
6. Add `--timeout` flag
7. Tests: headless execution, timeout behavior, signal handling, cleanup
