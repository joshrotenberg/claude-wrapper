//! Server configuration. Loaded from TOML or built in code.

use std::collections::BTreeMap;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// Top-level server configuration. Map from a TOML file or build by
/// hand. Default is "talk to whatever `claude` is on `$PATH`, no
/// special env, default timeout."
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ServerConfig {
    /// Wrapper-side knobs: binary, working dir, env, global args.
    pub claude: ClaudeConfig,
    /// Async-turn registry knobs: TTL + sweeper cadence. Defaults
    /// to 1 hour TTL with a 60-second sweep interval.
    pub turns: TurnConfig,
    /// Server-level policy: what mutating operations are allowed.
    /// Defaults are deliberately conservative (no mutations).
    pub policy: ServerPolicy,
    /// Override the on-disk history root that the `history` feature
    /// reads from. Defaults to `~/.claude/projects`. Useful for
    /// tests (point at a tempdir) and non-default Claude Code
    /// installs. Only consulted when the `history` Cargo feature
    /// is enabled.
    pub history_root: Option<PathBuf>,
    /// Override the on-disk agents root that the `artifacts` feature
    /// reads from. Defaults to `~/.claude/agents`. Same semantics
    /// as [`Self::history_root`] -- intended for tests and
    /// non-default Claude Code installs. Only consulted when the
    /// `artifacts` Cargo feature is enabled.
    pub agents_root: Option<PathBuf>,
    /// Override the repository path that the `worktrees` feature
    /// targets when no explicit `repo_path` is passed to
    /// `worktree_list`. Defaults to [`ClaudeConfig::working_dir`]
    /// when unset, or the process cwd if neither is set. Useful for
    /// tests (point at a `git init`'d tempdir) and for servers that
    /// want a per-server "default repo." Only consulted when the
    /// `worktrees` Cargo feature is enabled.
    pub worktrees_root: Option<PathBuf>,
    /// Override the on-disk jobs root that the `jobs` feature reads
    /// from. Defaults to `~/.claude/jobs`. Same semantics as
    /// [`Self::history_root`] -- intended for tests and non-default
    /// Claude Code installs. Only consulted when the `jobs` Cargo
    /// feature is enabled.
    pub jobs_root: Option<PathBuf>,
    /// Per-surface runtime gates. Layered on top of the compile-time
    /// Cargo features: a Cargo feature being off means a surface is
    /// **never** registered regardless of this config; a Cargo
    /// feature being on means the surface defaults to registered
    /// but can be disabled per-deployment via these flags.
    pub surfaces: SurfacesConfig,
}

/// Per-surface runtime gates. All default `true` so out-of-the-box
/// behavior matches the historical "if it's compiled in, it's
/// registered" policy. Operators flip individual flags to `false`
/// when they want a single binary that exposes only a subset.
///
/// **Cargo features remain the hard ceiling.** Setting
/// `enable_artifacts = true` on a binary built without the
/// `artifacts` feature does nothing -- the surface code isn't
/// linked in.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct SurfacesConfig {
    /// L2 CLI passthrough tools (`claude_*`). Default `true`.
    pub enable_core: bool,
    /// L2.5 long-lived chat surface (`chat_*`, `turn_*`,
    /// `claude://chats[/{id}]`). Default `true`.
    pub enable_chat: bool,
    /// Process counters (`metrics_summary`, `claude://metrics`).
    /// Default `true`.
    pub enable_metrics: bool,
    /// Mutating CLI passthrough (`claude_mcp_add`,
    /// `claude_plugin_install`, etc.) AND mutating artifact tools
    /// (`agent_write`, `agent_delete`). Layered on top of the
    /// existing `policy.allow_mutations` runtime check and the
    /// `mutations` Cargo feature -- ALL three must be true for
    /// mutating tools to register. Default `true`.
    pub enable_mutations: bool,
    /// Historical session reads (`claude_project_list`,
    /// `claude_session_*`, `claude://projects`, `claude://sessions/{id}`).
    /// Default `true`.
    pub enable_history: bool,
    /// User-level agent artifact reads (`agent_list`, `agent_get`,
    /// `claude://agents[/{stem}]`). Default `true`.
    pub enable_artifacts: bool,
    /// Git worktree introspection (`worktree_list`,
    /// `claude://worktrees`). Default `true`.
    pub enable_worktrees: bool,
    /// Background-job state reads (`claude_job_list`,
    /// `claude_job_get`, `claude://jobs[/{short_id}]`). Default `true`.
    pub enable_jobs: bool,
}

impl Default for SurfacesConfig {
    fn default() -> Self {
        Self {
            enable_core: true,
            enable_chat: true,
            enable_metrics: true,
            enable_mutations: true,
            enable_history: true,
            enable_artifacts: true,
            enable_worktrees: true,
            enable_jobs: true,
        }
    }
}

/// Server-level policy flags. Mutating tools (mcp_add, plugin_install,
/// etc.) are NOT registered unless [`Self::allow_mutations`] is true.
/// When off the model literally cannot discover them.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ServerPolicy {
    /// When true, register CLI mutating tools that change MCP server
    /// config, plugins, marketplaces. Default false because an
    /// unmonitored coordinator could otherwise rewrite your claude
    /// setup without warning.
    pub allow_mutations: bool,
}

/// Knobs for the async-turn registry's TTL eviction.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct TurnConfig {
    /// Terminal turns (done/failed/cancelled) older than this are
    /// evicted by the background sweeper. In seconds.
    pub ttl_secs: u64,
    /// How often the sweeper runs. In seconds.
    pub sweep_interval_secs: u64,
}

impl Default for TurnConfig {
    fn default() -> Self {
        Self {
            ttl_secs: 3600, // 1 hour
            sweep_interval_secs: 60,
        }
    }
}

/// Inputs to the [`claude_wrapper::ClaudeBuilder`].
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ClaudeConfig {
    /// Override the `claude` binary path. Defaults to `$PATH` lookup.
    pub binary: Option<PathBuf>,
    /// Default working directory for spawned processes. Tools may
    /// override per call where it makes sense.
    pub working_dir: Option<PathBuf>,
    /// Per-invocation timeout in seconds. Defaults to 5 minutes.
    pub timeout_secs: Option<u64>,
    /// Extra environment variables passed to every spawned `claude`.
    pub env: BTreeMap<String, String>,
    /// Global args prepended to every `claude` invocation (after the
    /// binary, before the subcommand). Useful for `--no-color` and
    /// the like.
    pub global_args: Vec<String>,
}
