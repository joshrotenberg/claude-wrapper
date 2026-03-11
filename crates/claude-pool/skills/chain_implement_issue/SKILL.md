---
name: chain_implement_issue
description: >-
  Chain workflow for implementing a GitHub issue: plan, implement, rebase,
  then test and create a PR. Designed for use with pool_chain.
argument-hint: "<issue-number>"
metadata:
  scope: chain
  arguments:
    - name: issue
      description: GitHub issue number to implement.
      required: true
---

Implement a GitHub issue end-to-end using a four-phase chain workflow.

## Phase 1: Plan

Read the issue with `gh issue view {issue}` and analyze the codebase to produce a detailed implementation plan.

- Identify which files need to change and what changes are required.
- Note any risks, dependencies, or ambiguities.
- Determine the branch name (e.g., `feat/issue-{issue}-short-description` or `fix/issue-{issue}-short-description`).
- Output a structured plan as markdown. Do NOT modify any files in this phase.

## Phase 2: Implement

Execute the plan from Phase 1:

- Create the branch.
- Make all code changes described in the plan.
- Run `cargo fmt --all` to format.
- Run `cargo clippy --all-targets --all-features -- -D warnings` and fix any warnings.
- Commit the changes with a conventional commit message referencing the issue.

## Phase 3: Rebase

Rebase onto the latest main to avoid merge conflicts in the PR:

- `git fetch origin && git rebase origin/main`
- Resolve any conflicts, preferring the feature branch's intent.
- Run `cargo check` to verify the result compiles.

## Phase 4: Test and PR

Run the full test suite and create a pull request:

- `cargo test --lib --all-features`
- `cargo test --doc --all-features`
- Fix any test failures.
- Create a PR with `gh pr create` referencing the issue (e.g., "Closes #{issue}").
- Report the PR URL.

## Usage as a Chain

```
pool_chain steps:
  - skill: "plan_then_execute"  # or just inline planning
    tools: [Read, Grep, Glob, Bash]
  - skill: "implement"
    tools: [Read, Grep, Glob, Bash, Edit, Write]
  - skill: "rebase_onto_main"
    tools: [Bash]
  - skill: "pre_push"
    tools: [Bash, Edit, Write]
```

The coordinator reviews output between phases and can abort if any phase fails.
