# Generate Prompt Evaluation Report

Evaluated `crates/claudes/src/generate_prompt.md` across 7 scenarios: simple bugfix, multi-file feature, research fan-out, refactoring, multi-issue parallel, chained workflow, and vague input.

---

## 1. Overall Assessment

The prompt is a solid 50-line foundation that correctly handles task decomposition heuristics and produces structurally valid manifests. It gets the easy cases right (single bugfix, parallel independent tasks) but breaks down on chain semantics, context flow between tasks, and edge cases (vague input, research needing web tools, already-done work).

**Grade: B-** — Correct structure, incomplete guidance. The manifests it produces will work for simple cases but require manual correction for chains, research tasks, and nuanced scenarios.

---

## 2. Strengths (Consistent Across Evaluations)

**Task decomposition heuristics work well.** Rules 1-4 correctly guide splitting decisions. The "Unclear? One task" default (rule 4) is the right conservative choice, validated by eval-vague. The parallel vs chain distinction (rules 2-3) was correct in eval-multi-issue and eval-chain.

**Post hooks for verification.** Every evaluation confirmed that the `post_hooks` guidance produces correct verification. The "use post_hooks, not self-reporting" instruction is one of the strongest parts of the prompt.

**Tool scoping guidance is good.** The per-task-type tool recommendations (code, review, PR) are specific and actionable. eval-feature and eval-chain both produced correctly scoped PR tasks.

**Common patterns are useful.** The bug fix chain, feature chain, and research fan-out patterns give the model concrete templates. eval-chain and eval-research both leveraged these directly.

**Conciseness.** At 50 lines, the prompt avoids overwhelming the model. Every evaluation noted this as a positive.

---

## 3. Weaknesses (Consistent Failures)

### 3.1 Chain + Worktree Isolation Interaction (Critical)

**Found in:** eval-feature, eval-refactor, eval-chain

Each task in a chain runs in its own worktree. If task B depends on code changes from task A, task B starts from the base branch and cannot see task A's changes. The prompt never explains this. Every chain-based evaluation produced manifests where downstream tasks assumed they could see upstream changes.

This is the single most impactful gap. It causes silent failures: the manifest looks correct but produces tasks that operate on stale code.

### 3.2 No Breadcrumb Documentation

**Found in:** eval-feature, eval-refactor, eval-chain

The runner automatically writes breadcrumbs for chained tasks, but the generate prompt never mentions this mechanism. Downstream task prompts either redundantly describe what upstream tasks did, or say "read the breadcrumb" without knowing the path. The model generating the manifest has no way to leverage breadcrumbs effectively.

### 3.3 No Guidance for Vague or Impossible Input

**Found in:** eval-vague, eval-refactor

The prompt assumes input is always actionable. For "make the code better" (eval-vague), there's no fallback to a research-first pattern. For "migrate to thiserror" when thiserror is already in use (eval-refactor), there's no instruction to flag the request as already satisfied. The model blindly generates manifests for work that may be unnecessary or too vague to be useful.

### 3.4 Missing Tool Names

**Found in:** eval-research, eval-multi-issue

The prompt lists `Read, Edit, Write, Glob, Grep, Bash(...)` as valid tool names but never mentions `WebSearch` or `WebFetch`. Research tasks that need external information have no way to request these tools. Similarly, `Bash(gh issue view *)` isn't suggested for issue-referencing tasks.

### 3.5 No `append_system_prompt` Guidance

**Found in:** eval-bugfix, eval-chain, eval-refactor

The prompt lists `append_system_prompt` as a shared field but never explains when or how to use it. The PROMPTING.md checklist says to include the Rust 2024 if-let chain instruction via `append_system_prompt`, but the generate prompt doesn't reference this. No evaluation produced a manifest that used it.

### 3.6 No Model Selection Heuristic

**Found in:** eval-vague, eval-research, eval-chain

The prompt mentions `claude-sonnet-4-6` and `claude-opus-4-6` as valid models but gives no guidance on when to choose one over the other. Vague tasks, synthesis tasks, and architecture decisions benefit from opus; mechanical tasks are fine with sonnet.

---

## 4. Specific Improvements (By Priority)

### P0 — Fix chain + worktree semantics

Add after the CHAINS section:

```
CHAIN EXECUTION — how chained tasks interact:
- Each task runs in its own worktree, even in chains
- Chained tasks share a branch: task A pushes, task B's worktree is created from that branch tip
- The runner writes breadcrumbs (summaries of what each task did) to .claudes/breadcrumbs/
- Downstream tasks automatically receive upstream breadcrumbs as context
- If task B needs task A's code changes, they MUST share a branch name
- Alternative: combine tightly coupled code changes into one task
```

### P1 — Add vague input handling

Add as rule 5 in TASK DESIGN:

```
5. Input too vague to identify files or specific changes? Generate a research task first
   (isolation: none, Read+Glob+Grep+Write only) that analyzes the codebase and writes a plan,
   then an implementation task that follows the plan.
```

### P1 — Document all valid tool names

Add a TOOLS section:

```
VALID TOOL NAMES for allowed_tools/disallowed_tools:
- Read, Edit, Write, Glob, Grep — file operations
- Bash(pattern) — shell commands matching glob (e.g., Bash(cargo *), Bash(git *), Bash(gh pr *))
- WebSearch, WebFetch — web access (use for research tasks needing external info)
- TodoWrite, NotebookEdit — rarely needed
```

### P1 — Add `append_system_prompt` guidance

Add to PROMPT BEST PRACTICES:

```
- Use append_system_prompt in shared block for project-wide rules:
  Rust projects: "Use Rust 2024 if-let chains. Do NOT modify files not named in the task prompt."
  This prevents common post_hook failures (collapsible_if, unexpected file edits).
```

### P2 — Add model selection heuristic

Add to TASK FIELDS after the model field:

```
  Heuristic: sonnet for mechanical tasks (formatting, simple fixes, test writing).
  opus for judgment-heavy tasks (architecture, synthesis, vague requirements, complex refactors).
```

### P2 — Add issue task pattern

Add to COMMON PATTERNS:

```
- Issue fix (parallel): one task per issue, each starts with `gh issue view N`,
  allowed_tools include Bash(gh issue *)
```

### P2 — Add regression test instruction

Add to PROMPT BEST PRACTICES:

```
- For bug fix tasks: always instruct the model to add a regression test
- For feature tasks: instruct the model to add tests unless a separate test task follows in the chain
```

### P3 — Add PR body template

Add to COMMON PATTERNS:

```
- PR body template (use in task prompts):
  ## Summary\n- [what changed and why]\n\n## Files changed\n- [list]\n\n## Test plan\n- [ ] [verification steps]
```

### P3 — Add "when NOT to chain" guidance

Add to CHAINS:

```
- Prefer one task over a chain when: total change is < 5 files, subtasks touch overlapping
  files, or the scope is small enough for one session. Chains add overhead and complexity.
```

### P3 — Add branch naming guidance

Add to TASK FIELDS:

```
- branch: use conventional naming — fix/description, feat/description, refactor/description.
  All tasks in a chain should share the same branch name.
```

---

## 5. Missing Guidance (Topics Not Covered)

| Topic | Impact | Where It Matters |
|-------|--------|-----------------|
| Chain + worktree interaction | Critical | Any chained code task |
| Breadcrumb mechanism | High | All chain patterns |
| Valid tool names (complete list) | High | Research tasks, issue tasks |
| `append_system_prompt` usage | High | Rust projects, file restriction enforcement |
| Vague input fallback | Medium | Ambiguous user requests |
| Model selection | Medium | All tasks (but especially vague/complex) |
| When NOT to create a PR task | Medium | Feature chains (PR auto-added when not requested) |
| When NOT to split into chain | Medium | Small refactors, tightly coupled changes |
| Existing code awareness | Medium | Refactors of already-migrated code |
| `output_path` convention for research | Low | Research fan-outs |
| File sharing semantics with isolation:none | Low | Research tasks |

---

## 6. Revised Prompt

```markdown
You generate claudes manifest JSON for running Claude Code tasks.

Think through the task decomposition carefully, then output the manifest as a JSON object.
Your response should end with the complete JSON manifest. Any reasoning should come before the JSON.
Do NOT run the manifest or execute any tasks — only output the JSON.

TASK DESIGN — decide how to split the work:
1. One coherent goal? One task.
2. All subtasks independent? Parallel tasks (no chains).
3. Subtasks need results from others? Use chains.
4. Unclear? One task. Splitting incorrectly is worse than not splitting.
5. Input too vague to identify files or changes? Generate a research/plan task first
   (isolation: none, Read+Glob+Grep+Write) that analyzes the codebase and writes a plan,
   followed by an implementation task. Do not generate open-ended edit tasks without scoping.
6. Total change < 5 files or subtasks touch overlapping files? Prefer one task over a chain.

CHAINS — declare sequential or fan-out dependencies:
- Linear: "chains": [["a", "b", "c"]] means a then b then c
- Fan-out: "chains": [["a", ["b1", "b2"], "c"]] means a, then b1+b2 parallel, then c
- Multiple chains merge dependencies
- All tasks in a chain MUST share the same branch name
- Each chained task runs in its own worktree, created from the branch tip after the
  previous task pushes. Downstream tasks see upstream code changes via the shared branch.
- The runner writes breadcrumbs (task summaries) that downstream tasks receive automatically.
  Do not duplicate upstream context in downstream prompts — breadcrumbs handle this.

MANIFEST FIELDS (all optional except tasks):
- version: always 1
- shared: defaults inherited by all tasks (model, isolation, post_hooks, allowed_tools,
  disallowed_tools, append_system_prompt)
- chains: dependency chains (array of arrays)
- tasks: array of task objects

TASK FIELDS:
- name: kebab-case identifier (required)
- prompt: detailed instructions (required)
- branch: git branch name (use fix/, feat/, refactor/, docs/ prefixes)
- depends_on: array of task names this depends on
- model: claude-sonnet-4-6 (default, for mechanical tasks) or claude-opus-4-6 (for
  judgment-heavy tasks: architecture, synthesis, vague requirements)
- isolation: {"type": "worktree", "base_dir": ".worktrees"} for code tasks,
  {"type": "none"} for research/plan tasks
- allowed_tools: array of tool names to permit
- disallowed_tools: array of tool names to block
- post_hooks: shell commands that must pass after task completes
- pre_hooks: shell commands that run before task starts
- append_system_prompt: additional system context injected into the task session

VALID TOOL NAMES:
- Read, Edit, Write, Glob, Grep — file operations
- Bash(pattern) — shell commands matching glob (Bash(cargo *), Bash(git *), Bash(gh *))
- WebSearch, WebFetch — web access for research tasks
- TodoWrite, NotebookEdit — rarely needed

PROMPT BEST PRACTICES:
- List every file the task may modify, plus an explicit "Do NOT modify" exclusion list
- Specify exact commit message using conventional format with scope
- Include PR creation only if the user requested it; use a full body template:
  ## Summary, ## Files changed, ## Test plan
- Use post_hooks for verification (cargo fmt, cargo test), not self-reporting
- For bug fix tasks: always instruct the model to add a regression test
- For issue references: start the prompt with `gh issue view N` and include Bash(gh *) in tools
- Use append_system_prompt in the shared block for project-wide rules (e.g., Rust 2024
  if-let chain instruction, file restriction enforcement)

TASK TYPE DEFAULTS:
- Code tasks: worktree isolation, allowed_tools [Read, Edit, Bash(cargo *), Bash(git *)],
  disallowed_tools [Write] unless new files needed
- Research/plan tasks: isolation none, allowed_tools [Read, Glob, Grep, Write, WebSearch, WebFetch]
- Review tasks: allowed_tools [Read, Glob, Grep], disallowed_tools [Edit, Write]
- PR tasks: allowed_tools [Read, Glob, Grep, Bash(git *), Bash(gh pr *)],
  disallowed_tools [Edit, Write]

COMMON PATTERNS:
- Bug fix: single task with regression test, or ["plan", "fix"] for complex bugs
- Feature chain: ["implement", "test", "pr"] — all share one branch
- Research fan-out: ["collect", ["research-1", "research-2"], "summarize"] — isolation none
- Multi-issue parallel: one task per issue, all independent, each with its own branch
- Vague input: ["research", "implement"] — research task scopes the work first
```

---

## Evaluation Coverage Matrix

| Scenario | Decomposition | Isolation | Tools | Chains | Prompt Quality |
|----------|:---:|:---:|:---:|:---:|:---:|
| eval-bugfix | PASS | PASS | GOOD | n/a | ADEQUATE (missing test guidance) |
| eval-feature | PASS | FAIL (worktree gap) | GOOD | FAIL (no shared branch) | ADEQUATE |
| eval-research | PASS | PASS | FAIL (no WebSearch) | PASS | GOOD |
| eval-refactor | PASS | FAIL (worktree gap) | PASS | OVERKILL | POOR (already done) |
| eval-multi-issue | PASS | PASS | WEAK (no gh issue) | n/a | ADEQUATE |
| eval-chain | PASS | FAIL (worktree gap) | GOOD | STRUCTURALLY OK | VAGUE (implement task) |
| eval-vague | PASS (rule 4) | n/a | PASS | n/a | NO FALLBACK |

The revised prompt addresses every FAIL and WEAK cell in this matrix.
