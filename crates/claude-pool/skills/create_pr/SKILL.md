---
name: create_pr
description: Create a pull request for the current branch.
argument-hint: "<title> <body> [issue]"
metadata:
  arguments:
    - name: title
      description: PR title (short, under 70 characters).
      required: true
    - name: body
      description: PR description/body.
      required: true
    - name: issue
      description: Issue number to close (e.g. 42). Omit if none.
      required: false
---

Create a pull request using `gh pr create`.

Title: {title}

Body:
{body}

If an issue number is provided, append "Closes #{issue}" to the body.
Issue: {issue}

Steps:
1. Check if the current branch has an upstream. If not, push with `git push -u origin HEAD`.
2. Create the PR with `gh pr create --title "..." --body "..."`.
3. Leave the PR open for the user to merge.
4. Omit Co-Authored-By and "Generated with Claude Code" signatures (per project convention).
5. Report the PR URL when done.
