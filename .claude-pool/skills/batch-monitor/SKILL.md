---
name: batch-monitor
description: >-
  Monitor fan-out chains/tasks through to PR completion. Checks chain status,
  PR status, detects conflicts, and suggests rebases. Runs at coordinator level.
argument-hint: "<chain_ids> [pr_numbers] [interval] [mode] [rebase_order]"
metadata:
  scope: coordinator
  arguments:
    - name: chain_ids
      description: Comma-separated chain IDs to monitor
      required: true
    - name: pr_numbers
      description: Comma-separated PR numbers (discovered from chain output if omitted)
      required: false
    - name: interval
      description: "Polling interval (default: 3m)"
      required: false
    - name: mode
      description: "interactive (default), review (future), headless (future)"
      required: false
    - name: rebase_order
      description: "least-conflicts (default) or submission-order"
      required: false
---

# Batch Monitor

You are monitoring a batch of parallel chains that produce PRs. Track progress, detect conflicts, rebase when safe, and report status with actionable next steps.

## Phase 1: Initialize

Parse `chain_ids` (comma-separated, required). Default `interval` to `3m`, `mode` to `interactive`, `rebase_order` to `least-conflicts`.

Report startup config and suggest: `/loop {interval} check batch status for chains {chain_ids}`

## Phase 2: Chain Monitoring

Each tick, for each chain_id call `pool_chain_result`. Report:

```
| Chain ID | Status | Steps | Current Step | Output |
|----------|--------|-------|--------------|--------|
| {id}     | {status} | {X}/{Y} | {name}   | {brief} |
```

- On failure: report error, continue monitoring others, flag for human
- On completion: extract PR number from output (regex `#(\d+)`)
- When all chains complete or fail, advance to Phase 3

## Phase 3: PR Monitoring

Each tick, check each PR with `gh pr view`. Report:

```
| PR | State | Checks | Mergeable | Action |
|----|-------|--------|-----------|--------|
| #{n} | {state} | {status} | {yes/no} | {action} |
```

### Conflict Detection and Rebase

If a PR has conflicts and mode is `interactive`:
1. Fetch latest main
2. Checkout PR branch, attempt `git rebase origin/main`
3. If clean: `git push --force-with-lease` and report success
4. If conflicts: stop, do NOT force-push, flag for human intervention

Process in `rebase_order`: least-conflicts rebases PRs with fewest conflicting files first.

### Suggest Actions

For review-ready PRs, output: `/pr-comments {pr_number}`

## Phase 4: Exit

When all PRs are merged or flagged:

```
| PR | State | Result |
|----|-------|--------|
| #{n} | MERGED | Success |

Summary: {merged} merged, {flagged} need intervention
Duration: {time}
```

Suggest `/compact focus on: final PR summaries` if context is heavy.

## Error Handling

- **Chain fails**: report, continue others, flag in summary
- **PR not discoverable**: ask coordinator for `pr_numbers` argument
- **Complex rebase**: refuse to force-push, flag for human
- **CI stuck 2+ hours**: report, suggest manual check
