# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.1.1](https://github.com/joshrotenberg/claude-wrapper/compare/claude-pool-mcp-v0.1.0...claude-pool-mcp-v0.1.1) - 2026-03-16

### Other

- update Cargo.lock dependencies

## [0.1.0](https://github.com/joshrotenberg/claude-wrapper/releases/tag/claude-pool-mcp-v0.1.0) - 2026-03-16

### Added

- ship pool coordinator skill and remove old skill infrastructure ([#299](https://github.com/joshrotenberg/claude-wrapper/pull/299))
- add claude-pool-mcp crate (tower-mcp based pool server) ([#282](https://github.com/joshrotenberg/claude-wrapper/pull/282))

### Other

- update READMEs, .mcp.json, and workspace for release prep ([#301](https://github.com/joshrotenberg/claude-wrapper/pull/301))
- [**breaking**] remove dead code — skills, workflows, pool-server, claudes ([#283](https://github.com/joshrotenberg/claude-wrapper/pull/283))
