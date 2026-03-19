# Prompt Guide

Practical checklist for writing effective task prompts with `claudes`. Based on lessons from
real usage. For general prompting best practices, see
[Anthropic's prompting documentation](https://docs.anthropic.com/en/docs/build-with-claude/prompt-engineering/overview).

---

## 1. Prompt Checklist

Every task prompt should include:

- **File restrictions** — List every file the model is allowed to modify. "Only modify `src/foo.rs`
  and `tests/foo_test.rs`. Do NOT touch any other files."
- **What NOT to touch** — Call out files that are tempting but off-limits. "Do NOT modify
  `Cargo.toml`, `.gitignore`, or any file not listed above."
- **Commit message format** — Specify exactly. "Commit with message `fix(auth): correct token
  expiry check`."
- **PR creation instructions** — Include the full `gh pr create` invocation with a required body
  format. Minimal descriptions are a common failure mode; tell the model what sections to include
  (Summary, files changed, Test plan).
- **Verification steps** — List the exact commands to run before the task is done. "Run
  `cargo fmt --check` and `cargo test --lib` and confirm they pass."
- **Rust 2024 if-let chains** — For Rust projects, include in the system prompt: "Use Rust 2024
  if-let chains: write `if let Some(x) = y && condition {` instead of nested if-let/if blocks."
  This prevents `collapsible_if` clippy failures that post-hooks will catch.
- **`finally_hooks` for cleanup** — Include `finally_hooks` for cleanup that must run regardless
  of whether the task succeeds or fails (e.g., removing temp files, resetting state).

---

## 2. Manifest Best Practices

### Name tasks explicitly

Auto-generated names are opaque in logs and worktree paths. Use descriptive names.

```json
{ "name": "fix-token-expiry" }
```

not

```json
{ "name": "task-1" }
```

### Use `disallowed_tools` for edit-only tasks

If the task should only edit existing files, block `Write` to prevent the model from creating new
files.

```json
{ "disallowed_tools": ["Write"] }
```

### Use `post_hooks` for deterministic validation

Models sometimes claim they ran checks but didn't. `post_hooks` run after the task completes and
will fail the run if they exit non-zero — no self-reporting required.

```json
{ "post_hooks": ["cargo fmt --check", "cargo test --lib --all-features"] }
```

### Use `append_system_prompt` for project-wide context

Inject standing rules once rather than repeating them in every prompt.

```json
{ "append_system_prompt": "This is a Rust 2024 project. Do NOT modify any file not explicitly named in the task prompt." }
```

### Scope `allowed_tools` to what the task needs

Start narrow. A code-editing task rarely needs every tool.

```json
{ "allowed_tools": ["Read", "Edit", "Bash(cargo *)", "Bash(git *)"] }
```

---

## 3. Common Mistakes

- **Model claims it ran tests but didn't.** Use `post_hooks` to enforce verification. Never rely on
  the model's self-report.

- **Model edits unrelated files.** Combine explicit file restrictions in the prompt with
  `disallowed_tools: ["Write"]`. Two signals are more reliable than one.

- **Minimal PR descriptions.** "Closes #N" is not enough. Explicitly instruct the model to include a
  Summary section, a list of modified files, and a Test plan. Put the expected PR body format
  directly in the prompt.

- **Overly broad tool access.** Omitting `allowed_tools` gives the model access to everything.
  Scope it to what the task actually needs — this also catches mistakes earlier (wrong tool use
  fails fast instead of silently succeeding in unexpected ways).

- **Vague commit messages.** "Update code" gets accepted. Specify the exact conventional-commit
  message in the prompt.

---

## 4. Parallel vs Sequential

Not all tasks can safely run in parallel. The key distinction is whether tasks touch the same
logical unit.

**Additive changes are usually safe to parallelize.** Adding a new function, a new struct field,
or a new test to a file does not conflict with another task doing the same thing in a different
location. Merge is mechanical.

**Structural changes to the same function must be sequenced.** If two tasks both modify how a
function branches — different control flow, different return types, different error handling — they
will produce conflicting diffs on the same lines. No amount of tooling resolves this cleanly. One
task must finish before the other starts.

**This is a planning problem, not a tooling problem.** It applies to any parallel work system, not
just `claudes`. When scheduling tasks, ask: do any of these tasks modify the same function body?
If yes, sequence them.

Example of a bad parallel split: "Refactor `parse()` to return `Result`" and "Add early-return to
`parse()` when input is empty" — both rewrite the same function's control flow and will conflict.
Run one, then the other.

---

## 5. Common Post-Hook Failures

`post_hooks` catch verification failures that the model's self-report misses. Common failures:

- **`collapsible_if` clippy lint** — The model wrote nested `if let` / `if` blocks instead of a
  Rust 2024 if-let chain. Fix: add the if-let chain instruction to the system prompt (see Prompt
  Checklist above). Or run `claudes fix` to retry the failing task.

- **`cargo fmt` failures** — The model usually ran `cargo fmt` but missed a file, or a generated
  file was not formatted. Fix: identify the unformatted file from the hook output and run
  `cargo fmt` on it, or let `claudes fix` handle it.

These failures surface as non-zero exit codes from `post_hooks`. The run is marked failed and the
worktree is preserved for inspection.

---

## 6. Annotated Example

```json
{
  "version": 1,

  "shared": {
    "append_system_prompt": "This is a Rust 2024 project. Use if-let chains: write `if let Some(x) = y && condition {` instead of nested if-let/if blocks. Do NOT modify any file not explicitly named in the task prompt.",
    "allowed_tools": [
      "Read",
      "Edit",
      "Bash(cargo *)",
      "Bash(git *)",
      "Bash(gh pr *)"
    ],
    "disallowed_tools": ["Write"],
    "post_hooks": [
      "cargo fmt --check",
      "cargo clippy --all-targets -- -D warnings",
      "cargo test --lib"
    ]
  },

  "tasks": [
    {
      "name": "fix-token-expiry",

      "prompt": "Fix the off-by-one error in token expiry in `src/auth/token.rs`.\n\nOnly modify `src/auth/token.rs` and `tests/auth/token_test.rs`. Do NOT touch any other file.\n\nCommit with message `fix(auth): correct token expiry check`.\n\nThen run:\n  gh pr create --title 'fix(auth): correct token expiry check' --body '## Summary\n- Fixed off-by-one in token expiry\n- Modified: src/auth/token.rs, tests/auth/token_test.rs\n\n## Test plan\n- [ ] cargo test --lib passes\n- [ ] cargo clippy clean'\n\nDo NOT create a PR until all verification steps pass."
    }
  ]
}
```

Key points in this example:

- `shared` block holds config that applies to every task — avoids repeating it per task
- `shared.append_system_prompt` includes the Rust 2024 if-let chain instruction
- `shared.post_hooks` enforce verification without relying on the model's self-report
- `name` is descriptive, not auto-generated
- Prompt names every file that may be touched
- Prompt specifies the exact commit message and PR body format
- `disallowed_tools` blocks `Write` to prevent creating files
