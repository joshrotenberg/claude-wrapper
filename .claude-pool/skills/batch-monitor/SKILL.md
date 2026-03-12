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
      description: Comma-separated chain IDs to monitor (e.g., "abc123,def456")
      required: true
    - name: pr_numbers
      description: >-
        Optional comma-separated PR numbers (e.g., "231,232,233").
        If omitted, will be discovered from chain outputs.
      required: false
    - name: interval
      description: >-
        Polling interval in format "XmYs" (e.g., "3m", "30s", "2m30s").
        Default: "3m"
      required: false
    - name: mode
      description: >-
        Operating mode: "interactive" (v1, default, rebase + manual merge),
        "review" (v2, future), "headless" (v3, future).
      required: false
    - name: rebase_order
      description: >-
        Order for sequential rebases: "least-conflicts" (default, minimize conflicts),
        "submission-order" (fifo).
      required: false
---

# Batch Monitor — Fan-Out Coordination

You are a coordinator monitoring a batch of parallel chains/tasks that produce GitHub PRs. Your job is to orchestrate them through to completion: track progress, detect conflicts, suggest rebases, and report status.

---

## Phase 1: Initialization

### Step 1.1: Parse Arguments and Validate

Parse the arguments:
- `chain_ids` (required): Split by comma, trim whitespace. Example: `"abc123,def456"` → `["abc123", "def456"]`
- `pr_numbers` (optional): If provided, parse as comma-separated integers. Otherwise, set to empty.
- `interval` (optional): Default to `"3m"` if not provided. Validate format (e.g., `"3m"`, `"30s"`, `"2m30s"`).
- `mode` (optional): Default to `"interactive"`. Validate: must be one of `interactive`, `review`, `headless`.
- `rebase_order` (optional): Default to `"least-conflicts"`. Validate: must be one of `least-conflicts`, `submission-order`.

If validation fails, report the error and exit.

### Step 1.2: Report Startup Configuration

Output:
```
## Batch Monitor — Starting

| Config | Value |
|--------|-------|
| Chains | {count} |
| PRs | {count if known, else "TBD (discovery pending)"} |
| Interval | {interval} |
| Mode | {mode} (v1=interactive, v2=review, v3=headless) |
| Rebase order | {rebase_order} |

Startup: {timestamp}
```

Then suggest the `/loop` command for reference:
```
Suggested monitoring loop (if using /loop):
/loop {interval} check batch status for chains {chain_ids_csv}
```

---

## Phase 2: Chain Status Monitoring (Loop until all chains complete)

### Step 2.1: Check Each Chain Status

For each `chain_id`, use `pool_chain_result` to fetch status. Report:

```
## Chain Status — Tick {tick_number} at {timestamp}

| Chain ID | Status | Steps Complete | Current Step | Output |
|----------|--------|-----------------|--------------|--------|
| {id} | {running|completed|failed} | {X}/{Y} | {step_name or "—"} | {brief summary or error} |
| ... | | | | |

Summary: {count} running, {count} completed, {count} failed
```

### Step 2.2: Detect Chain Failures

If any chain has status `failed`:
- Report which chain failed and why (from `pool_chain_result` error output)
- **Do NOT exit** — continue monitoring other chains
- Flag in the summary as needing human intervention

### Step 2.3: Discover PR Numbers (if not provided upfront)

Once chains start completing, extract PR numbers from their outputs:
- Parse chain output for PR creation steps (e.g., "Created PR #231")
- Extract PR numbers using regex: `#(\d+)`
- Build a map: `chain_id → pr_number`

Update the status table with discovered PRs:
```
| Chain ID | Status | PR | ... |
| abc123 | completed | #231 | ... |
```

### Step 2.4: Loop Logic

- If all chains are completed or failed, advance to **Phase 3: PR Status Monitoring**
- Otherwise, sleep for `{interval}`, then repeat from Step 2.1

---

## Phase 3: PR Status Monitoring (Loop until all PRs are merged or flagged)

Once chains complete, monitor the PRs themselves. For each known PR:

### Step 3.1: Fetch PR Status

Use `gh pr view {pr_number} --json number,title,state,statusCheckRollup,reviewDecision,baseRefName,headRefName,commits` for each PR. Parse:

- `number`: PR number
- `title`: PR title
- `state`: OPEN, DRAFT, MERGED, CLOSED
- `statusCheckRollup`: PENDING, FAILURE, SUCCESS, NEUTRAL, STALE
- `reviewDecision`: APPROVE, REQUEST_CHANGES, REVIEW_REQUIRED, COMMENTED, null
- `baseRefName`: target branch (usually "main")
- `headRefName`: source branch
- `commits`: count of commits

Report:
```
## PR Status — Tick {tick_number} at {timestamp}

| PR | Title | State | Checks | Review | Mergeable | Action |
|----|-------|-------|--------|--------|-----------|--------|
| #231 | {title} | OPEN | ✓ SUCCESS | — | Yes | Ready to merge |
| #232 | {title} | OPEN | ✗ FAILURE | — | No | Waiting for CI |
| #233 | {title} | OPEN | ⧖ PENDING | REQUESTED | No | Conflict: needs rebase |
| ... | | | | | | |

Summary: {count} mergeable, {count} blocked by CI, {count} conflict, {count} merged
```

### Step 3.2: Detect Conflicts

For each OPEN PR:
1. Try to merge locally (dry run) using `git merge --no-commit --no-ff {head_ref}` on `main`
2. If merge conflict detected, mark as `Conflict: needs rebase`
3. Revert the test merge

### Step 3.3: Rebase PRs with Conflicts (Interactive Mode Only)

If `mode == "interactive"` and conflicts detected:

**Rebase Strategy:** Process PRs in `rebase_order`:

- **least-conflicts** (default): Rebase PRs with the fewest conflicting files first (reduces downstream conflicts)
- **submission-order**: Rebase PRs in the order chains were submitted

For each PR to rebase:

1. **Fetch latest main**: `git fetch origin main`
2. **Checkout PR branch**: `git checkout {head_ref}`
3. **Attempt rebase**: `git rebase origin/main`
   - If successful: `git push {head_ref} --force-with-lease`
   - If conflicts: Stop and flag for manual intervention (don't force-push garbage)
4. **Report outcome**:
   ```
   | #231 | {title} | OPEN | ✓ SUCCESS | — | Yes | ✓ Rebased |
   ```

### Step 3.4: Suggest Actions with Slash Commands

When PRs are ready, include actionable slash commands:

```
## Recommended Actions

Ready for review:
- `/pr-comments 231` — Review changes in #231
- `/pr-comments 232` — Review changes in #232

Waiting for CI:
- #233 checks still running, will auto-merge once green

At completion (merge all green PRs):
- Use GitHub UI to merge, or coordinate with automation
- Run `/cost` to review session spend
```

### Step 3.5: Loop Logic

- If all PRs are MERGED, advance to **Phase 4: Exit**
- If any PR is flagged for manual intervention (rebase failed, review required), report and **ask the coordinator** before proceeding
- Otherwise, sleep for `{interval}`, then repeat from Step 3.1

---

## Phase 4: Exit and Summary

### Step 4.1: Final Report

```
## Batch Monitor — Complete

| PR | Title | State | Result |
|----|-------|-------|--------|
| #231 | {title} | MERGED | ✓ Success |
| #232 | {title} | MERGED | ✓ Success |
| #233 | {title} | OPEN | ⚠ Manual merge needed (review pending) |

Summary:
- Completed: {count}
- Merged: {count}
- Needs manual intervention: {count}
- Total time: {duration}
- Total chains: {count}

Session cost:
/cost
```

### Step 4.2: Cleanup Suggestions

If context is heavy:
```
To free context after reviewing results, run:
/compact focus on: final PR summaries and any unresolved issues
```

---

## Mode Reference

### Mode 1: Interactive (v1, current MVP)

- **Rebase**: Automatic (sequential, least-conflicts-first)
- **Review**: None (human reviews PRs directly via GitHub or `/pr-comments`)
- **Merge**: Manual (coordinator merges via GitHub UI or `/merge` command)
- **Use case**: Developer in the loop, wants to review before merge

**Behavior:**
1. Monitor chains to completion
2. Discover PR numbers
3. Check PR status each tick
4. Auto-rebase conflicted PRs (halt on complex conflicts)
5. Report status and suggest next actions
6. Exit when all PRs are merged or flagged

### Mode 2: Review (v2, future)

- **Rebase**: Automatic (same as v1)
- **Review**: Posts AI-generated review comments on PRs
- **Merge**: Manual (after coordinator reviews the comments)
- **Use case**: Developer reviews AI feedback before merging

### Mode 3: Headless (v3, future)

- **Rebase**: Automatic (same as v1)
- **Review**: Automatic AI review with thresholds
- **Merge**: Automatic (when CI passes + review threshold met)
- **Use case**: Unattended batch work (e.g., bulk refactors, dependency updates)

---

## Error Handling and Edge Cases

### Chain Fails
- Report the error
- Continue monitoring other chains
- Flag in final summary as needing investigation

### PR Creation Step Missing from Chain Output
- If PR number cannot be discovered, ask coordinator to provide `pr_numbers` argument
- Exit with error and instructions

### Rebase Conflict Too Complex
- Detect multi-way conflicts or non-linear history
- Report: "Cannot auto-rebase #231 safely — too many conflicts. Manual intervention required."
- Do NOT force-push
- Flag for human intervention

### CI Timeout (checks stuck in PENDING)
- After 2+ hours of PENDING, report and ask coordinator if CI is hung
- Suggest `/pr-comments {pr}` to check status manually

### GitHub API Rate Limit
- Report the error
- Sleep longer (e.g., 30 minutes) and retry
- Suggest running `/cost` to check spending

---

## Coordinator Slash Commands Integration

The batch-monitor skill documents and leverages Claude Code slash commands as part of the human-coordinator workflow:

### Core Commands for Monitoring

| Command | When to use | Integration with batch-monitor |
|---------|------------|-------------------------------|
| `/loop [interval] <prompt>` | Set up recurring status checks | Skill suggests the right `/loop` invocation on startup |
| `/compact [instructions]` | Free context after a batch completes | Skill could suggest compact instructions preserving PR status |
| `/pr-comments [PR]` | Review worker-created PRs inline | Skill lists PRs ready for review with `/pr-comments` commands |
| `/cost` | Track coordinator + worker spend | Skill reports cumulative cost at completion |

### Session Management

| Command | When to use |
|---------|------------|
| `/resume [session]` | Pick up a previous coordination session |
| `/fork [name]` | Branch coordinator flow before risky operations (e.g., rebasing multiple PRs) |
| `/context` | Check context health during long coordination sessions |

### Recommended Coordinator Patterns

**Start of batch:**
```
/loop 3m batch-monitor {chain_ids}
```

**Mid-batch (context getting heavy):**
```
/compact focus on: pending PRs, chain status, unresolved conflicts
```

**Batch complete, PRs ready:**
```
/pr-comments 231
/pr-comments 232
```

**Long session spanning multiple batches:**
```
/cost
/context
```

---

## Implementation Notes for v1 (MVP)

The interactive mode (`v1`) is the MVP for this skill. Future modes will extend it:

- **v2** adds AI-generated review comments posted to PRs
- **v3** adds automatic merge thresholds and full automation

For v1, focus on:
1. Reliable chain status polling
2. Accurate PR discovery
3. Safe rebase logic (fail-safe on complex conflicts)
4. Clear, actionable reporting
5. Integration with slash commands for human coordination

---

## Example Usage

Start a batch-monitor session after fanning out three chains:

```
Pool ran 3 chains in parallel. Chain IDs: chain-abc, chain-def, chain-ghi

Fire batch-monitor:
pool_skill_run skill: "batch-monitor" arguments: {
  "chain_ids": "chain-abc,chain-def,chain-ghi",
  "interval": "3m",
  "mode": "interactive",
  "rebase_order": "least-conflicts"
}
```

The skill will then:
1. Poll chains every 3 minutes until complete
2. Discover PR numbers from chain outputs
3. Monitor PR status for conflicts and CI
4. Auto-rebase conflicted PRs (least-conflicts-first)
5. Suggest `/pr-comments` for review-ready PRs
6. Exit with final summary and merge instructions
