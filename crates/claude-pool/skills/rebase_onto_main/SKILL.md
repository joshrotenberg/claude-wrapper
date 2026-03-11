---
name: rebase_onto_main
description: "Rebase current branch onto latest main, resolving conflicts."
metadata:
  scope: task
---

Rebase the current branch onto the latest `origin/main`.

## Steps

1. **Fetch latest**: Run `git fetch origin` to get the latest remote state.

2. **Rebase**: Run `git rebase origin/main`.

3. **Handle conflicts**: If the rebase produces conflicts:
   - For each conflicted file, examine the conflict markers and resolve them sensibly.
   - Prefer the current branch's intent while incorporating upstream changes.
   - After resolving each file, `git add` it and `git rebase --continue`.
   - If a conflict is ambiguous or risky, abort with `git rebase --abort` and report what happened.

4. **Verify**: Run `cargo check` to confirm the rebased code compiles. If it fails, diagnose and fix the issue, then amend the relevant commit.

5. **Report**: Summarize what happened:
   - How many commits were rebased
   - Whether any conflicts were encountered and how they were resolved
   - Whether `cargo check` passed on the first try or required fixes
