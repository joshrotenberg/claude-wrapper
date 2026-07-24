# Changelog

All notable changes to this project will be documented in this file.

## [Unreleased]

## [0.1.0](https://github.com/joshrotenberg/claude-wrapper/releases/tag/cr-v0.1.0) - 2026-07-24

### Added

- *(cr)* env precedence, alias profiles, editor compose; ship as installable crate claude-cr ([#715](https://github.com/joshrotenberg/claude-wrapper/pull/715))

### Features

- Config-driven CLI over claude-wrapper: TOML profiles with
  `defaults < profile < CR_<KEY> env < CLI flag` layering, alias profiles with
  `{{args}}`/`{{N}}`/`{{stdin}}` prompt templates, `-e` editor compose,
  `--explain` dry-run, `--save` creation-by-use, and a cost/turns footer.
