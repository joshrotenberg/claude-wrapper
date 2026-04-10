# Changelog

All notable changes to this project will be documented in this file.

## [0.5.0] - 2026-04-10

### Bug Fixes

- Changelog workflow creates PR instead of direct push 
- *(claudes)* Bright color palette and relative path display 
- *(claudes)* Strip worktree prefix from tool call paths 
- *(claudes)* Truncate errors to first line in status table 
- *(claudes)* Default to bypassPermissions for headless MCP runs 
- *(claude-wrapper)* Add -- separator before prompt in query args 
- *(claudes)* Make created_at and version optional in manifest deserialization  
- *(claudes)* Suppress overlap warnings for sequenced tasks  
- *(claudes)* Breadcrumb write permissions and per-run scoping (#432, #433) 
- *(ci)* Add checks:write permission for security audit 
- *(claudes)* Add CLI reference to interactive prompt  
- *(claudes)* Desugar chains before checking file overlaps 
- *(claudes)* Reuse worktree for chained tasks sharing a branch 
- *(claudes)* Auto-inherit branch from dependency in chains 
- *(claudes)* Truncate multi-line tool args in progress display 
- *(claude-runner)* Improve branch name sanitization 
- Add claude-runner to workspace members 
- *(claude-runner)* Inject clarify stage dynamically instead of defaulting in feature workflow 
- *(claude-runner)* Skip optional stages when issue is well-specified 
- *(claude-runner)* Workflow selection should be label-based, not title-prefix inference 
- *(claude-runner)* Rebase on origin/main before push and use git add -u 
- *(claude-wrapper)* [**breaking**] Remove panicky Transport::from(&str), add TryFrom 
- *(claude-wrapper)* Clean child cleanup on timeout; add missing tests 

### Documentation

- *(claudes)* Add prompt guide 
- *(claudes)* Add non-dev example manifests 
- *(claudes)* Update prompting guide with dogfood lessons 
- Claudes README and PROMPTING.md updates 
- *(claudes)* Add seismic research example manifest  

### Features

- Enable worktree isolation by default for pool slots 
- Add claudes manifest-driven execution engine 
- *(claudes)* Tests, examples, state file, cleanup policy 
- *(claudes)* Stream task events during execution 
- *(claudes)* Add TaskBuilder 
- *(claudes)* Add RunOptionsBuilder and PlanOptionsBuilder 
- *(claudes)* Smarter auto-generated task names 
- *(claudes)* Run IDs and state history 
- *(claudes)* Add init subcommand for manifest templates 
- *(claudes)* Add post_hooks support 
- *(claudes)* Add shared block to manifest schema 
- *(claudes)* Add output verbosity levels 
- *(claudes)* Add TOML manifest parsing 
- *(claudes)* Add pre_hooks support 
- *(claudes)* Distinguish timeout vs failure in status 
- *(claudes)* Auto-discover project manifest files 
- *(claudes)* Per-task colored output prefixes 
- *(claudes)* Add per-task NDJSON log persistence 
- *(claudes)* Add global defaults file support 
- *(claudes)* Add prompt_file and task selection 
- *(claudes)* Add named profile support 
- *(claudes)* Aggregate cost from stream events 
- *(claudes)* Enhance clean command with --runs and --branches 
- *(claudes)* Add finally_hooks (always-run hooks) 
- *(claudes)* Warn on potential file overlaps between tasks 
- *(claudes)* Add fix subcommand for failed tasks 
- *(claudes)* Add metrics command for run history analysis 
- *(claudes)* Add generate command for AI-assisted manifest creation 
- *(claudes)* Enhance generate with project context and add task-level metrics 
- *(claudes)* Add MCP server (claudes serve) 
- *(claudes)* Enhance MCP server with titles, descriptions, and instructions 
- *(claudes)* Add background execution for run_manifest MCP tool 
- *(claudes)* Show elapsed time and consistent format in streaming output 
- *(claudes)* Add turn usage to metrics output 
- *(claudes)* Show in-progress runs in status 
- *(claudes)* Add structured tracing spans per task 
- *(claudes)* Add per-run breakdown to metrics output 
- *(claudes)* Add CLI commands to MCP tool responses 
- *(claudes)* Colored status output for task results 
- *(claudes)* Default isolation to worktree  
- *(claudes)* Three distinct output modes  
- *(claudes)* In-place indicatif spinners for progress mode 
- *(claudes)* Task dependencies (depends_on)  
- *(claudes)* Chains — manifest-level sugar for task dependencies  
- *(claudes)* Breadcrumbs for cross-task context in dependency chains  
- *(claudes)* Explicit skill injection and settings passthrough  
- *(claudes)* Interactive mode — bare `claudes` launches orchestrator  
- *(claudes)* Two-tier CLI help and workflow-ordered commands 
- *(claudes)* Hook progress display and meaningful tool args (#434, #435) 
- *(claudes)* Improved generate prompt and robust JSON extraction 
- *(claudes)* Add conditional task execution with condition field  
- Claude-runner — autonomous GitHub issue runner 
- *(claude-runner)* Lease system via GitHub labels 
- *(claude-runner)* Show cost estimation in dry-run mode 
- *(claude-runner)* Global and per-workflow concurrency limits 
- *(claude-runner)* Route-based config model  
- *(claude-runner)* Refine stage prompt templates for higher quality output 
- *(claude-runner)* Configurable stage prompt templates 
- *(claude-wrapper)* [**breaking**] Reshape Session around Arc<Claude>; add streaming 

### Miscellaneous

- Deprecate claude-pool and claude-pool-mcp in favor of claudes 
- Update .mcp.json from claude-pool to claudes 
- Bump crossterm from 0.28.1 to 0.29.0 
- Bump indicatif from 0.17.11 to 0.18.4 
- Bump tower-mcp from 0.8.4 to 0.9.1 
- Bump toml from 0.8.23 to 1.0.7+spec-1.1.0 
- Bump actions/download-artifact from 7 to 8 
- Bump peter-evans/create-pull-request from 7 to 8 
- Bump toml from 1.0.7+spec-1.1.0 to 1.1.0+spec-1.1.0 
- Bump tower-mcp from 0.9.1 to 0.10.0 
- Bump toml from 1.1.0+spec-1.1.0 to 1.1.2+spec-1.1.0 
- Bump tokio from 1.50.0 to 1.51.0 in the tokio-ecosystem group 
- Remove orphan tmp/ gitlink and gitignore it 
- Lock claude-runner publish to false 

### Testing

- *(claudes)* Add integration tests for hooks, shared, toml, autodiscovery 
- *(claudes)* Verify finally_hooks run on pre_hook failure 
- *(claudes)* Add integration tests for profiles, prompt_file, clean, status 
- *(claudes)* Comprehensive integration test suite  

## [claude-pool-mcp-v0.1.1] - 2026-03-16

### Miscellaneous

- Bump actions/upload-artifact from 6 to 7 
- Bump tempfile from 3.26.0 to 3.27.0 
- Bump clap from 4.5.60 to 4.6.0 
- Bump tracing-subscriber from 0.3.22 to 0.3.23 
- Bump docker/login-action from 3 to 4 
- Bump docker/setup-buildx-action from 3 to 4 
- Bump docker/metadata-action from 5 to 6 
- *(claude-pool-mcp)* Release v0.1.1 

## [0.4.0] - 2026-03-16

### Bug Fixes

- Streaming timeout + gitignore .claude-pool 
- Suppress unexpected_cfgs warnings for rest feature 
- Handle duplicate tags in release workflow 
- Prevent routing LLM from using tools instead of classifying 
- Create worktrees under repo instead of temp dir 

### Documentation

- Add comprehensive rustdoc to pool server tools 
- Move REST API reference into rustdoc module docs 
- Coordinator workflow as first-class concept 
- Comprehensive rustdoc for all public command builders 
- Update READMEs, .mcp.json, and workspace for release prep 

### Features

- REST API scaffold (Phase 1) 
- SSE streaming endpoints for REST API (Phase 2) 
- REST API Phase 4 — auth, concurrency limiting, webhooks, tests 
- Task execution metrics, session aggregation, and REST/MCP endpoints 
- Add pool-routing coordinator skill 
- Add pagination to REST list endpoints 
- Add json_schema to TaskOverrides for structured output 
- Add disallowed_tools and tools to TaskOverrides for tool scoping 
- Add max_budget_usd to TaskOverrides for per-task budget caps 
- Add fallback_model to PoolConfig and SlotConfig 
- Add batch-monitor coordinator skill 
- Add skill-audit and repo-scanner skills 
- Add coordinator pre-flight check to pool:ready handler 
- Add Transport enum for MCP type safety 
- Add missing high-priority CLI flags to QueryCommand 
- Scaffold claudes CLI 
- Add missing subcommands and MCP OAuth flags 
- Add per-task budget enforcement to pool 
- Pool polish — worktree cleanup, workflow disk loading, JSON file store 
- Add pool examples and document examples in READMEs 
- Add auto-routing — LLM picks run/fan_out/chain 
- Structured auto-routing hints and modular prompt 
- Prompt refinement, route normalization, and stress test 
- Add claude-pool-mcp crate (tower-mcp based pool server) 
- Improve route_stress diagnostics and document system prompt findings 
- Add routing test harness with structured output 
- Harden routing prompt with decision tree, examples, and anti-patterns 
- Use system prompt and XML tags for auto-routing 
- Ship pool coordinator skill and remove old skill infrastructure 

### Miscellaneous

- Remove stale CLI flags (quiet, color, doctor --json, agents --json) 
- Release 

### Refactoring

- Extract executor.rs and cli_parsing.rs from pool.rs 
- Centralize ID generation 
- Organize lib.rs re-exports with prelude module 
- TaskOverrides + RunOptions builder 
- Trim batch-monitor skill from 389 to 95 lines 
- [**breaking**] Remove dead code — skills, workflows, pool-server, claudes 

### Testing

- Add CI-runnable fake-binary tests for wrapper commands and retry 
- Add route_stress as ignored integration test 

## [0.3.1] - 2026-03-11

### Bug Fixes

- Strip CLAUDECODE at startup and surface stderr in pool errors 
- Add --verbose when using stream-json output format 
- Preserve GitHub remote URL in clone isolation 
- Rewrite coordinator skill prompts for haiku compatibility 

### Documentation

- Establish consistent user-facing vocabulary for pool operations 
- Add AGENTS.md for agent-assisted development 
- Fix quality gates and skills examples in claude-pool README 

### Features

- Add server metadata to pool_status and clone isolation mode 
- Implement inter-slot messaging for claude-pool 
- Adopt SKILL.md format and add global skills directory 
- Add structured failure details to TaskResult  
- Add broadcast messaging and slot discovery (#165, #166) 
- Session fix, plan-then-execute skill, $ARGUMENTS substitution (#161, #167, #162) 
- Add pool_dashboard and chain_watcher loop monitoring skills 
- Add auto-delivery messaging and self-claiming task queue (#169, #170) 
- Align skills with Agent Skills standard  
- Add ${CLAUDE_SKILL_DIR} substitution and skill directory docs 
- Add chain workflow and triage skills for claude-pool 
- Add quality gate hooks for task lifecycle 

### Miscellaneous

- Move builtin skills to SKILL.md files  
- Remove legacy JSON skills, migrate project_release to SKILL.md 
- Release 

## [0.3.0] - 2026-03-11

### Bug Fixes

- Capture output in single run instead of re-executing on non-zero exit 
- Detect repo root for worktree isolation 

### Documentation

- Add installation and deployment guidance 
- Add model selection heuristics to server instructions 
- Make tool surface and server instructions workflow-agnostic 
- Update READMEs for release readiness 

### Features

- Add --min-slots/--max-slots CLI flags and document dynamic scaling 
- Detect permission prompts in pool slot stderr 
- Add built-in create_pr skill 
- Load project-local skills from .claude-pool/skills/ directory 
- [**breaking**] Add skill scopes and extract project-specific skills 
- Add skill management tools (list/get/add/remove/save) 
- Add supervisor loop for slot health monitoring 
- Pass MCP config to pool slots 
- Fix cost tracking by matching CLI's total_cost_usd field 
- Add chain cancellation (pool_cancel_chain) 
- Live output for running chain steps 
- Per-chain worktree isolation opt-in  
- Add HTTP transport for claude-pool-server 
- Structured inter-step context for chains 
- Default chain isolation to worktree and add rebase skill 
- Add --json support to AgentsCommand and DoctorCommand 
- Add --json support to AgentsCommand and DoctorCommand 
- Add global flag helpers to ClaudeBuilder 
- Add Session management abstraction 

### Miscellaneous

- Release 

### Testing

- Add fake-claude binary and integration test infrastructure 
- Add claude-wrapper integration tests for streaming, timeout, and errors 
- Add claude-pool-server tool handler tests 
- Add claude-pool integration tests for pool lifecycle, chains, and supervisor 

## [claude-pool-v0.1.0] - 2026-03-10

### Bug Fixes

- Add version specs to workspace dependencies for crates.io publishing 

## [0.2.1] - 2026-03-10

### Bug Fixes

- Changelog workflow detached HEAD and branch protection 

### Documentation

- Add multi-worker coordination guidance 
- Add task sizing conventions to server instructions 
- Fix per-crate READMEs 
- Add project positioning to root README 

### Features

- Add project-specific skills for claude-wrapper workspace 
- Add execution mode guidance to server instructions 
- Add issue_watcher skill for GitHub issue-based workflow 
- Add loop_monitor skill for diff-based PR monitoring 
- Add pool_fan_out_chains for parallel multi-step pipelines 
- Add permission detection config fields to GlobalWorkerConfig 

### Miscellaneous

- Release 

### Refactoring

- [**breaking**] Rename worker to slot across codebase 

## [0.2.0] - 2026-03-10

### Bug Fixes

- [**breaking**] Correct AuthStatus fields to match CLI output, add license files and docs 
- Queue excess fan_out prompts when prompts exceed worker count 

### Documentation

- Update README with workspace overview and pool features 
- Add /loop scheduling tip to MCP server instructions 

### Features

- Add dynamic McpConfig tempfile and stream_query example 
- Add CLI version parsing and compatibility checks 
- Retry/backoff policies for transient failures 
- Add claude-pool worker pool and MCP server 
- Add enhanced worker identity (name, role, description) 
- Add --worktree CLI flag to claude-pool-server 
- Async chain execution with progress and failure policies 
- Add pool://chains/{chain_id} resource template 
- Add built-in workflow templates for common chain patterns 

### Miscellaneous

- Release v0.1.0 
- Bump which from 7.0.3 to 8.0.2 
- Add cargo-dist release workflow 
- Release v0.2.0 
- Add workflow_dispatch trigger to release-plz 

## [0.1.0] - 2026-03-09

### Features

- Initial implementation of claude-wrapper 

### Miscellaneous

- Initial commit


