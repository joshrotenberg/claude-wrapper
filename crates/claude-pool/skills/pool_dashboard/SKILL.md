---
name: pool_dashboard
description: >-
  Monitor pool health: slot utilization, queue depth, budget spend,
  and active chains. Reports only changes between iterations. Designed
  for /loop usage.
metadata:
  scope: coordinator
---

Generate a pool health dashboard. Compare against previous state and report changes.

## Step 1: Gather Current State

Call these MCP tools and collect the results:
1. `pool_status` - get slot counts, task counts, spend, budget
2. `pool_find_slots` with no filters - get all slot details
3. `context_get` key: `pool_dashboard_state` - get previous snapshot

## Step 2: Compare and Detect Changes

Compare current vs previous snapshot. Track:
- **Slot state changes**: idle->busy, busy->idle, errored, stopped
- **Queue depth changes**: pending tasks increasing (backlog) or clearing
- **Budget consumption**: spend delta since last check, % remaining
- **New completions**: tasks completed since last check
- **Errors**: any slots in errored state, failed tasks

## Step 3: Format Output

Always show the compact dashboard header:
```
Pool: {idle}/{total} idle | Queue: {pending} pending, {running} running | Spend: ${spend/100} / ${budget/100}
```

If changes detected, add a Changes section:
```
Changes:
- slot-0: idle -> busy (task-abc)
- 3 tasks completed ($0.12 spent)
- Budget: 45% -> 52% consumed
```

If no changes: just show the header line with "(no changes)".

## Step 4: Store State

Call `context_set` with key `pool_dashboard_state` and a compact JSON snapshot of current state for next iteration comparison.

## Usage

`/loop 2m pool_skill_run skill: "pool_dashboard"`
