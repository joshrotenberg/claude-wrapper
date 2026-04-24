//! Filesystem isolation for the inner claude.
//!
//! When [`SandboxMode::PerServer`] is enabled, the server creates an
//! isolated tree on disk:
//!
//! ```text
//! <base_dir>/<name>/
//! ├── home/
//! │   ├── .claude/             # CLAUDE_CONFIG_DIR points here
//! │   │   └── credentials.json # optional copy of host's
//! │   └── .config/             # XDG_CONFIG_HOME points here
//! └── workspace/               # claude's cwd
//! ```
//!
//! Every `claude` invocation by the server then runs with `HOME`,
//! `XDG_CONFIG_HOME`, and `CLAUDE_CONFIG_DIR` redirected into the
//! sandbox, and its working directory set to `<sandbox>/workspace`.
//! The host's real `~/.claude` is invisible to it.
//!
//! This is the lighter-weight alternative to running the server in a
//! container: same isolation goal (pin claude's view of the
//! filesystem to a known dir we control), no Docker dependency.
//! Containers can layer on top later for callers who want full
//! filesystem/network/process isolation in addition.

use std::path::{Path, PathBuf};

use super::config::{ClaudeConfig, SandboxConfig, SandboxMode};
use crate::error::{Error, Result};

/// A materialised per-server sandbox.
///
/// Holds the resolved paths and exposes [`apply_to`](Self::apply_to)
/// for injecting overrides into [`ClaudeConfig`] before the [`crate::Claude`]
/// client is built.
#[derive(Debug, Clone)]
pub struct Sandbox {
    home: PathBuf,
    workspace: PathBuf,
    config_dir: PathBuf,
}

impl Sandbox {
    /// Resolve, create-if-missing, and optionally seed a per-server
    /// sandbox. Idempotent: safe to call repeatedly with the same
    /// config; existing files (sessions, history) are preserved.
    pub fn create(cfg: &SandboxConfig) -> Result<Self> {
        let base = cfg.base_dir.clone().unwrap_or_else(default_base_dir);
        let root = base.join(&cfg.name);
        let home = root.join("home");
        let claude_dir = home.join(".claude");
        let xdg_config = home.join(".config");
        let workspace = root.join("workspace");

        for d in [&claude_dir, &xdg_config, &workspace] {
            std::fs::create_dir_all(d).map_err(|e| Error::Io {
                message: format!("failed to create sandbox dir {}: {e}", d.display()),
                source: e,
                working_dir: Some(root.clone()),
            })?;
        }

        if cfg.inherit_credentials {
            // Two files contribute to "is this user logged in":
            //
            // 1. `~/.claude/credentials.json` (when present; many
            //    setups don't have it -- macOS keychain users in
            //    particular).
            // 2. `~/.claude.json` -- the top-level config file
            //    holding the OAuth account id, user preferences,
            //    recent sessions list, etc. Without it, claude
            //    treats the sandbox HOME as a fresh unconfigured
            //    account and refuses with "Not logged in" even on
            //    keychain-authed hosts.
            //
            // We copy both. `.claude.json` is large (a few hundred
            // KB of accumulated state) so the sandbox starts with
            // some host-side history baked in. That's the trade
            // for "auth just works"; document and move on.
            copy_if_present(&host_claude_dir(), &claude_dir, "credentials.json")?;
            copy_if_present(&host_home(), &home, ".claude.json")?;
        }
        if cfg.inherit_settings {
            copy_if_present(&host_claude_dir(), &claude_dir, "settings.json")?;
        }

        Ok(Self {
            home,
            workspace,
            config_dir: claude_dir,
        })
    }

    /// Inject sandbox env + cwd into a [`ClaudeConfig`]. Caller-set
    /// values win: if the user explicitly supplied `working_dir` or
    /// `HOME` in their config, we don't overwrite them.
    pub fn apply_to(&self, claude_cfg: &mut ClaudeConfig) {
        if claude_cfg.working_dir.is_none() {
            claude_cfg.working_dir = Some(self.workspace.clone());
        }
        claude_cfg
            .env
            .entry("HOME".to_string())
            .or_insert_with(|| self.home.display().to_string());
        claude_cfg
            .env
            .entry("XDG_CONFIG_HOME".to_string())
            .or_insert_with(|| self.home.join(".config").display().to_string());
        claude_cfg
            .env
            .entry("CLAUDE_CONFIG_DIR".to_string())
            .or_insert_with(|| self.config_dir.display().to_string());
    }

    /// Path to the sandbox `home` directory.
    pub fn home(&self) -> &Path {
        &self.home
    }

    /// Path to the sandbox workspace (claude's cwd).
    pub fn workspace(&self) -> &Path {
        &self.workspace
    }

    /// Path to the sandbox claude config dir (`<home>/.claude`).
    pub fn config_dir(&self) -> &Path {
        &self.config_dir
    }
}

/// Build a sandbox per `cfg.mode`, returning `None` when off.
pub(crate) fn maybe_create(cfg: &SandboxConfig) -> Result<Option<Sandbox>> {
    match cfg.mode {
        SandboxMode::Off => Ok(None),
        SandboxMode::PerServer => Sandbox::create(cfg).map(Some),
    }
}

fn default_base_dir() -> PathBuf {
    if let Ok(home) = std::env::var("HOME")
        && !home.is_empty()
    {
        return PathBuf::from(home).join(".cache").join("claude-server");
    }
    PathBuf::from("/tmp/claude-server")
}

fn host_home() -> PathBuf {
    std::env::var("HOME").map(PathBuf::from).unwrap_or_default()
}

fn host_claude_dir() -> PathBuf {
    let home = host_home();
    if home.as_os_str().is_empty() {
        PathBuf::new()
    } else {
        home.join(".claude")
    }
}

/// Copy `<src_dir>/<filename>` to `<dst_dir>/<filename>` if the
/// source exists and the destination doesn't yet. No-op otherwise --
/// preserving sandbox state across restarts is intentional.
fn copy_if_present(src_dir: &Path, dst_dir: &Path, filename: &str) -> Result<()> {
    let src = src_dir.join(filename);
    let dst = dst_dir.join(filename);
    if !src.exists() || dst.exists() {
        return Ok(());
    }
    std::fs::copy(&src, &dst).map_err(|e| Error::Io {
        message: format!(
            "failed to copy {} into sandbox {}: {e}",
            src.display(),
            dst.display()
        ),
        source: e,
        working_dir: Some(dst_dir.to_path_buf()),
    })?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::env;

    #[test]
    fn off_mode_returns_none() {
        let cfg = SandboxConfig {
            mode: SandboxMode::Off,
            ..Default::default()
        };
        assert!(maybe_create(&cfg).unwrap().is_none());
    }

    #[test]
    fn per_server_creates_dir_tree() {
        let tmp = tempfile::tempdir().unwrap();
        let cfg = SandboxConfig {
            mode: SandboxMode::PerServer,
            base_dir: Some(tmp.path().to_path_buf()),
            name: "unit-test".to_string(),
            inherit_credentials: false,
            inherit_settings: false,
        };
        let sandbox = maybe_create(&cfg).unwrap().unwrap();

        assert!(sandbox.home().exists());
        assert!(sandbox.workspace().exists());
        assert!(sandbox.config_dir().exists());

        let root = tmp.path().join("unit-test");
        assert!(root.join("home").join(".claude").is_dir());
        assert!(root.join("home").join(".config").is_dir());
        assert!(root.join("workspace").is_dir());
    }

    #[test]
    fn apply_to_injects_env_and_cwd() {
        let tmp = tempfile::tempdir().unwrap();
        let cfg = SandboxConfig {
            mode: SandboxMode::PerServer,
            base_dir: Some(tmp.path().to_path_buf()),
            name: "apply-test".to_string(),
            inherit_credentials: false,
            inherit_settings: false,
        };
        let sandbox = maybe_create(&cfg).unwrap().unwrap();

        let mut claude_cfg = ClaudeConfig::default();
        sandbox.apply_to(&mut claude_cfg);

        assert_eq!(claude_cfg.working_dir.as_deref(), Some(sandbox.workspace()));
        assert_eq!(
            claude_cfg.env.get("HOME").map(String::as_str),
            Some(sandbox.home().display().to_string().as_str())
        );
        assert_eq!(
            claude_cfg.env.get("CLAUDE_CONFIG_DIR").map(String::as_str),
            Some(sandbox.config_dir().display().to_string().as_str())
        );
    }

    #[test]
    fn apply_to_does_not_overwrite_user_overrides() {
        let tmp = tempfile::tempdir().unwrap();
        let cfg = SandboxConfig {
            mode: SandboxMode::PerServer,
            base_dir: Some(tmp.path().to_path_buf()),
            name: "override-test".to_string(),
            inherit_credentials: false,
            inherit_settings: false,
        };
        let sandbox = maybe_create(&cfg).unwrap().unwrap();

        let mut claude_cfg = ClaudeConfig {
            working_dir: Some(PathBuf::from("/explicit/cwd")),
            ..Default::default()
        };
        claude_cfg
            .env
            .insert("HOME".to_string(), "/explicit/home".to_string());
        sandbox.apply_to(&mut claude_cfg);

        assert_eq!(
            claude_cfg.working_dir.as_deref(),
            Some(Path::new("/explicit/cwd"))
        );
        assert_eq!(
            claude_cfg.env.get("HOME").map(String::as_str),
            Some("/explicit/home")
        );
        // Unset CLAUDE_CONFIG_DIR did get the sandbox value.
        assert_eq!(
            claude_cfg.env.get("CLAUDE_CONFIG_DIR").map(String::as_str),
            Some(sandbox.config_dir().display().to_string().as_str())
        );
    }

    #[test]
    fn rerun_preserves_existing_files() {
        let tmp = tempfile::tempdir().unwrap();
        let cfg = SandboxConfig {
            mode: SandboxMode::PerServer,
            base_dir: Some(tmp.path().to_path_buf()),
            name: "persist-test".to_string(),
            inherit_credentials: false,
            inherit_settings: false,
        };
        let sandbox1 = maybe_create(&cfg).unwrap().unwrap();
        // Drop a marker file in the workspace.
        std::fs::write(sandbox1.workspace().join("marker"), "hello").unwrap();

        // Re-run with same config; marker should still be there.
        let sandbox2 = maybe_create(&cfg).unwrap().unwrap();
        assert_eq!(
            std::fs::read_to_string(sandbox2.workspace().join("marker")).unwrap(),
            "hello"
        );
    }

    #[test]
    fn inherit_credentials_copies_when_present() {
        let tmp = tempfile::tempdir().unwrap();
        let fake_home = tmp.path().join("fake-host-home");
        let fake_claude = fake_home.join(".claude");
        std::fs::create_dir_all(&fake_claude).unwrap();
        std::fs::write(fake_claude.join("credentials.json"), r#"{"fake":"creds"}"#).unwrap();

        // Override $HOME for this test so host_claude_dir() points at our fake.
        // SAFETY: tests in a single process run sequentially per default cargo
        // settings; this test sets and restores $HOME locally.
        let prev_home = env::var("HOME").ok();
        unsafe { env::set_var("HOME", fake_home.display().to_string()) };

        let cfg = SandboxConfig {
            mode: SandboxMode::PerServer,
            base_dir: Some(tmp.path().to_path_buf()),
            name: "creds-test".to_string(),
            inherit_credentials: true,
            inherit_settings: false,
        };
        let sandbox = maybe_create(&cfg).unwrap().unwrap();

        let copied = sandbox.config_dir().join("credentials.json");
        assert!(copied.exists());
        assert_eq!(
            std::fs::read_to_string(&copied).unwrap(),
            r#"{"fake":"creds"}"#
        );

        // Restore $HOME so subsequent tests aren't affected.
        unsafe {
            match prev_home {
                Some(v) => env::set_var("HOME", v),
                None => env::remove_var("HOME"),
            }
        }
    }
}
