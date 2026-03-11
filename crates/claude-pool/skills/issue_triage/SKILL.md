---
name: issue_triage
description: "Analyze a GitHub issue and post a structured triage comment with scope, risk, and approach."
argument-hint: "<issue-number>"
metadata:
  scope: coordinator
  arguments:
    - name: issue
      description: GitHub issue number to triage.
      required: true
---

Perform a structured triage analysis of a GitHub issue and post findings as a comment.

## Step 1: Read the Issue

Fetch the issue details:
```
gh issue view {issue} --json number,title,body,labels,author,assignees
```

## Step 2: Analyze the Codebase

Based on the issue description, explore the codebase to understand:
- Which files and modules are affected
- How large the change is likely to be (small/medium/large)
- Whether tests exist for the affected code
- Any related issues or PRs

## Step 3: Assess Risk

Evaluate:
- **Breaking changes**: Could this affect public API?
- **Test coverage**: Are the affected paths well-tested?
- **Complexity**: Is this a straightforward change or does it touch many modules?
- **Dependencies**: Does this require upstream changes or coordination?

## Step 4: Post Triage Comment

Post a structured comment on the issue using `gh issue comment {issue} --body`:

```markdown
## Triage Analysis

**Scope**: [small | medium | large]
**Risk**: [low | medium | high]
**Estimated effort**: [description]

### Affected Files
- `path/to/file.rs` - [what changes]
- ...

### Proposed Approach
1. [Step-by-step plan]

### Risks and Considerations
- [Any concerns]

### Recommendation
[Proceed / Needs clarification / Needs design discussion / Too large to automate]
```

## Step 5: Apply Label

- If the issue is clear and actionable: add label `pool:triage` (ready for human review)
- If the issue needs clarification: add label `pool:blocked`, post a comment asking specific questions
- Do NOT proceed to implementation -- wait for human approval via `pool:accepted` label
