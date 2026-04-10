# claudes

> **⚠ DEPRECATED — unmaintained and not published.**
>
> `claudes` is kept in the repo for git history but is `publish = false`, excluded from the workspace build, and will not receive updates. This tool was an experiment in manifest-driven parallel execution; similar functionality is being pursued in a separate project. The maintained crate in this repo is [`claude-wrapper`](../claude-wrapper/).
>
> To revive this crate, move it from `[workspace] exclude` back into `[workspace] members` in the root `Cargo.toml`.

---

Manifest-driven execution engine for headless Claude Code sessions.

Write a manifest describing your tasks. claudes runs them in parallel, each in its own git
worktree, with streaming output and post-hook validation.

## Install

```bash
cargo install --path crates/claudes
```

Requires the `claude` CLI in your PATH.

## Usage

### Quick start

```bash
# Run tasks from a prompt
claudes run -p "fix the pagination bug" -p "add unit tests"

# Use AI to generate a manifest
claudes generate -p "work on issues 1, 2, 3 as separate tasks"

# Generate a manifest template
claudes init --tasks 3 --model sonnet -o plan.json

# Run from a manifest file
claudes run --manifest plan.json -v

# Check results
claudes status
claudes metrics
```

### Manifests

A manifest is a JSON or TOML document describing what to execute:

```json
{
  "version": 1,
  "shared": {
    "model": "sonnet",
    "max_turns": 30,
    "permission_mode": "bypassPermissions",
    "post_hooks": ["cargo fmt --check", "cargo test --lib"]
  },
  "tasks": [
    {
      "name": "fix-pagination",
      "prompt": "Fix the pagination bug in src/api/list.rs",
      "isolation": { "type": "worktree", "base_dir": ".worktrees" }
    },
    {
      "name": "add-tests",
      "prompt": "Add unit tests for the auth module"
    }
  ]
}
```

Or in TOML:

```toml
[shared]
model = "sonnet"
max_turns = 30
post_hooks = ["cargo fmt --check", "cargo test --lib"]

[[tasks]]
name = "fix-pagination"
prompt = "Fix the pagination bug in src/api/list.rs"

[[tasks]]
name = "add-tests"
prompt = "Add unit tests for the auth module"
```

### Subcommands

| Command | Description |
|---|---|
| `run` | Execute tasks from a manifest, config, or CLI args |
| `plan` | Generate a manifest without executing |
| `init` | Generate a manifest template with stub tasks |
| `generate` | AI-assisted manifest creation using Claude |
| `status` | Show results of the most recent run |
| `fix` | Re-run failed/timed-out tasks with error context |
| `metrics` | Aggregate stats across run history |
| `clean` | Remove worktrees, run state, and merged branches |

### Features

- **Parallel execution** in isolated git worktrees
- **Shared blocks** for manifest-level defaults
- **Named profiles** for reusable configuration presets
- **Pre/post/finally hooks** for setup, validation, and cleanup
- **Streaming output** with per-task colors and verbosity levels (`-v`, `-vv`)
- **State persistence** with timestamped run IDs
- **Cost tracking** aggregated from stream events
- **Auto-discovery** of `claudes.toml` in the project root
- **TOML and JSON** manifest formats
- **`claudes fix`** to re-run failed tasks with error context
- **`claudes generate`** for AI-assisted manifest creation

### Auto-discovery

When running without `--manifest`, claudes searches for:

1. `claudes.toml` or `.claudes.toml` in the current directory
2. `claudes.json` or `.claudes.json` in the current directory

### Global defaults

Create `~/.config/claudes/defaults.toml` with shared defaults that apply to all projects:

```toml
[shared]
model = "sonnet"
post_hooks = ["cargo check --quiet"]
```

## Prompt guide

See [PROMPTING.md](PROMPTING.md) for best practices on writing task prompts,
parallel vs sequential task planning, and handling merge conflicts.

## Examples

- `examples/plan_and_run.rs` — plan -> review -> execute workflow
- `examples/manifest_file.rs` — load and run from JSON file
- `examples/programmatic.rs` — build tasks as structs
- `examples/manifests/` — non-software-development example manifests
