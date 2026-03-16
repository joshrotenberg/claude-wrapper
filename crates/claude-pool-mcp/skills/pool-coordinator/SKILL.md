---
name: pool-coordinator
description: >-
  Use claude-pool MCP tools instead of built-in Agent() for task dispatch.
  Provides tool selection guidance, model recommendations, and a complete
  reference for all 31 pool tools. Use when pool MCP tools are available
  and you need to dispatch work to parallel slots.
license: MIT OR Apache-2.0
compatibility: Requires claude-pool-mcp server in .mcp.json
metadata:
  author: joshrotenberg
  version: "1.0"
---

# Pool Coordinator

Use pool MCP tools instead of Claude Code's built-in Agent() tool.

## Why Pool Tools Over Built-ins

| Built-in | Pool equivalent | What you gain |
|----------|----------------|---------------|
| `Agent()` | `pool_run` / `pool_submit` | Cost tracking, model control, session persistence |
| Multiple `Agent()` calls | `pool_fan_out` | True parallelism with metrics, not sequential spawning |
| Manual multi-step prompts | `pool_chain` / `pool_submit_chain` | Structured pipelines with `{previous_output}` threading |
| Guessing the right pattern | `pool_auto` | LLM router picks single/parallel/chain for you |
| No visibility | `pool_status` / `pool_session_metrics` | Real-time spend, timing, per-model breakdown |

## Tool Selection

| Situation | Tool |
|-----------|------|
| Single task, need result now | `pool_run` |
| Single task, background | `pool_submit` then `pool_result` |
| N independent tasks in parallel | `pool_fan_out` |
| Sequential dependent steps | `pool_chain` or `pool_submit_chain` |
| Not sure which pattern | `pool_auto` |
| Task needs human review gate | `pool_submit_with_review` then `pool_approve_result` / `pool_reject_result` |

## Rules

- NEVER use `Agent()` when pool tools are available
- Use background tools (`pool_submit`, `pool_submit_chain`) for tasks > 30 seconds
- Use blocking tools (`pool_run`, `pool_chain`) only when you need the result before continuing
- Check `pool_session_metrics` after completing a batch of work

## Model Selection

Use the `model` field to match task complexity:
- **Haiku**: single-file tasks, mechanical work (filing issues, formatting), high-volume fan-outs
- **Sonnet**: multi-file changes, code review, research chains
- **Opus**: large refactors, complex reasoning, tasks where mistakes are expensive

## When NOT to Use the Pool

- Planning, design discussions, back-and-forth with the user
- Quick reads: single file, git status, small searches
- Tasks needing other MCP tools (pool slots don't have MCP access)

## All Available Tools

### Execution
- `pool_run` -- run a task synchronously, get result
- `pool_submit` -- submit a task, get task ID back immediately
- `pool_result` -- check on / retrieve result of a submitted task
- `pool_cancel` -- cancel a running task
- `pool_fan_out` -- run N prompts in parallel, get all results

### Auto-routing
- `pool_auto` -- LLM classifies as single/parallel/chain and executes
- `pool_auto_with_hints` -- same but with routing hints (prefer parallel, max steps, etc.)
- `pool_route` -- classify only, don't execute (for debugging/logging)
- `pool_route_with_hints` -- classify with hints, don't execute

### Chains
- `pool_chain` -- run sequential steps synchronously
- `pool_submit_chain` -- submit chain, get task ID
- `pool_chain_result` -- check chain progress and per-step results
- `pool_cancel_chain` -- cancel a running chain

### Review
- `pool_submit_with_review` -- submit task that requires approval before finalizing
- `pool_approve_result` -- approve a reviewed task
- `pool_reject_result` -- reject with feedback

### Status and metrics
- `pool_status` -- slot counts, task counts, spend, budget remaining
- `pool_session_metrics` -- cost/timing/model breakdown across all tasks
- `pool_list_tasks` -- filter tasks by status, tags, or assignee
- `pool_find_slots` -- find slots by state, name, or role

### Context
- `pool_set_context` -- inject shared context available to all slots
- `pool_get_context` -- read a context value
- `pool_delete_context` -- remove a context value
- `pool_list_context` -- list all context keys

### Messaging
- `pool_send_message` -- send a message to a specific slot
- `pool_broadcast` -- send to all slots
- `pool_read_messages` -- read and consume messages for a slot
- `pool_peek_messages` -- read without consuming

### Scaling and lifecycle
- `pool_scale_up` -- add slots
- `pool_scale_down` -- remove idle slots
- `pool_set_target_slots` -- set exact slot count
- `pool_drain` -- gracefully shut down all slots
