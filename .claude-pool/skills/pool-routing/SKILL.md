---
name: cps-coordinator-pool-routing
description: >-
  Guidance for the coordinator to choose between pool tools, native Agent(),
  and inline work. Promotes pool-first execution for parallel and mechanical tasks.
metadata:
  scope: coordinator
---

# Pool-Routing Guidance

You are a coordinator guiding task execution. This skill helps you choose the right execution path: **pool tools** (async/parallel), **Agent tool** (research), or **inline** (conversation/planning).

## Decision Tree

### 1. Is this an implementation task (write code, edit files, fix bugs, add features)?

**YES** → Use the **pool**: `pool_submit`, `pool_submit_chain`, `pool_fan_out`, or `pool_run`
- Mechanical tasks (file edits, refactors): `pool_run` or fire with `pool_submit`
- Complex multi-step tasks: `pool_submit_chain` for sequential workflows
- Independent instances (test variations, multiple PRs): `pool_fan_out` for parallel execution
- See **Model Selection** below for effort/model overrides

**NO** → Continue to question 2.

### 2. Is this a research task requiring MCP tools (crates.io, GitHub code search, documentation)?

**YES** → Use the **Agent tool** with specialized subagents:
- `Agent(..., subagent_type="Explore")` for codebase exploration and discovery
- Examples: "search for JWT implementations across the repo", "find all WebSocket usage", "explore dependency trees"
- Agent tool has MCP access; pool slots do not
- Use this when you need to investigate before deciding what to build

**NO** → Continue to question 3.

### 3. Is this planning, conversation, or clarification work?

**YES** → Work **inline** (native Claude):
- Answering user questions about the codebase or architecture
- Clarifying ambiguous requirements before delegating to pool
- Explaining what the code does or why changes are needed
- Asking for user input on approach or trade-offs
- Inline work is fast, synchronous, and context-aware

**NO** → You've covered the main categories. Default to **inline** for unclassified work.

---

## Pool Execution Patterns

### Quick Implementation Task
Use `pool_run` (synchronous, blocks until done):
```
pool_run with prompt: "Fix the login bug in src/auth.rs following the issue description"
```

### Fire and Check Later
Use `pool_submit` (asynchronous, returns task_id immediately):
```
task_id = pool_submit with prompt: "Implement the feature"
# ... do other work ...
result = pool_result(task_id)
```

### Sequential Workflow (E.g., Edit → Test → PR)
Use `pool_submit_chain` (ordered steps, each feeds into the next):
```
pool_submit_chain with steps:
  - name: "Implement"
    type: "prompt"
    value: "Fix the bug in {issue}"
  - name: "Test"
    type: "prompt"
    value: "Run tests to verify the fix works"
  - name: "Create PR"
    type: "prompt"
    value: "Create a PR referencing the issue"
```

### Parallel Independent Work
Use `pool_fan_out` (N independent tasks run in parallel):
```
pool_fan_out with prompts:
  - "Implement Feature A"
  - "Implement Feature B"
  - "Fix Bug C"
```

---

## Model Selection

Choose the right model for the task's complexity:

**Haiku** (fast, cost-efficient):
- Single-file edits
- Mechanical tasks (reformatting, renaming)
- Running tests or builds
- Simple bug fixes with clear root cause
- High-volume parallel work (fan-outs)
- Rebase and merge operations
- **Effort override**: `effort: "min"` or `effort: "low"`

**Sonnet** (balanced):
- Multi-file refactors with dependencies
- Code review that requires architectural reasoning
- Feature implementation requiring design decisions
- Tests and test infrastructure
- Complex git operations
- **Effort override**: `effort: "medium"` (default for most tasks)

**Opus** (most capable):
- Large mechanical refactors (whole-crate rewrites)
- Complex architectural reasoning
- Subtle logic bugs requiring deep analysis
- Design work that affects multiple systems
- **Effort override**: `effort: "high"` or `effort: "max"`

---

## Cost Awareness

Periodically check task spending:
```
pool_session_metrics()
```

Returns:
- Total spend and spend by model
- Task count by state (running, completed, failed)
- Timing distribution (p50, p95, max)
- Per-model breakdown

Use metrics to:
- Catch runaway tasks (unexpectedly high spend)
- Rebalance model selection if too many expensive tasks
- Understand typical task costs for planning future work

---

## Anti-Patterns

❌ **Do NOT:**
- Fire a task with `pool_submit` and immediately check `pool_result` in a loop (defeats async benefit; use `pool_run` instead)
- Use pool for conversation-style clarification (inline only)
- Use Agent() for implementation; use pool instead
- Chain unrelated steps together (use `pool_fan_out` instead)
- Assume pool tasks have local git state; they do (but context is fresh per slot)
- Ignore `pool_session_metrics`; cost surprises are avoidable

**✓ Do:**
- Fire a chain, then monitor it with `loop` and `chain_watcher` skill
- Use pool for anything that touches files, runs commands, or needs parallelism
- Use Agent() only for research with MCP tools
- Check metrics regularly to stay cost-aware
- Combine pool tools with inline coordination for complex workflows

---

## Examples

### Example 1: Fix a bug
1. **Inline**: Read the issue, understand the problem
2. **Pool**: `pool_run` with the description
3. **Inline**: Review the result, ask follow-ups if needed

### Example 2: Implement a feature + tests + PR
1. **Inline**: Clarify requirements
2. **Pool**: `pool_submit_chain` with steps: implement → write tests → create PR
3. **Inline**: Monitor with `chain_watcher`, approve the result

### Example 3: Multiple independent fixes
1. **Inline**: Collect all issues that are pool-ready
2. **Pool**: `pool_fan_out` with a prompt for each issue
3. **Inline**: Review each result

### Example 4: Investigate before deciding
1. **Agent**: `Agent(..., subagent_type="Explore")` to search the codebase
2. **Inline**: Analyze findings, decide on approach
3. **Pool**: Fire the implementation based on research
