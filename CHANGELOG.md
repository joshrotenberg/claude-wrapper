## [0.10.1] - 2026-05-30

### 🚀 Features

- Add QueryCommand::worktree_named for explicit name (closes #616) (#618)
- Typed partial-message accessor on StreamEvent (closes #617) (#620)

### ⚙️ Miscellaneous Tasks

- Bump serde_json in the serde-ecosystem group (#615)
- Release v0.10.1 (#619)
## [0.10.0] - 2026-05-27

### 🚀 Features

- *(duplex)* DuplexOptions::resume + continue_session (#586)
- *(history)* Claude_wrapper::history JSONL session parser (#587)
- Artifacts module -- read agent definitions (#588)
- Typed worktree builder + slash command helpers (#589)
- Auth strategy detection from environment (#590)
- Typed auth errors -- classify CLI failures at exec time (#591)
- AgentsRoot write / write_new / delete (#592)
- Worktrees -- read-side git worktree introspection (#594)
- --agent / --agents typed builders on DuplexOptions (#595)
- Declare tested-against CLI range + runtime drift warning (#596)
- Plugin lifecycle parity with claude 2.1.143 (#603)
- Auth login modes + sso fix + auto_mode docstring (#605)
- ProjectPurgeCommand for `claude project purge` (#604)
- Jobs -- read-side background-job state introspection (#606)
- *(history)* Expand SessionSummary with preview, cost, tokens (#609)
- Skills -- read-side ~/.claude/skills/<stem>/SKILL.md (#611)
- *(duplex)* Permission_mode + dangerously_skip_permissions builders (#614)
- *(history)* Paginate list tools + fix aiTitle field name (#610)
- Commands -- read custom slash command files (#613)
- Settings -- read on-disk settings layers (#612)

### 🐛 Bug Fixes

- *(query)* Execute_json arg ordering broke `--output-format json` (#579)

### 💼 Other

- *(agents)* Claude agents is now a TUI; AgentsCommand can't list (#593)

### 🚜 Refactor

- *(workspace)* Move claude-wrapper crate into crates/ (#581)

### 📚 Documentation

- Add root README and restore workspace-level LICENSE files (#583)

### ⚙️ Miscellaneous Tasks

- Bump tokio from 1.52.1 to 1.52.3 in the tokio-ecosystem group (#578)
- Pin macos-13 to dodge macos-latest rustup-init quirk (#582)
- Release v0.10.0 (#580)
## [0.9.0] - 2026-05-08

### 🚀 Features

- Add DuplexSession for long-lived stream-json conversations (#562)
- Add DuplexSession::subscribe with classified InboundEvent stream (#564)
- Add mid-turn permission handling to DuplexSession (#565)
- Add DuplexSession::interrupt for clean mid-turn cancel (#566)
- Add Conversation wrapper for DuplexSession bookkeeping (#574)
- Add health/watchdog primitives to DuplexSession (#575)
- *(examples)* Add minimal HTTP claude-as-a-service example (#576)

### 📚 Documentation

- Reposition DuplexSession as the recommended multi-turn primitive (#569)
- *(examples)* Add DuplexSession examples for chat and interrupt (#570)

### 🧪 Testing

- *(duplex)* Expand live integration coverage and tighten assertions (#568)

### ⚙️ Miscellaneous Tasks

- Update changelog (#560)
- Release v0.9.0 (#563)
## [0.8.0] - 2026-05-04

### 🚀 Features

- Add Xhigh variant to Effort enum (#559)

### ⚙️ Miscellaneous Tasks

- Update changelog (#556)
- Bump tokio from 1.51.0 to 1.52.1 in the tokio-ecosystem group (#558)
- Release v0.7.1 (#557)
## [0.7.0] - 2026-04-24

### 🚀 Features

- Add BudgetTracker with warning/exceeded callbacks (#543)
- Typed ToolPattern for allowed/disallowed tool lists (#545)
- Add sync feature with blocking exec/retry twins (#547)
- Sync execute_sync on commands + Claude version helpers (#548)
- Stream_query_sync for blocking NDJSON streaming (#549)
- [**breaking**] Make tokio optional via new async feature (#550)
- Wrap --bare + 4 other query flags, add auto-mode subcommands (#553)
- Wrap remaining #552 items (from_pr, plugin tag, update, install) (#554)

### ⚙️ Miscellaneous Tasks

- Update changelog (#542)
- [**breaking**] Drop deprecated crates, flatten workspace, rewrite docs (#551)
- Release v0.7.0 (#546)
## [0.6.0] - 2026-04-23

### 🚀 Features

- Isolate bypass-permissions behind `dangerous` module + env-var gate (#540)

### ⚙️ Miscellaneous Tasks

- Release v0.6.0 (#541)
## [0.5.1] - 2026-04-10

### 📚 Documentation

- Deprecation banners on unmaintained crates; audit wrapper README (#531)

### ⚙️ Miscellaneous Tasks

- Update changelog (#529)
- Exclude deprecated crates from workspace (#530)
- Remove stale root-level files; rewrite AGENTS.md for 0.5.0 (#532)
- Release v0.5.1 (#317)
## [0.5.0] - 2026-04-10

### 🚀 Features

- Enable worktree isolation by default for pool slots (#316)
- Add claudes manifest-driven execution engine (#318)
- *(claudes)* Tests, examples, state file, cleanup policy (#319)
- *(claudes)* Stream task events during execution (#330)
- *(claudes)* Add TaskBuilder (#331)
- *(claudes)* Add RunOptionsBuilder and PlanOptionsBuilder (#332)
- *(claudes)* Smarter auto-generated task names (#333)
- *(claudes)* Run IDs and state history (#334)
- *(claudes)* Add init subcommand for manifest templates (#338)
- *(claudes)* Add post_hooks support (#339)
- *(claudes)* Add shared block to manifest schema (#343)
- *(claudes)* Add output verbosity levels (#344)
- *(claudes)* Add TOML manifest parsing (#349)
- *(claudes)* Add pre_hooks support (#352)
- *(claudes)* Distinguish timeout vs failure in status (#350)
- *(claudes)* Auto-discover project manifest files (#348)
- *(claudes)* Per-task colored output prefixes (#351)
- *(claudes)* Add per-task NDJSON log persistence (#354)
- *(claudes)* Add global defaults file support (#355)
- *(claudes)* Add prompt_file and task selection (#356)
- *(claudes)* Add named profile support (#357)
- *(claudes)* Aggregate cost from stream events (#362)
- *(claudes)* Enhance clean command with --runs and --branches (#360)
- *(claudes)* Add finally_hooks (always-run hooks) (#363)
- *(claudes)* Warn on potential file overlaps between tasks (#367)
- *(claudes)* Add fix subcommand for failed tasks (#369)
- *(claudes)* Add metrics command for run history analysis (#370)
- *(claudes)* Add generate command for AI-assisted manifest creation (#372)
- *(claudes)* Enhance generate with project context and add task-level metrics (#376)
- *(claudes)* Add MCP server (claudes serve) (#385)
- *(claudes)* Enhance MCP server with titles, descriptions, and instructions (#388)
- *(claudes)* Add background execution for run_manifest MCP tool (#395)
- *(claudes)* Show elapsed time and consistent format in streaming output (#397)
- *(claudes)* Add turn usage to metrics output (#398)
- *(claudes)* Show in-progress runs in status (#399)
- *(claudes)* Add structured tracing spans per task (#405)
- *(claudes)* Add per-run breakdown to metrics output (#404)
- *(claudes)* Add CLI commands to MCP tool responses (#403)
- *(claudes)* Colored status output for task results (#402)
- *(claudes)* Default isolation to worktree (#413) (#417)
- *(claudes)* Three distinct output modes (#415) (#418)
- *(claudes)* In-place indicatif spinners for progress mode (#422)
- *(claudes)* Task dependencies (depends_on) (#400) (#423)
- *(claudes)* Chains — manifest-level sugar for task dependencies (#419) (#424)
- *(claudes)* Breadcrumbs for cross-task context in dependency chains (#420) (#425)
- *(claudes)* Explicit skill injection and settings passthrough (#380) (#436)
- *(claudes)* Interactive mode — bare `claudes` launches orchestrator (#358) (#429)
- *(claudes)* Two-tier CLI help and workflow-ordered commands (#437)
- *(claudes)* Hook progress display and meaningful tool args (#434, #435) (#443)
- *(claudes)* Improved generate prompt and robust JSON extraction (#447)
- *(claudes)* Add conditional task execution with condition field (#444) (#449)
- Claude-runner — autonomous GitHub issue runner (#477)
- *(claude-runner)* Lease system via GitHub labels (#492)
- *(claude-runner)* Show cost estimation in dry-run mode (#493)
- *(claude-runner)* Global and per-workflow concurrency limits (#497)
- *(claude-runner)* Route-based config model (#496) (#503)
- *(claude-runner)* Refine stage prompt templates for higher quality output (#505)
- *(claude-runner)* Configurable stage prompt templates (#510)
- *(claude-wrapper)* [**breaking**] Reshape Session around Arc<Claude>; add streaming (#528)

### 🐛 Bug Fixes

- Changelog workflow creates PR instead of direct push (#312)
- *(claudes)* Bright color palette and relative path display (#365)
- *(claudes)* Strip worktree prefix from tool call paths (#390)
- *(claudes)* Truncate errors to first line in status table (#392)
- *(claudes)* Default to bypassPermissions for headless MCP runs (#394)
- *(claude-wrapper)* Add -- separator before prompt in query args (#410)
- *(claudes)* Make created_at and version optional in manifest deserialization (#409) (#416)
- *(claudes)* Suppress overlap warnings for sequenced tasks (#430) (#439)
- *(claudes)* Breadcrumb write permissions and per-run scoping (#432, #433) (#440)
- *(ci)* Add checks:write permission for security audit (#441)
- *(claudes)* Add CLI reference to interactive prompt (#431) (#438)
- *(claudes)* Desugar chains before checking file overlaps (#445)
- *(claudes)* Reuse worktree for chained tasks sharing a branch (#451)
- *(claudes)* Auto-inherit branch from dependency in chains (#464)
- *(claudes)* Truncate multi-line tool args in progress display (#465)
- *(claude-runner)* Improve branch name sanitization (#478)
- Add claude-runner to workspace members (#479)
- *(claude-runner)* Inject clarify stage dynamically instead of defaulting in feature workflow (#488)
- *(claude-runner)* Skip optional stages when issue is well-specified (#501)
- *(claude-runner)* Workflow selection should be label-based, not title-prefix inference (#507)
- *(claude-runner)* Rebase on origin/main before push and use git add -u (#512)
- *(claude-wrapper)* [**breaking**] Remove panicky Transport::from(&str), add TryFrom (#525)
- *(claude-wrapper)* Clean child cleanup on timeout; add missing tests (#527)

### 📚 Documentation

- *(claudes)* Add prompt guide (#341)
- *(claudes)* Add non-dev example manifests (#342)
- *(claudes)* Update prompting guide with dogfood lessons (#366)
- Claudes README and PROMPTING.md updates (#377)
- *(claudes)* Add seismic research example manifest (#421) (#426)

### 🧪 Testing

- *(claudes)* Add integration tests for hooks, shared, toml, autodiscovery (#371)
- *(claudes)* Verify finally_hooks run on pre_hook failure (#391)
- *(claudes)* Add integration tests for profiles, prompt_file, clean, status (#406)
- *(claudes)* Comprehensive integration test suite (#386) (#407)

### ⚙️ Miscellaneous Tasks

- Deprecate claude-pool and claude-pool-mcp in favor of claudes (#373)
- Update .mcp.json from claude-pool to claudes (#408)
- Bump crossterm from 0.28.1 to 0.29.0 (#518)
- Bump indicatif from 0.17.11 to 0.18.4 (#514)
- Bump tower-mcp from 0.8.4 to 0.9.1 (#517)
- Bump toml from 0.8.23 to 1.0.7+spec-1.1.0 (#516)
- Bump actions/download-artifact from 7 to 8 (#515)
- Bump peter-evans/create-pull-request from 7 to 8 (#513)
- Bump toml from 1.0.7+spec-1.1.0 to 1.1.0+spec-1.1.0 (#520)
- Bump tower-mcp from 0.9.1 to 0.10.0 (#519)
- Bump toml from 1.1.0+spec-1.1.0 to 1.1.2+spec-1.1.0 (#522)
- Bump tokio from 1.50.0 to 1.51.0 in the tokio-ecosystem group (#521)
- Remove orphan tmp/ gitlink and gitignore it (#526)
- Lock claude-runner publish to false (#524)
## [claude-pool-mcp-v0.1.1] - 2026-03-16

### ⚙️ Miscellaneous Tasks

- Bump actions/upload-artifact from 6 to 7 (#308)
- Bump tempfile from 3.26.0 to 3.27.0 (#309)
- Bump clap from 4.5.60 to 4.6.0 (#304)
- Bump tracing-subscriber from 0.3.22 to 0.3.23 (#307)
- Bump docker/login-action from 3 to 4 (#306)
- Bump docker/setup-buildx-action from 3 to 4 (#303)
- Bump docker/metadata-action from 5 to 6 (#302)
- *(claude-pool-mcp)* Release v0.1.1 (#311)
## [0.4.0] - 2026-03-16

### 🚀 Features

- REST API scaffold (Phase 1) (#211)
- SSE streaming endpoints for REST API (Phase 2) (#213)
- REST API Phase 4 — auth, concurrency limiting, webhooks, tests (#214)
- Task execution metrics, session aggregation, and REST/MCP endpoints (#216)
- Add pool-routing coordinator skill (#218)
- Add pagination to REST list endpoints (#219)
- Add json_schema to TaskOverrides for structured output (#230)
- Add disallowed_tools and tools to TaskOverrides for tool scoping (#231)
- Add max_budget_usd to TaskOverrides for per-task budget caps (#232)
- Add fallback_model to PoolConfig and SlotConfig (#236)
- Add batch-monitor coordinator skill (#237)
- Add skill-audit and repo-scanner skills (#242)
- Add coordinator pre-flight check to pool:ready handler (#245)
- Add Transport enum for MCP type safety (#251)
- Add missing high-priority CLI flags to QueryCommand (#258)
- Scaffold claudes CLI (#260)
- Add missing subcommands and MCP OAuth flags (#261)
- Add per-task budget enforcement to pool (#272)
- Pool polish — worktree cleanup, workflow disk loading, JSON file store (#275)
- Add pool examples and document examples in READMEs (#276)
- Add auto-routing — LLM picks run/fan_out/chain (#278)
- Structured auto-routing hints and modular prompt (#280)
- Prompt refinement, route normalization, and stress test (#281)
- Add claude-pool-mcp crate (tower-mcp based pool server) (#282)
- Improve route_stress diagnostics and document system prompt findings (#291)
- Add routing test harness with structured output (#293)
- Harden routing prompt with decision tree, examples, and anti-patterns (#294)
- Use system prompt and XML tags for auto-routing (#296)
- Ship pool coordinator skill and remove old skill infrastructure (#299)

### 🐛 Bug Fixes

- Streaming timeout + gitignore .claude-pool (#202)
- Suppress unexpected_cfgs warnings for rest feature (#212)
- Handle duplicate tags in release workflow (#217)
- Prevent routing LLM from using tools instead of classifying (#284)
- Create worktrees under repo instead of temp dir (#298)

### 🚜 Refactor

- Extract executor.rs and cli_parsing.rs from pool.rs (#204)
- Centralize ID generation (#207)
- Organize lib.rs re-exports with prelude module (#208)
- TaskOverrides + RunOptions builder (#209)
- Trim batch-monitor skill from 389 to 95 lines (#240)
- [**breaking**] Remove dead code — skills, workflows, pool-server, claudes (#283)

### 📚 Documentation

- Add comprehensive rustdoc to pool server tools (#205)
- Move REST API reference into rustdoc module docs (#220)
- Coordinator workflow as first-class concept (#247)
- Comprehensive rustdoc for all public command builders (#252)
- Update READMEs, .mcp.json, and workspace for release prep (#301)

### 🧪 Testing

- Add CI-runnable fake-binary tests for wrapper commands and retry (#273)
- Add route_stress as ignored integration test (#297)

### ⚙️ Miscellaneous Tasks

- Remove stale CLI flags (quiet, color, doctor --json, agents --json) (#259)
- Release (#300)
## [0.3.1] - 2026-03-11

### 🚀 Features

- Add server metadata to pool_status and clone isolation mode (#151)
- Implement inter-slot messaging for claude-pool (#153)
- Adopt SKILL.md format and add global skills directory (#157)
- Add structured failure details to TaskResult (#155) (#159)
- Add broadcast messaging and slot discovery (#165, #166) (#172)
- Session fix, plan-then-execute skill, $ARGUMENTS substitution (#161, #167, #162) (#173)
- Add pool_dashboard and chain_watcher loop monitoring skills (#174)
- Add auto-delivery messaging and self-claiming task queue (#169, #170) (#175)
- Align skills with Agent Skills standard (#162) (#179)
- Add ${CLAUDE_SKILL_DIR} substitution and skill directory docs (#181)
- Add chain workflow and triage skills for claude-pool (#182)
- Add quality gate hooks for task lifecycle (#183)

### 🐛 Bug Fixes

- Strip CLAUDECODE at startup and surface stderr in pool errors (#138)
- Add --verbose when using stream-json output format (#142)
- Preserve GitHub remote URL in clone isolation (#154)
- Rewrite coordinator skill prompts for haiku compatibility (#188)

### 📚 Documentation

- Establish consistent user-facing vocabulary for pool operations (#158)
- Add AGENTS.md for agent-assisted development (#176)
- Fix quality gates and skills examples in claude-pool README (#186)

### ⚙️ Miscellaneous Tasks

- Move builtin skills to SKILL.md files (#178) (#180)
- Remove legacy JSON skills, migrate project_release to SKILL.md (#189)
- Release (#139)
## [0.3.0] - 2026-03-11

### 🚀 Features

- Add --min-slots/--max-slots CLI flags and document dynamic scaling (#91)
- Detect permission prompts in pool slot stderr (#88)
- Add built-in create_pr skill (#90)
- Load project-local skills from .claude-pool/skills/ directory (#87)
- [**breaking**] Add skill scopes and extract project-specific skills (#93)
- Add skill management tools (list/get/add/remove/save) (#98)
- Add supervisor loop for slot health monitoring (#97)
- Pass MCP config to pool slots (#100)
- Fix cost tracking by matching CLI's total_cost_usd field (#106)
- Add chain cancellation (pool_cancel_chain) (#107)
- Live output for running chain steps (#108)
- Per-chain worktree isolation opt-in (#104) (#109)
- Add HTTP transport for claude-pool-server (#111)
- Structured inter-step context for chains (#112)
- Default chain isolation to worktree and add rebase skill (#114)
- Add --json support to AgentsCommand and DoctorCommand (#129)
- Add --json support to AgentsCommand and DoctorCommand (#128)
- Add global flag helpers to ClaudeBuilder (#131)
- Add Session management abstraction (#133)

### 🐛 Bug Fixes

- Capture output in single run instead of re-executing on non-zero exit (#130)
- Detect repo root for worktree isolation (#134)

### 📚 Documentation

- Add installation and deployment guidance (#84)
- Add model selection heuristics to server instructions (#89)
- Make tool surface and server instructions workflow-agnostic (#96)
- Update READMEs for release readiness (#135)

### 🧪 Testing

- Add fake-claude binary and integration test infrastructure (#119)
- Add claude-wrapper integration tests for streaming, timeout, and errors (#120)
- Add claude-pool-server tool handler tests (#126)
- Add claude-pool integration tests for pool lifecycle, chains, and supervisor (#127)

### ⚙️ Miscellaneous Tasks

- Release (#85)
## [claude-pool-v0.1.0] - 2026-03-10

### 🐛 Bug Fixes

- Add version specs to workspace dependencies for crates.io publishing (#83)
## [0.2.1] - 2026-03-10

### 🚀 Features

- Add project-specific skills for claude-wrapper workspace (#57)
- Add execution mode guidance to server instructions (#56)
- Add issue_watcher skill for GitHub issue-based workflow (#65)
- Add loop_monitor skill for diff-based PR monitoring (#71)
- Add pool_fan_out_chains for parallel multi-step pipelines (#79)
- Add permission detection config fields to GlobalWorkerConfig (#80)

### 🐛 Bug Fixes

- Changelog workflow detached HEAD and branch protection (#54)

### 🚜 Refactor

- [**breaking**] Rename worker to slot across codebase (#81)

### 📚 Documentation

- Add multi-worker coordination guidance (#63)
- Add task sizing conventions to server instructions (#68)
- Fix per-crate READMEs (#69)
- Add project positioning to root README (#70)

### ⚙️ Miscellaneous Tasks

- Release (#52)
## [0.2.0] - 2026-03-10

### 🚀 Features

- Add dynamic McpConfig tempfile and stream_query example (#15)
- Add CLI version parsing and compatibility checks (#16)
- Retry/backoff policies for transient failures (#17)
- Add claude-pool worker pool and MCP server (#25)
- Add enhanced worker identity (name, role, description) (#33)
- Add --worktree CLI flag to claude-pool-server (#32)
- Async chain execution with progress and failure policies (#38)
- Add pool://chains/{chain_id} resource template (#39)
- Add built-in workflow templates for common chain patterns (#46)

### 🐛 Bug Fixes

- [**breaking**] Correct AuthStatus fields to match CLI output, add license files and docs (#14)
- Queue excess fan_out prompts when prompts exceed worker count (#45)

### 📚 Documentation

- Update README with workspace overview and pool features (#44)
- Add /loop scheduling tip to MCP server instructions (#48)

### ⚙️ Miscellaneous Tasks

- Release v0.1.0 (#2)
- Bump which from 7.0.3 to 8.0.2 (#3)
- Add cargo-dist release workflow (#26)
- Release v0.2.0 (#4)
- Add workflow_dispatch trigger to release-plz (#51)
## [0.1.0] - 2026-03-09

### 🚀 Features

- Initial implementation of claude-wrapper (#1)

### ⚙️ Miscellaneous Tasks

- Initial commit
