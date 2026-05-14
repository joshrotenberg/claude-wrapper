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
