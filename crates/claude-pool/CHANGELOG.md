# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.4.0](https://github.com/joshrotenberg/claude-wrapper/compare/claude-pool-v0.3.0...claude-pool-v0.4.0) - 2026-03-12

### Added

- task execution metrics, session aggregation, and REST/MCP endpoints ([#216](https://github.com/joshrotenberg/claude-wrapper/pull/216))
- SSE streaming endpoints for REST API (Phase 2) ([#213](https://github.com/joshrotenberg/claude-wrapper/pull/213))

### Other

- TaskOverrides + RunOptions builder ([#209](https://github.com/joshrotenberg/claude-wrapper/pull/209))
- organize lib.rs re-exports with prelude module ([#208](https://github.com/joshrotenberg/claude-wrapper/pull/208))
- centralize ID generation ([#207](https://github.com/joshrotenberg/claude-wrapper/pull/207))
- add comprehensive rustdoc to pool server tools ([#205](https://github.com/joshrotenberg/claude-wrapper/pull/205))

## [0.3.0](https://github.com/joshrotenberg/claude-wrapper/compare/claude-pool-v0.2.0...claude-pool-v0.3.0) - 2026-03-11

### Added

- add quality gate hooks for task lifecycle ([#183](https://github.com/joshrotenberg/claude-wrapper/pull/183))
- add chain workflow and triage skills for claude-pool ([#182](https://github.com/joshrotenberg/claude-wrapper/pull/182))
- add ${CLAUDE_SKILL_DIR} substitution and skill directory docs ([#181](https://github.com/joshrotenberg/claude-wrapper/pull/181))
- align skills with Agent Skills standard ([#162](https://github.com/joshrotenberg/claude-wrapper/pull/162)) ([#179](https://github.com/joshrotenberg/claude-wrapper/pull/179))
- add auto-delivery messaging and self-claiming task queue (#169, #170) ([#175](https://github.com/joshrotenberg/claude-wrapper/pull/175))
- add pool_dashboard and chain_watcher loop monitoring skills ([#174](https://github.com/joshrotenberg/claude-wrapper/pull/174))
- session fix, plan-then-execute skill, $ARGUMENTS substitution (#161, #167, #162) ([#173](https://github.com/joshrotenberg/claude-wrapper/pull/173))
- add broadcast messaging and slot discovery (#165, #166) ([#172](https://github.com/joshrotenberg/claude-wrapper/pull/172))
- add structured failure details to TaskResult ([#155](https://github.com/joshrotenberg/claude-wrapper/pull/155)) ([#159](https://github.com/joshrotenberg/claude-wrapper/pull/159))
- adopt SKILL.md format and add global skills directory ([#157](https://github.com/joshrotenberg/claude-wrapper/pull/157))
- implement inter-slot messaging for claude-pool ([#153](https://github.com/joshrotenberg/claude-wrapper/pull/153))
- add server metadata to pool_status and clone isolation mode ([#151](https://github.com/joshrotenberg/claude-wrapper/pull/151))

### Fixed

- rewrite coordinator skill prompts for haiku compatibility ([#188](https://github.com/joshrotenberg/claude-wrapper/pull/188))
- preserve GitHub remote URL in clone isolation ([#154](https://github.com/joshrotenberg/claude-wrapper/pull/154))

### Other

- fix quality gates and skills examples in claude-pool README ([#186](https://github.com/joshrotenberg/claude-wrapper/pull/186))
- move builtin skills to SKILL.md files ([#178](https://github.com/joshrotenberg/claude-wrapper/pull/178)) ([#180](https://github.com/joshrotenberg/claude-wrapper/pull/180))

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
