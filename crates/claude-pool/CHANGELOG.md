# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.2.0](https://github.com/joshrotenberg/claude-wrapper/compare/claude-pool-v0.1.0...claude-pool-v0.2.0) - 2026-03-11

### Added

- add Session management abstraction ([#133](https://github.com/joshrotenberg/claude-wrapper/pull/133))
- add --json support to AgentsCommand and DoctorCommand ([#129](https://github.com/joshrotenberg/claude-wrapper/pull/129))
- default chain isolation to worktree and add rebase skill ([#114](https://github.com/joshrotenberg/claude-wrapper/pull/114))
- structured inter-step context for chains ([#112](https://github.com/joshrotenberg/claude-wrapper/pull/112))
- per-chain worktree isolation opt-in ([#104](https://github.com/joshrotenberg/claude-wrapper/pull/104)) ([#109](https://github.com/joshrotenberg/claude-wrapper/pull/109))
- live output for running chain steps ([#108](https://github.com/joshrotenberg/claude-wrapper/pull/108))
- add chain cancellation (pool_cancel_chain) ([#107](https://github.com/joshrotenberg/claude-wrapper/pull/107))
- fix cost tracking by matching CLI's total_cost_usd field ([#106](https://github.com/joshrotenberg/claude-wrapper/pull/106))
- pass MCP config to pool slots ([#100](https://github.com/joshrotenberg/claude-wrapper/pull/100))
- add supervisor loop for slot health monitoring ([#97](https://github.com/joshrotenberg/claude-wrapper/pull/97))
- add skill management tools (list/get/add/remove/save) ([#98](https://github.com/joshrotenberg/claude-wrapper/pull/98))
- [**breaking**] add skill scopes and extract project-specific skills ([#93](https://github.com/joshrotenberg/claude-wrapper/pull/93))
- load project-local skills from .claude-pool/skills/ directory ([#87](https://github.com/joshrotenberg/claude-wrapper/pull/87))
- add built-in create_pr skill ([#90](https://github.com/joshrotenberg/claude-wrapper/pull/90))
- detect permission prompts in pool slot stderr ([#88](https://github.com/joshrotenberg/claude-wrapper/pull/88))

### Other

- update READMEs for release readiness ([#135](https://github.com/joshrotenberg/claude-wrapper/pull/135))
- add claude-pool integration tests for pool lifecycle, chains, and supervisor ([#127](https://github.com/joshrotenberg/claude-wrapper/pull/127))
- add fake-claude binary and integration test infrastructure ([#119](https://github.com/joshrotenberg/claude-wrapper/pull/119))
- make tool surface and server instructions workflow-agnostic ([#96](https://github.com/joshrotenberg/claude-wrapper/pull/96))

## [0.1.0](https://github.com/joshrotenberg/claude-wrapper/releases/tag/claude-pool-v0.1.0) - 2026-03-10

### Added

- add claude-pool slot pool and MCP server ([#25](https://github.com/joshrotenberg/claude-wrapper/pull/25))
