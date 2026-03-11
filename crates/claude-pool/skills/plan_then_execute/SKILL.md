---
name: plan_then_execute
description: >-
  Two-phase workflow: plan in read-only mode, then execute after review.
  Use for complex or risky tasks where you want to approve the approach
  before implementation begins.
argument-hint: "<task-description>"
metadata:
  scope: chain
  arguments:
    - name: task
      description: Description of the task to plan and execute.
      required: true
---

Execute a two-phase plan-then-execute workflow for the given task.

## Phase 1: Plan (Read-Only)

Analyze the task and produce a detailed implementation plan. During this phase you MUST NOT modify any files. Only read, search, and explore.

Task: {task}

Your plan should include:
1. **Summary**: One-sentence description of what will change
2. **Files to modify**: List each file and what changes are needed
3. **Files to create**: Any new files needed (justify why)
4. **Testing strategy**: How you will verify the changes work
5. **Risks**: Anything that could go wrong or needs careful handling

Output the plan as structured markdown. Do NOT implement anything yet.

## Phase 2: Execute

After the plan is reviewed and this phase begins, implement the plan exactly as described. Run all relevant checks (fmt, clippy, test) and fix any issues.

If the task includes creating a PR, do so. Otherwise, commit the changes and report what was done.

## Usage as a Chain

This skill is designed to be run as a two-step chain:
- Step 1: Plan phase with read-only tools (Read, Grep, Glob, Bash)
- Step 2: Execute phase with full tools, receiving the plan as input

The coordinator reviews the plan between steps and can abort if needed.
