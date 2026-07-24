# cr

A config-driven CLI over [`claude-wrapper`](https://crates.io/crates/claude-wrapper):
a saved `claude -p` you can re-run. Name a bundle of flags (and optionally a
prompt) as a profile, then repeat it with a single word. One concept, the
profile, carries the whole surface.

The crate publishes as `claude-cr`; it installs a binary named `cr`.

## Install

```bash
cargo install claude-cr
```

Requires the [Claude Code CLI](https://docs.claude.com/en/docs/claude-code) on
`PATH`. `cr` shells out to it.

## Quickstart

```bash
# One-off: cr forwards a prompt to `claude -p` and prints the answer.
cr "explain this error" 

# Pick a model / effort inline.
cr -m opus --effort high "review this design"

# Pipe a prompt in.
git diff | cr "write a commit message for this diff"

# See the exact `claude` command cr would run, without spawning it.
cr --explain "summarize this"
```

Every run ends with a footer on stderr: the model that actually billed, turns,
cost, and wall time. `-q/--quiet` drops it.

## Config and precedence

`cr` reads two TOML files, low to high:

1. user: `~/.config/cr/config.toml`
2. project: `./cr.toml`

Within that, each option resolves lowest to highest as:

```
defaults  <  profile  <  CR_<KEY> env var  <  CLI flag
```

An explicit flag always wins. `cr config` prints the two paths; `cr config
--edit` opens the project file in `$EDITOR`.

```toml
# cr.toml
default_profile = "cheap"

[defaults]
effort = "medium"

[profiles.cheap]
model = "haiku"
effort = "low"
```

Every profile-able option has a `CR_<KEY>` environment mirror, so CI and
scripts can override a config value without editing files:

| Env var                    | Sets                    |
| -------------------------- | ----------------------- |
| `CR_MODEL`                 | model                   |
| `CR_EFFORT`                | effort                  |
| `CR_HERMETIC`              | seal ambient config     |
| `CR_WORKTREE`              | run in a git worktree   |
| `CR_AGENT`                 | pin a subagent          |
| `CR_APPEND_SYSTEM_PROMPT`  | append to system prompt |
| `CR_MAX_BUDGET_USD`        | per-run budget ceiling  |
| `CR_ALLOWED_TOOLS`         | allowed tool patterns   |

`CR_PROFILE` is separate: it selects which profile is active (below an explicit
`--profile`, above the config's `default_profile`).

## Profiles

A profile is a named bundle of settings. Select one with `--profile NAME`, or
set `default_profile` to apply it automatically. `cr profiles` lists them and
marks the default.

```bash
cr --profile cheap "quick question"
cr profiles
```

`--no-profile` ignores the auto-applied default for one run.

### Alias profiles

A profile that carries a `prompt` template is an *alias*: invoke it by name and
pass template arguments positionally.

```toml
[profiles.review]
model = "opus"
effort = "high"
prompt = "Review {{args}} for bugs and style."
```

```bash
cr review src/main.rs        # -> "Review src/main.rs for bugs and style."
```

Template substitution:

- `{{args}}` all positional arguments, space-joined
- `{{1}}`, `{{2}}`, ... the Nth argument
- `{{stdin}}` piped stdin (read only when referenced)

A template with no placeholder appends the arguments after a blank line, so
`[profiles.note] prompt = "Summarize the following."` plus `cr note foo bar`
sends the prompt followed by `foo bar`.

Profiles without a `prompt` stay flag-bundles, selected with `--profile`; only
alias profiles dispatch positionally.

### Creating profiles by use

`--save NAME` captures the resolved settings of a run into the project
`cr.toml`, then exits without spawning. A supplied prompt (positional or `-f`,
never stdin) is saved as the alias `prompt`:

```bash
# Save a flag bundle.
cr -m haiku --effort low --save cheap

# Save an alias in one shot.
cr "Review {{args}} for bugs and style." -m opus --save review
```

## Composing the prompt

The prompt comes from exactly one source, in this order: `-e`, `-f FILE`, a
positional argument, an alias template, or stdin.

- `-e/--editor` opens `$VISUAL`/`$EDITOR` (fallback `vi`) on a `.md` scratch
  file; on save the trimmed buffer is the prompt.
- `-f/--file PATH` reads the prompt from a file.

## Output

- Prose by default. On a TTY the answer streams live; on a pipe it is buffered.
  Force with `--stream` / `--no-stream`.
- `--json` prints the full structured result envelope.
- `--schema FILE` constrains the answer to a JSON Schema (implies `--json`).

## Sessions and isolation

- `--continue` resumes the most recent session in the directory; `--resume ID`
  resumes a specific one; `--session-id UUID` mints a new session with an id you
  choose (for scripted multi-turn).
- `-C/--cwd PATH` runs as if from another directory.
- `--worktree` / `--worktree-name NAME` runs in a fresh git worktree.
- `--hermetic` seals ambient `~/.claude` config for a reproducible run.

## Meta

- `--explain` prints the exact `claude` command and exits, no spawn. Combine
  with `--stream` to see the rendered prompt in the argv.
- `--save NAME` captures a profile (see above).

Run `cr --help` for the full flag reference.
