---
name: chain_watcher
description: >-
  Watch one or more active chains and report step progress changes.
  Designed for /loop usage to babysit long-running chains.
argument-hint: "[chain-id,chain-id,...]"
metadata:
  scope: coordinator
  arguments:
    - name: chain_ids
      description: Comma-separated chain task IDs to watch (only needed on first run).
      required: false
---

Monitor active chains and report progress changes.

## Step 1: Gather State

1. Get previous state: `context_get` key: `chain_watcher_state`
2. Get the list of chain task IDs to watch from the previous state, or from the arguments if this is the first run.
3. For each chain ID, call `pool_chain_result` to get current progress.

## Step 2: Detect Changes

For each chain, compare current vs previous:
- **Step transitions**: step N completed, step N+1 started
- **Completions**: chain finished (success or failure)
- **Cost accumulation**: new spend since last check
- **Retries**: if a step used retries, note it
- **Failures**: step failed, chain stopped

## Step 3: Format Output

Compact per-chain status line:
```
chain-abc: [3/5] running "test" ($0.08)
chain-def: [5/5] completed ($0.15)
chain-ghi: [2/4] FAILED at "build" ($0.04)
```

If changes since last check, add details:
```
Changes:
- chain-abc: step "lint" completed (0 retries), started "test"
- chain-def: completed successfully, total cost $0.15
```

If no changes: show status lines with "(no changes)".

## Step 4: Store State

Call `context_set` key: `chain_watcher_state` with JSON:
```json
{
  "chain_ids": ["chain-abc", "chain-def"],
  "snapshots": {
    "chain-abc": { "step": 2, "status": "running", "cost": 8000 },
    "chain-def": { "step": 4, "status": "completed", "cost": 15000 }
  }
}
```

## Usage

`/loop 1m pool_skill_run skill: "chain_watcher" arguments: { "chain_ids": "chain-abc,chain-def" }`

On subsequent iterations, chain IDs are loaded from context state. Completed/failed chains are kept in the report until removed.
