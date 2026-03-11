---
name: loop_monitor
description: Monitor GitHub PRs and report only meaningful changes on each iteration.
metadata:
  scope: coordinator
  arguments:
    - name: repo
      description: GitHub repo in owner/repo format (e.g., joshrotenberg/claude-wrapper)
      required: true
    - name: filters
      description: "Optional gh pr list filters (e.g., is:draft, label:pool:ready)"
      required: false
    - name: verbose
      description: "Report full table even if unchanged (default: false)"
      required: false
---

Monitor GitHub PRs in {repo}{filters_note} and report only changes.

## Workflow

### 1. Fetch Current State
```bash
gh pr list -R {repo} {filters} --json number,title,state,statusCheckRollup,reviewDecision,labels,updatedAt --limit 100
```

Parse as JSON array of PRs. Each PR needs: number, title, state (OPEN/DRAFT/MERGED/CLOSED), statusCheckRollup (PENDING/FAILURE/SUCCESS/NEUTRAL), reviewDecision (APPROVE/REQUEST_CHANGES/REVIEW_REQUIRED/COMMENTED), labels (array), updatedAt (timestamp).

### 2. Retrieve Previous State
Use mcp context_get key: "loop_monitor_state_{repo_slug}".

If nothing found, store current state and report:
"Initial snapshot of {repo}. {count} PRs. Monitoring now."
Then exit.

### 3. Diff: Identify Only Meaningful Changes

**New PRs** (in current, not in previous):
- Report: "NEW #{number}: {title} ({state})"

**Status Transitions** (state changed):
- DRAFT -> OPEN: "OPENED #{number}"
- OPEN -> MERGED: "MERGED #{number}"
- OPEN -> CLOSED: "CLOSED #{number}"

**Review Status Changes** (reviewDecision changed):
- -> REQUEST_CHANGES: "CHANGES REQUESTED #{number}"
- -> APPROVE: "APPROVED #{number}"

**Status Checks Changed** (statusCheckRollup changed):
- -> FAILURE: "CHECKS FAILING #{number}"
- FAILURE -> SUCCESS: "CHECKS PASSING #{number}"
- PENDING -> SUCCESS: "CHECKS COMPLETE #{number}"

**Label Changes** (labels added/removed):
- If `pool:ready` added: "LABELED pool:ready #{number}"
- If `pool:ready` removed: "UNLABELED pool:ready #{number}"

Skip cosmetic changes (comment count, updatedAt alone).

### 4. Format Output

If changes found:
```
## PR Monitor: {repo}

{list of changes, one per line, reverse-chronological}

Summary: {count} new, {count} status changes, {count} review updates, {count} check failures
Last check: {timestamp}
```

If no changes:
```
No changes to {repo}.
```

### 5. Store New State
Use mcp context_set key: "loop_monitor_state_{repo_slug}" with compact JSON:
```json
{
  "timestamp": "2025-03-10T14:35:00Z",
  "prs": [
    { "number": 68, "title": "docs: add task sizing", "state": "OPEN", "statusCheckRollup": "SUCCESS", "reviewDecision": null, "labels": ["docs"] }
  ]
}
```

## Error Handling

If `gh pr list` fails:
- Report: "Failed to fetch PRs: {error}"
- Don't update context

## Usage

`/loop 5m pool_skill_run skill: "loop_monitor" arguments: { "repo": "owner/repo", "filters": "is:draft" }`
