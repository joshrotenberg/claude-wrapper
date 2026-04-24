//! Server configuration. TOML-loadable, with sensible defaults for
//! every field so a zero-config server boots happily.
//!
//! The CLI binary reads this from disk; library users construct it
//! programmatically and pass it to [`super::build_router`].

use std::collections::HashMap;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// Top-level server config. Everything has a default; minimum config
/// = "use what's in PATH, no budget cap, all tools registered."
#[derive(Debug, Default, Clone, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct ServerConfig {
    /// Configuration for the underlying [`crate::Claude`] client.
    pub claude: ClaudeConfig,

    /// Server-wide policy toggles.
    pub server: ServerPolicy,

    /// Defaults applied to agent surface (`agent.*`) tools.
    pub agent: AgentConfig,

    /// Optional global budget tracker. Applied to every chat session
    /// and (per [`ServerPolicy::apply_budget_to_cli`]) to
    /// cli surface `query`/`query_json` calls.
    pub budget: Option<BudgetConfig>,

    /// Filesystem isolation for the inner claude. When enabled, the
    /// server overrides `HOME`, `XDG_CONFIG_HOME`, `CLAUDE_CONFIG_DIR`,
    /// and the working dir on every CLI invocation so the inner claude
    /// sees a sandbox tree instead of the host's real `~/.claude`.
    pub sandbox: SandboxConfig,
}

impl ServerConfig {
    /// Parse a config from a TOML string.
    pub fn from_toml_str(s: &str) -> Result<Self, toml::de::Error> {
        toml::from_str(s)
    }

    /// Read and parse a TOML config file.
    pub fn from_path(path: impl AsRef<std::path::Path>) -> std::io::Result<Self> {
        let text = std::fs::read_to_string(path)?;
        Self::from_toml_str(&text)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e.to_string()))
    }
}

/// Maps to [`crate::ClaudeBuilder`].
#[derive(Debug, Default, Clone, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct ClaudeConfig {
    /// Path to the `claude` binary. Default: `which("claude")`.
    pub binary: Option<PathBuf>,
    /// Working directory for child processes.
    pub working_dir: Option<PathBuf>,
    /// Per-command timeout in seconds.
    pub timeout_secs: Option<u64>,
    /// Environment variables passed to every child.
    pub env: HashMap<String, String>,
    /// Global args prepended to every subcommand.
    pub global_args: Vec<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct ServerPolicy {
    /// If false, mutating cli surface tools (mcp.add/remove,
    /// plugin.install/uninstall, marketplace.add/remove,
    /// install/update, etc.) are not registered.
    pub allow_mutations: bool,
    /// If false, `claude.raw` is not registered.
    pub allow_raw: bool,
    /// If true, the global budget tracker also applies to cli surface
    /// `query`/`query_json` calls (in addition to all agent surface).
    pub apply_budget_to_cli: bool,
}

impl Default for ServerPolicy {
    fn default() -> Self {
        Self {
            allow_mutations: true,
            allow_raw: false,
            apply_budget_to_cli: true,
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct AgentConfig {
    /// Default model passed to every `agent.*` call when the caller
    /// doesn't supply one.
    pub default_model: Option<String>,
    /// Default permission mode (string accepted by the CLI:
    /// `"acceptEdits"`, `"plan"`, etc.).
    pub default_permission_mode: Option<String>,
    /// Default allowed-tool patterns.
    pub default_allowed_tools: Vec<String>,
    /// Default disallowed-tool patterns.
    pub default_disallowed_tools: Vec<String>,
    /// Default system prompt.
    pub default_system_prompt: Option<String>,
    /// If true, every `agent.*` call passes `--bare`. Recommended
    /// for service deployments to avoid leaking host CLAUDE.md /
    /// hooks / keychain into requests.
    pub bare: bool,
    /// How long an idle chat survives before the registry drops it.
    /// (v0 does not actually evict yet; field is reserved.)
    pub idle_timeout_secs: u64,
}

impl Default for AgentConfig {
    fn default() -> Self {
        Self {
            default_model: None,
            default_permission_mode: None,
            default_allowed_tools: Vec::new(),
            default_disallowed_tools: Vec::new(),
            default_system_prompt: None,
            // Default off: `--bare` restricts auth to ANTHROPIC_API_KEY or
            // apiKeyHelper and disables keychain/OAuth reads. That breaks
            // the dominant "host has an authed claude" case (macOS keychain
            // auth via Claude Pro/Max). Service operators wanting the
            // deterministic-headless behaviour should set this to true
            // explicitly in their config. Validated during real-world
            // nested-claude testing -- see PR #555 discussion.
            bare: false,
            idle_timeout_secs: 1800,
        }
    }
}

#[derive(Debug, Default, Clone, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct BudgetConfig {
    pub max_usd: Option<f64>,
    pub warn_at_usd: Option<f64>,
}

/// Sandbox isolation modes.
///
/// - `Off`: no isolation. The inner claude inherits the server
///   process's environment and reads/writes `~/.claude` as the host
///   user. Fine for "I trust the server because I run it." Default.
/// - `PerServer`: one sandbox per server instance, shared by every
///   tool call. The inner claude sees an isolated `HOME` / config
///   dir and runs in `<sandbox>/workspace`. Repeated server starts
///   with the same `name` reuse the same sandbox so sessions persist.
///
/// Future: `PerChat` would give each `agent.chat.open` its own
/// sandbox, fully isolating chats from each other.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SandboxMode {
    #[default]
    Off,
    PerServer,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct SandboxConfig {
    /// Isolation mode. Default: `off`.
    pub mode: SandboxMode,
    /// Root directory holding sandbox subtrees. Default:
    /// `$HOME/.cache/claude-server` (or `/tmp/claude-server` if
    /// `$HOME` is unset).
    pub base_dir: Option<PathBuf>,
    /// Sandbox subdirectory name. Stable across server restarts so
    /// sessions persist; `rm -rf <base_dir>/<name>` to reset.
    /// Default: `default`.
    pub name: String,
    /// Copy host's `~/.claude/credentials.json` into the sandbox at
    /// boot if present. Lets the sandboxed claude authenticate with
    /// the host user's existing OAuth/keychain auth without forcing
    /// `bare = true`. Snapshot-on-boot: if the host re-auths after
    /// the server starts, the sandbox keeps the old credentials
    /// until the next server restart. Default: true.
    pub inherit_credentials: bool,
    /// Copy host's `~/.claude/settings.json` into the sandbox at
    /// boot. Most callers want a fresh `settings.json` so the
    /// server's claude is deterministic; default false.
    pub inherit_settings: bool,
}

impl Default for SandboxConfig {
    fn default() -> Self {
        Self {
            mode: SandboxMode::Off,
            base_dir: None,
            name: "default".to_string(),
            inherit_credentials: true,
            inherit_settings: false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_toml_uses_defaults() {
        let cfg = ServerConfig::from_toml_str("").unwrap();
        assert!(cfg.server.allow_mutations);
        assert!(!cfg.server.allow_raw);
        assert!(!cfg.agent.bare);
        assert_eq!(cfg.agent.idle_timeout_secs, 1800);
        assert!(cfg.claude.binary.is_none());
        assert!(cfg.budget.is_none());
    }

    #[test]
    fn parses_full_config() {
        let toml = r#"
[claude]
binary = "/usr/local/bin/claude"
timeout_secs = 600

[claude.env]
ANTHROPIC_API_KEY = "sk-..."

[server]
allow_mutations = false
allow_raw = false
apply_budget_to_cli = true

[agent]
default_model = "sonnet"
bare = true
idle_timeout_secs = 600

[budget]
max_usd = 10.0
warn_at_usd = 8.0
"#;
        let cfg = ServerConfig::from_toml_str(toml).unwrap();
        assert_eq!(
            cfg.claude.binary.as_deref(),
            Some(std::path::Path::new("/usr/local/bin/claude"))
        );
        assert!(!cfg.server.allow_mutations);
        assert_eq!(cfg.agent.default_model.as_deref(), Some("sonnet"));
        assert_eq!(cfg.budget.as_ref().and_then(|b| b.max_usd), Some(10.0));
    }
}
