# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.3.0](https://github.com/joshrotenberg/claude-wrapper/compare/claude-pool-server-v0.2.0...claude-pool-server-v0.3.0) - 2026-03-11

### Added

- add quality gate hooks for task lifecycle ([#183](https://github.com/joshrotenberg/claude-wrapper/pull/183))
- add ${CLAUDE_SKILL_DIR} substitution and skill directory docs ([#181](https://github.com/joshrotenberg/claude-wrapper/pull/181))
- align skills with Agent Skills standard ([#162](https://github.com/joshrotenberg/claude-wrapper/pull/162)) ([#179](https://github.com/joshrotenberg/claude-wrapper/pull/179))
- add auto-delivery messaging and self-claiming task queue (#169, #170) ([#175](https://github.com/joshrotenberg/claude-wrapper/pull/175))
- add broadcast messaging and slot discovery (#165, #166) ([#172](https://github.com/joshrotenberg/claude-wrapper/pull/172))
- adopt SKILL.md format and add global skills directory ([#157](https://github.com/joshrotenberg/claude-wrapper/pull/157))
- implement inter-slot messaging for claude-pool ([#153](https://github.com/joshrotenberg/claude-wrapper/pull/153))
- add server metadata to pool_status and clone isolation mode ([#151](https://github.com/joshrotenberg/claude-wrapper/pull/151))

### Fixed

- strip CLAUDECODE at startup and surface stderr in pool errors ([#138](https://github.com/joshrotenberg/claude-wrapper/pull/138))

### Other

- establish consistent user-facing vocabulary for pool operations ([#158](https://github.com/joshrotenberg/claude-wrapper/pull/158))

## [0.2.0](https://github.com/joshrotenberg/claude-wrapper/compare/claude-pool-server-v0.1.0...claude-pool-server-v0.2.0) - 2026-03-11

### Added

- add Session management abstraction ([#133](https://github.com/joshrotenberg/claude-wrapper/pull/133))
- default chain isolation to worktree and add rebase skill ([#114](https://github.com/joshrotenberg/claude-wrapper/pull/114))
- structured inter-step context for chains ([#112](https://github.com/joshrotenberg/claude-wrapper/pull/112))
- add HTTP transport for claude-pool-server ([#111](https://github.com/joshrotenberg/claude-wrapper/pull/111))
- per-chain worktree isolation opt-in ([#104](https://github.com/joshrotenberg/claude-wrapper/pull/104)) ([#109](https://github.com/joshrotenberg/claude-wrapper/pull/109))
- add chain cancellation (pool_cancel_chain) ([#107](https://github.com/joshrotenberg/claude-wrapper/pull/107))
- pass MCP config to pool slots ([#100](https://github.com/joshrotenberg/claude-wrapper/pull/100))
- add skill management tools (list/get/add/remove/save) ([#98](https://github.com/joshrotenberg/claude-wrapper/pull/98))
- [**breaking**] add skill scopes and extract project-specific skills ([#93](https://github.com/joshrotenberg/claude-wrapper/pull/93))
- load project-local skills from .claude-pool/skills/ directory ([#87](https://github.com/joshrotenberg/claude-wrapper/pull/87))
- add --min-slots/--max-slots CLI flags and document dynamic scaling ([#91](https://github.com/joshrotenberg/claude-wrapper/pull/91))

### Other

- update READMEs for release readiness ([#135](https://github.com/joshrotenberg/claude-wrapper/pull/135))
- add claude-pool-server tool handler tests ([#126](https://github.com/joshrotenberg/claude-wrapper/pull/126))
- make tool surface and server instructions workflow-agnostic ([#96](https://github.com/joshrotenberg/claude-wrapper/pull/96))
- add model selection heuristics to server instructions ([#89](https://github.com/joshrotenberg/claude-wrapper/pull/89))
- add installation and deployment guidance ([#84](https://github.com/joshrotenberg/claude-wrapper/pull/84))

## [0.1.0](https://github.com/joshrotenberg/claude-wrapper/releases/tag/claude-pool-server-v0.1.0) - 2026-03-10

### Added

- add claude-pool slot pool and MCP server ([#25](https://github.com/joshrotenberg/claude-wrapper/pull/25))
