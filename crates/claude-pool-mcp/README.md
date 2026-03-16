# claude-pool-mcp

MCP server exposing claude-pool as tools for Claude Code.

## Overview

`claude-pool-mcp` is a thin MCP server that wraps `claude-pool` as 31 tools. Every tool maps 1:1 to a pool method -- no business logic, no planning. The client (your interactive Claude session) decides what to run; the server dispatches.

## Usage

Add to your `.mcp.json`:

```json
{
  "mcpServers": {
    "claude-pool": {
      "command": "cargo",
      "args": ["run", "-p", "claude-pool-mcp", "--", "-n", "4", "--model", "sonnet"]
    }
  }
}
```

Or run directly:

```bash
claude-pool-mcp -n 4 --model sonnet --budget-usd 10.0
```

## CLI Options

| Flag | Default | Description |
|------|---------|-------------|
| `-n`, `--slots` | 2 | Number of worker slots |
| `-m`, `--model` | (none) | Default model for all slots |
| `-e`, `--effort` | (none) | Effort level (low, medium, high, max) |
| `-b`, `--budget-usd` | (none) | Total budget cap in USD |
| `-s`, `--system-prompt` | (none) | System prompt for all slots |
| `-p`, `--permission-mode` | plan | Permission mode (plan, auto, default, etc.) |
| `--min-slots` | 1 | Minimum slot floor |
| `--max-slots` | 16 | Maximum slot ceiling |

## Available Tools

### Execution
`pool_run`, `pool_submit`, `pool_result`, `pool_cancel`, `pool_fan_out`

### Auto-routing
`pool_auto`, `pool_auto_with_hints`, `pool_route`, `pool_route_with_hints`

### Chains
`pool_chain`, `pool_submit_chain`, `pool_chain_result`, `pool_cancel_chain`

### Review
`pool_submit_with_review`, `pool_approve_result`, `pool_reject_result`

### Status
`pool_status`, `pool_session_metrics`, `pool_list_tasks`, `pool_find_slots`

### Context
`pool_set_context`, `pool_get_context`, `pool_delete_context`, `pool_list_context`

### Messaging
`pool_send_message`, `pool_broadcast`, `pool_read_messages`, `pool_peek_messages`

### Scaling
`pool_scale_up`, `pool_scale_down`, `pool_set_target_slots`, `pool_drain`

## Coordinator Skill

See [`skills/pool-coordinator/SKILL.md`](skills/pool-coordinator/SKILL.md) for a ready-made skill you can add to your `CLAUDE.md` to get Claude to prefer pool tools over built-in Agent() calls.

## License

MIT OR Apache-2.0
