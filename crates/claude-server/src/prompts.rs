//! MCP prompts: message templates clients can pull into context.

use std::collections::HashMap;

use tower_mcp::protocol::GetPromptResult;
use tower_mcp::{Prompt, PromptBuilder};

use crate::state::ServerState;

pub(crate) fn prompts(_state: &ServerState) -> Vec<Prompt> {
    vec![prompt_describe_server(), prompt_usage_guide()]
}

fn prompt_describe_server() -> Prompt {
    PromptBuilder::new("describe_server")
        .description(
            "Ask the recipient LLM to describe this claude-server by \
             reading its own MCP resources. Zero args. Intended for \
             bootstrapping a new client / coordinator.",
        )
        .handler(|_args: HashMap<String, String>| async move {
            Ok(GetPromptResult::builder()
                .description("Summarize this claude-server via its resources.")
                .user(
                    "Read the MCP resources `claude://config` and `claude://tools` \
                     from this server, then summarize: what is this server, what \
                     tools does it expose, and what is the active configuration? \
                     Keep the summary under 200 words. Do not invoke any tools \
                     beyond reading these resources.",
                )
                .build())
        })
        .build()
}

fn prompt_usage_guide() -> Prompt {
    PromptBuilder::new("usage_guide")
        .description(
            "Pull the claude-server usage handbook into the calling \
             agent's context. Covers the three-surface taxonomy, the \
             async-by-default rule, common flows, error patterns, and \
             where to find live state. Read this once when you connect.",
        )
        .handler(|_args: HashMap<String, String>| async move {
            Ok(GetPromptResult::builder()
                .description("How to use this claude-server effectively.")
                .user(USAGE_GUIDE_TEXT)
                .build())
        })
        .build()
}

const USAGE_GUIDE_TEXT: &str = "\
# claude-server usage guide

You are connected to claude-server -- an MCP server that wraps the \
Claude Code CLI. Treat this as your operator manual.

## Three surfaces

1. **`claude_*`** -- 1:1 mirror of the `claude` CLI. Single-shot \
queries, version checks, agent listing, MCP / plugin / marketplace \
inspection, doctor.

2. **`chat_*`** -- long-lived multi-turn conversations. Each chat is \
a duplex `claude` subprocess held open across turns; turns within a \
chat serialize, chats run in parallel.

3. **`turn_*`** -- async lifecycle for in-flight turns. Whenever you \
fire an agent turn (via the bare `chat_send` or `claude_query`) you \
get back a `turn_id`; these tools let you observe and control it.

Plus `metrics_summary` for process-wide counters and the gated \
mutation surface (claude_mcp_add, claude_plugin_install, ...) when \
the server is started with `policy.allow_mutations = true`.

## The async-by-default rule

Agent turns are async. Always.

- `chat_send(chat_id, prompt)` returns `{ turn_id }` immediately. \
The turn runs in the background. Use `turn_wait(turn_id, \
timeout_secs)` to block, `turn_get(turn_id)` to poll, or \
`turn_cancel(turn_id)` to abort.

- `claude_query(prompt)` returns `{ turn_id }` immediately for \
single-shot queries.

- `chat_send_sync` and `claude_query_sync` exist as escape hatches: \
they hold your request connection open until the turn completes. \
**Use these only when you genuinely need to block.** The async \
variants are the right choice 99% of the time -- they let you fire \
multiple turns in parallel, observe progress, and cancel.

- `chat_send_stream_sync` is sync and emits MCP progress \
notifications during the turn so callers see assistant deltas as \
they arrive. Useful for streaming UIs; otherwise prefer async.

## Typical flows

### Single question, get the answer

```
turn_id = claude_query(prompt: \"explain this error\", model: \"haiku\").turn_id
result  = turn_wait(turn_id: turn_id, timeout_secs: 30)
# result.result is the JSON envelope: { result, session_id, total_cost_usd, ... }
```

### Multi-turn conversation

```
chat_id = chat_open(model: \"haiku\", max_cost_usd: 1.0).chat_id

turn1 = chat_send(chat_id: chat_id, prompt: \"hi\").turn_id
done1 = turn_wait(turn_id: turn1)

turn2 = chat_send(chat_id: chat_id, prompt: \"follow-up\").turn_id
done2 = turn_wait(turn_id: turn2)

chat_close(chat_id: chat_id)
```

### Talk to a different project

```
chat_open(model: \"haiku\",
          working_dir: \"/path/to/other/project\",
          system_prompt: \"you're working in project X\")
```
The spawned `claude` subprocess starts in `working_dir`. One server \
can hold parallel chats against multiple project roots.

### Resume an on-disk session

```
chat_open(resume: \"<session_id from ~/.claude/projects/...>\")
```
Picks up the conversation that produced `session_id`. Subsequent \
turns extend the existing history.

### Coordinator / fan-out pattern

Fire several `claude_query` or `chat_send` calls without waiting \
between them; each returns a `turn_id` instantly. Then either \
`turn_wait` each in sequence or use `turn_list` to inspect all \
in-flight turns. Different chats run in parallel; multiple turns \
within a single chat queue.

## Live state and observability

- `claude://chats` -- list of currently open chats with cost + turn count.
- `claude://chats/{id}` -- one chat's full snapshot: history, \
  budget, session_id, cumulative cost. Subscribable -- you'll get \
  `notifications/resources/updated` when a turn settles, no polling \
  needed.
- `claude://metrics` -- process counters: turns_fired / done / \
  failed / cancelled, in_flight, total_cost_usd. Same shape as \
  `metrics_summary`.
- `claude://config` -- sanitized server config (env values redacted \
  on KEY/TOKEN/SECRET/PASSWORD).
- `claude://tools` -- the registered tool surface; useful for \
  programmatic introspection.
- `claude://projects` (history feature) -- every project under \
  `~/.claude/projects/` with session counts. Pair with \
  `claude://projects/{slug}` for that project's session list and \
  `claude://sessions/{id}` for the full parsed entry log. Same \
  shape as the `claude_project_list` / `claude_session_list` / \
  `claude_session_get` tools.

Before firing an expensive turn, `metrics_summary` lets you check \
cumulative spend. Open chats with `max_cost_usd` to enforce a hard \
ceiling -- the next `chat_send` errors before touching claude if \
the budget is exhausted.

## Common error patterns

- **\"no chat with id X\"** from `chat_send` / `chat_history` / \
  `chat_budget`: the chat was closed or never opened. Check \
  `chat_list` first if you're not sure.
- **\"no turn with id X\"** from `turn_get` / `turn_wait`: same \
  shape. Either the turn never existed or it was evicted by the \
  TTL sweeper (default 1 hour after terminal). Re-fire if needed.
- **`turn_wait` returns `{ status: \"timeout\" }`**: the turn is \
  still running; the timeout elapsed. Poll again or cancel.
- **Budget exceeded**: `chat_send` errors before calling claude \
  when cumulative cost reaches `max_cost_usd`. Open a new chat or \
  cancel the budget on this one (currently no API for the latter \
  -- close + reopen).

## What this server does NOT cover

- Skills CRUD (`~/.claude/skills/`) -- planned, not yet wired
- Agents CRUD (`~/.claude/agents/`) -- planned
- Worktree introspection / removal -- planned

For now, those surfaces live elsewhere or require direct filesystem access.
";
