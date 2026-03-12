---
name: skill-audit
description: >-
  Audit skills for size, staleness, frontmatter completeness, and overlap.
  Runs against all known skills or a single skill by name.
argument-hint: "[skill_name]"
metadata:
  scope: coordinator
  arguments:
    - name: skill_name
      description: "Audit a single skill by name (default: audit all)"
      required: false
---

# Skill Audit

Audit skills for health issues. Reports findings — never auto-deletes or modifies skills.

## Skill Discovery

Scan these directories for `*/SKILL.md` files:
1. `.claude-pool/skills/` (project skills, highest priority)
2. `~/.claude-pool/skills/` (global user skills)

If `skill_name` is provided, find only that skill. Otherwise audit all.

## Heuristics

For each skill, check:

### 1. Size
- Lines > 200: flag "consider trimming"
- Lines > 300: flag "strongly recommend trimming — context cost is high"

### 2. Frontmatter completeness
Required fields: `name`, `description`, `metadata.scope`. Flag any that are missing.

### 3. Scope validity
`metadata.scope` should be one of: `coordinator`, `worker`, `shared`. Flag unknown values.

### 4. Overlap detection
For each pair of skills, compute keyword overlap:
- Extract significant words (skip stopwords, YAML keys, markdown syntax) from the body.
- If two skills share > 50% of significant words, flag as potential duplicates.
- Skip this check when auditing a single skill.

### 5. Staleness
Run `git log -1 --format=%ci -- {skill_path}` for each skill.
- Last modified > 30 days ago: flag "review for staleness"
- Not tracked by git: note "untracked — consider committing or removing"

## Output

Report as a table per skill:

```
| Skill | Lines | Frontmatter | Scope | Stale | Issues |
|-------|-------|-------------|-------|-------|--------|
| {name} | {n} | {ok/missing: X} | {scope} | {date} | {list} |
```

If overlap detected, add a section:

```
Potential overlaps:
- {skill_a} <-> {skill_b}: {overlap_pct}% keyword overlap
```

End with a summary: `{total} skills audited, {issues} issues found, {clean} clean.`
