# Changelog

All notable changes to this project will be documented in this file.

## [Unreleased]

### Features

- Config-driven CLI over claude-wrapper: TOML profiles with
  `defaults < profile < CR_<KEY> env < CLI flag` layering, alias profiles with
  `{{args}}`/`{{N}}`/`{{stdin}}` prompt templates, `-e` editor compose,
  `--explain` dry-run, `--save` creation-by-use, and a cost/turns footer.
- Flag parity for every profile-able option, so an explicit flag always wins:
  `--agent`, `--append-system-prompt`, `--max-budget-usd`, `--allow-tool`.
- Tools, permissions, and limits as profile-able keys (flag + `CR_*` mirror):
  `--permission-mode` (with `--accept-edits`/`--plan` shortcuts),
  `--disallow-tool`, `--add-dir`, `--mcp-config`, `--fallback-model`,
  `--max-turns`.
- `cr profiles NAME` prints what a profile resolves to (defaults plus profile).
