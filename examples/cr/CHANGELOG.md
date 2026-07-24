# Changelog

All notable changes to this project will be documented in this file.

## [Unreleased]

### Features

- Config-driven CLI over claude-wrapper: TOML profiles with
  `defaults < profile < CR_<KEY> env < CLI flag` layering, alias profiles with
  `{{args}}`/`{{N}}`/`{{stdin}}` prompt templates, `-e` editor compose,
  `--explain` dry-run, `--save` creation-by-use, and a cost/turns footer.
