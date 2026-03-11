# Changelog

All notable changes to this project will be documented in this file.

## [unreleased]

## [0.3.1](https://github.com/joshrotenberg/claude-wrapper/compare/v0.3.0...v0.3.1) - 2026-03-11

### Added

- add structured failure details to TaskResult ([#155](https://github.com/joshrotenberg/claude-wrapper/pull/155)) ([#159](https://github.com/joshrotenberg/claude-wrapper/pull/159))

### Fixed

- add --verbose when using stream-json output format ([#142](https://github.com/joshrotenberg/claude-wrapper/pull/142))
- strip CLAUDECODE at startup and surface stderr in pool errors ([#138](https://github.com/joshrotenberg/claude-wrapper/pull/138))

## [0.3.0](https://github.com/joshrotenberg/claude-wrapper/compare/v0.2.1...v0.3.0) - 2026-03-11

### Added

- add Session management abstraction ([#133](https://github.com/joshrotenberg/claude-wrapper/pull/133))
- add --json support to AgentsCommand and DoctorCommand ([#129](https://github.com/joshrotenberg/claude-wrapper/pull/129))
- fix cost tracking by matching CLI's total_cost_usd field ([#106](https://github.com/joshrotenberg/claude-wrapper/pull/106))

### Other

- update READMEs for release readiness ([#135](https://github.com/joshrotenberg/claude-wrapper/pull/135))
- add claude-wrapper integration tests for streaming, timeout, and errors ([#120](https://github.com/joshrotenberg/claude-wrapper/pull/120))
- add fake-claude binary and integration test infrastructure ([#119](https://github.com/joshrotenberg/claude-wrapper/pull/119))

## [0.2.1](https://github.com/joshrotenberg/claude-wrapper/compare/v0.2.0...v0.2.1) - 2026-03-10

### Added

- add project-specific skills for claude-wrapper workspace ([#57](https://github.com/joshrotenberg/claude-wrapper/pull/57))

## [0.2.0](https://github.com/joshrotenberg/claude-wrapper/compare/v0.1.0...v0.2.0) - 2026-03-10

### Added

- add claude-pool worker pool and MCP server ([#25](https://github.com/joshrotenberg/claude-wrapper/pull/25))

## [0.1.0](https://github.com/joshrotenberg/claude-wrapper/releases/tag/v0.1.0) - 2026-03-09

### Added

- initial implementation of claude-wrapper ([#1](https://github.com/joshrotenberg/claude-wrapper/pull/1))

### Other

- initial commit
