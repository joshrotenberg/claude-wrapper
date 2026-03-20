# claude-runner

Autonomous GitHub issue runner — turns issues into pull requests.

## How it works

```
GitHub Issue  →  Triage  →  Workflow  →  Stages  →  Pull Request
```

Point it at a GitHub issue, it decides if the issue is ready, picks a workflow template, executes stages (plan, implement, test, review), and opens a PR. Each stage runs a bounded Claude Code session in an isolated git worktree.

## Quick start

```bash
# Process a single issue
claude-runner issue --repo owner/name --number 123

# Preview without executing
claude-runner issue --repo owner/name --number 123 --dry-run

# Poll for eligible issues
claude-runner run --repo owner/name

# Watch mode (continuous polling)
claude-runner watch --repo owner/name --interval 300
```

## Concepts

### Runner

The claude-runner process. Watches for work, processes issues, manages the lifecycle. Runs on a laptop, in CI, or on a VM. Stateless between runs — all state lives in GitHub (issues, PRs, labels) and local run records.

### Policy

Per-repo configuration controlling automation behavior. Lives in `runner.toml`.

```toml
repo = "owner/name"
eligible_labels = ["bug", "enhancement"]
exclude_labels = ["blocked", "needs-design"]
branch_pattern = "automation/{issue}-{slug}"
max_concurrency = 3
auto_merge = false
agent = "claude"
model = "claude-sonnet-4-6"
validation_commands = ["cargo test", "cargo clippy"]
```

### Issue Candidate

A GitHub issue that the runner is evaluating. Normalized from the GitHub API into a platform-independent representation.

### Triage

The decision gate: is this issue ready for automation? Outcomes:

- **Ready** — proceed to planning and execution
- **Needs clarification** — post questions as a comment, wait for response
- **Out of scope** — excluded by labels, too vague, or not eligible
- **Already in progress** — assigned or has an active lease
- **Blocked** — depends on another issue or external work
- **Duplicate** — matches an existing issue

### Workflow Template

The stage chain for a type of work. Defines which stages run and in what order. Built-in templates:

| Template | Stages | Use when |
|----------|--------|----------|
| **bug** | plan → implement → test → review → open_pr | Bug fixes |
| **feature** | clarify → plan → implement → test → review → open_pr | New features |
| **chore** | implement → test → open_pr | Maintenance tasks |
| **research** | research → comment | Investigation (no PR) |

Custom templates can be defined in `runner.toml`.

### Stage

A bounded unit of work within a run. Each stage has:

- A **kind** (plan, implement, test, review, open_pr, etc.)
- A **prompt** tailored to its job
- **Tool scoping** (plan is read-only, implement can edit, review can't write)
- **Retry policy** and optional timeout
- An **optional** flag (can be skipped without failing the run)

### Run

A persisted attempt to process one issue through a workflow. Tracks per-stage results, cost, duration, and branch/PR references. Stored in `~/.claude-runner/runs/`.

### Lease

A claim on an issue preventing duplicate work. Implemented as GitHub labels (`runner:in-progress`, `runner:complete`). Visible, auditable, no external state needed.

### Agent Adapter

The execution backend. Pluggable trait — Claude is the default, but any agent that can run bounded tasks in a working directory can be used.

## Architecture

Three separated layers:

```
┌─────────────────────────────────────────────────────┐
│  Platform Layer (github.rs)                          │
│  Issues, PRs, comments, labels via gh CLI            │
├─────────────────────────────────────────────────────┤
│  Domain Layer (policy, triage, planner, workflow)    │
│  Orchestration logic — what to do and when           │
├─────────────────────────────────────────────────────┤
│  Execution Layer (executor, isolation)               │
│  Agent invocation and work isolation                 │
└─────────────────────────────────────────────────────┘
```

The domain layer never touches GitHub API shapes or agent CLI details. The platform and execution layers are swappable.

## Stage kinds

| Kind | Purpose | Tools | Isolation |
|------|---------|-------|-----------|
| `plan` | Research codebase, write implementation plan | Read, Glob, Grep, Write | Read-only |
| `implement` | Make code changes | Read, Edit, Write, Bash | Worktree |
| `test` | Run tests, fix failures | Read, Edit, Bash | Worktree |
| `review` | Check changes for quality | Read, Glob, Grep | Read-only |
| `open_pr` | Push branch, create PR | git, gh | Platform op |
| `clarify` | Ask questions on the issue | Read, Grep, gh | Read-only |
| `research` | Gather information | Read, WebSearch, WebFetch | None |
| `comment` | Post findings on the issue | gh | Platform op |
| `fix_ci` | Fix CI failures | Read, Edit, Bash | Worktree |
| `rebase` | Rebase on main, resolve conflicts | git | Worktree |
| `address_review` | Respond to PR review comments | Read, Edit, Bash | Worktree |
| `merge` | Merge the PR | gh | Platform op |

## Deployment modes

- **Laptop**: `claude-runner issue` for single issues, `claude-runner watch` for continuous
- **CI/cron**: `claude-runner run` on a schedule
- **VM/server**: `claude-runner watch` with multiple repos
- **Per-type**: Run separate instances for bugs vs features vs research

GitHub is the coordination layer — branches are workspaces, PRs are deliverables, labels are state machines. No database needed.

## Security

See the [security hardening issue](https://github.com/joshrotenberg/claude-wrapper/issues/474) for the full threat model. Key defaults:

- Only process issues filed by the authenticated `gh` user
- `bypassPermissions` for headless execution (scoped per stage)
- Branch protection prevents force pushes
- PRs require human approval before merge (level 2)
