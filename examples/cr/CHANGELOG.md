# Changelog

All notable changes to this project will be documented in this file.

## [Unreleased]

## [0.1.0](https://github.com/joshrotenberg/claude-wrapper/releases/tag/cr-v0.1.0) - 2026-07-30

### Added

- *(cr)* flag and config parity plus capability knobs ([#719](https://github.com/joshrotenberg/claude-wrapper/pull/719))
- *(cr)* env precedence, alias profiles, editor compose; ship as installable crate claude-cr ([#715](https://github.com/joshrotenberg/claude-wrapper/pull/715))

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
- `cr repl`: an interactive multi-turn session over `DuplexSession`. Assistant
  text streams live; `/model`, `/effort`, `/profile` retune and respawn with
  `--resume`; `/session new`, `/use`, `/close`, `/all` run several
  conversations at once; Ctrl-C interrupts a turn. reedline editor on a TTY,
  plain stdin for scripting.
