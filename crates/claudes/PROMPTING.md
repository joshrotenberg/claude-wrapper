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

## 4. Annotated Example

```json
{
  "version": 1,
  "tasks": [
    {
      "name": "fix-token-expiry",

      "prompt": "Fix the off-by-one error in token expiry in `src/auth/token.rs`.\n\nOnly modify `src/auth/token.rs` and `tests/auth/token_test.rs`. Do NOT touch any other file.\n\nCommit with message `fix(auth): correct token expiry check`.\n\nThen run:\n  gh pr create --title 'fix(auth): correct token expiry check' --body '## Summary\n- Fixed off-by-one in token expiry\n- Modified: src/auth/token.rs, tests/auth/token_test.rs\n\n## Test plan\n- [ ] cargo test --lib passes\n- [ ] cargo clippy clean'\n\nDo NOT create a PR until all verification steps pass.",

      "allowed_tools": [
        "Read",
        "Edit",
        "Bash(cargo *)",
        "Bash(git *)",
        "Bash(gh pr *)"
      ],

      "disallowed_tools": ["Write"],

      "append_system_prompt": "Do NOT modify any file not explicitly named in the task prompt.",

      "post_hooks": [
        "cargo fmt --check",
        "cargo clippy --all-targets -- -D warnings",
        "cargo test --lib"
      ]
    }
  ]
}
```

Key points in this example:

- `name` is descriptive, not auto-generated
- Prompt names every file that may be touched
- Prompt specifies the exact commit message and PR body format
- `allowed_tools` is scoped to read, edit, cargo, git, and gh — nothing else
- `disallowed_tools` blocks `Write` to prevent creating files
- `append_system_prompt` reinforces the file restriction at the system level
- `post_hooks` enforce verification without relying on the model's self-report
