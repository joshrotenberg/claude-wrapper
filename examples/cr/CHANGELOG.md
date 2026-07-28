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
- Background jobs: `cr -d "<prompt>"` launches a detached `claude` run (no
  daemon; own process group, stdout journaled under `~/.config/cr/jobs/`) and
  returns. `cr jobs` lists them (including Claude Code's own daemon jobs,
  read-only); `cr job <id>` renders/tails one (`--follow`, `--json`); the REPL
  `<prompt> &` backgrounds a turn; reconnect with `cr repl --resume <id>`.
  Launching requires a budget or turn cap (unattended tool access) unless
  `--uncapped`.
- `cr repl`: an interactive multi-turn session over `DuplexSession`. Assistant
  text streams live; `/model`, `/effort`, `/profile` retune and respawn with
  `--resume`; `/session new`, `/use`, `/close`, `/all` run several
  conversations at once; Ctrl-C interrupts a turn. reedline editor on a TTY,
  plain stdin for scripting. Tab-completes commands and profile names, suggests
  the nearest command on a typo, `/json` prints the last turn's full result, and
  `-e/--exec` runs commands then exits.
