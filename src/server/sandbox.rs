//! Filesystem isolation for the inner claude.
//!
//! When [`SandboxMode::Env`] is enabled, the server creates an
//! isolated tree on disk:
//!
//! ```text
//! <base_dir>/<name>/
//! ├── home/
//! │   ├── .claude.json                # copied from host (auth_strategy=inherit)
//! │   ├── .claude/                    # claude populates as it runs
//! │   │   └── credentials.json        # copied from host if present (Linux mostly)
//! │   ├── .config/                    # XDG_CONFIG_HOME
//! │   └── Library/Keychains -> ...    # macOS only: symlink to host's
//! └── workspace/                      # claude's cwd
//! ```
//!
//! Server redirects `HOME`, `XDG_CONFIG_HOME`, and the working
//! directory. It does **not** set `CLAUDE_CONFIG_DIR` -- empirically,
//! doing so makes claude look for `<dir>/.claude.json` instead of
//! `<HOME>/.claude.json` and breaks auth even when the file exists in
//! HOME. Without `CLAUDE_CONFIG_DIR`, claude uses its normal HOME-rooted
//! lookup and isolation is achieved entirely through the HOME redirect.
//!
//! macOS keychain access (`security find-generic-password`) reads from
//! `<HOME>/Library/Keychains/login.keychain-db`. Since HOME is
//! redirected, the sandbox needs a symlink back to the host's
//! Keychains dir or auth fails. On Linux, where keychain isn't a
//! thing, the symlink helper is a no-op.
//!
//! This is the lighter-weight alternative to running the server in a
//! container: same isolation goal (pin claude's view of the
//! filesystem to a known dir we control), no Docker dependency.
//! Containers can layer on top later for callers who want full
//! filesystem / network / process isolation in addition.

use std::path::{Path, PathBuf};

use super::config::{AuthStrategy, ClaudeConfig, SandboxConfig, SandboxMode};
use crate::error::{Error, Result};

/// Env var inspected by `AuthStrategy::ApiKey`.
const API_KEY_ENV: &str = "ANTHROPIC_API_KEY";

/// A materialised per-server sandbox.
///
/// Holds the resolved paths and the resolved auth strategy. Use
/// [`apply_to`](Self::apply_to) to inject env / cwd overrides into
/// a [`ClaudeConfig`] before the [`crate::Claude`] client is built.
#[derive(Debug, Clone)]
pub struct Sandbox {
    home: PathBuf,
    workspace: PathBuf,
    config_dir: PathBuf,
    auth_strategy: AuthStrategy,
    /// API key resolved at sandbox creation time when
    /// `auth_strategy = ApiKey`. Cached so per-call `apply_to`
    /// stays cheap and deterministic.
    resolved_api_key: Option<String>,
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

        let resolved_api_key = match cfg.auth_strategy {
            AuthStrategy::None => None,
            AuthStrategy::Inherit => {
                // Three pieces contribute to "is this user logged in":
                //
                // 1. `<HOME>/.claude.json` -- the top-level config
                //    file holding `oauthAccount`, user preferences,
                //    recent sessions list. Claude reads this on
                //    startup; without it the sandbox is treated as
                //    a fresh unconfigured account.
                // 2. `<HOME>/.claude/credentials.json` -- the actual
                //    OAuth credential file when present. macOS users
                //    typically don't have it (auth is in keychain);
                //    Linux users do.
                // 3. `<HOME>/Library/Keychains/login.keychain-db` --
                //    macOS keychain location. Symlinked back to the
                //    host's so `security find-generic-password`
                //    succeeds inside the sandbox. No-op on Linux
                //    (the dir doesn't exist on the host).
                copy_if_present(&host_home(), &home, ".claude.json")?;
                copy_if_present(&host_claude_dir(), &claude_dir, "credentials.json")?;
                symlink_host_keychains_if_present(&home)?;
                None
            }
            AuthStrategy::ApiKey => {
                // Resolve from process env (or claude_cfg.env later).
                // Fail fast at server boot if the user picked api_key
                // and didn't actually supply one.
                let key = std::env::var(API_KEY_ENV).ok().filter(|s| !s.is_empty());
                if key.is_none() {
                    return Err(Error::Io {
                        message: format!(
                            "auth_strategy = api_key but {API_KEY_ENV} is not set in the server's \
                             environment; export it before starting the server, or pick a \
                             different auth_strategy"
                        ),
                        source: std::io::Error::new(
                            std::io::ErrorKind::NotFound,
                            "missing ANTHROPIC_API_KEY",
                        ),
                        working_dir: Some(root.clone()),
                    });
                }
                key
            }
        };
        if cfg.inherit_settings {
            copy_if_present(&host_claude_dir(), &claude_dir, "settings.json")?;
        }

        Ok(Self {
            home,
            workspace,
            config_dir: claude_dir,
            auth_strategy: cfg.auth_strategy,
            resolved_api_key,
        })
    }

    /// Inject sandbox env + cwd into a [`ClaudeConfig`]. Caller-set
    /// values win: if the user explicitly supplied `working_dir` or
    /// `HOME` in their config, we don't overwrite them.
    ///
    /// Sets `HOME` and `XDG_CONFIG_HOME` only. Deliberately does not
    /// set `CLAUDE_CONFIG_DIR` -- doing so makes claude look for
    /// `<dir>/.claude.json` instead of the standard `<HOME>/.claude.json`,
    /// and our HOME-rooted layout doesn't put a file there.
    ///
    /// For [`AuthStrategy::ApiKey`], also injects `ANTHROPIC_API_KEY`
    /// (resolved at sandbox creation time) into the env.
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
        if self.auth_strategy == AuthStrategy::ApiKey
            && let Some(ref key) = self.resolved_api_key
        {
            claude_cfg
                .env
                .entry(API_KEY_ENV.to_string())
                .or_insert_with(|| key.clone());
        }
    }

    /// The auth strategy this sandbox was built for.
    pub fn auth_strategy(&self) -> AuthStrategy {
        self.auth_strategy
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
        SandboxMode::Env => Sandbox::create(cfg).map(Some),
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

/// Symlink `<sandbox_home>/Library/Keychains` -> the host's
/// `~/Library/Keychains` so macOS keychain auth survives the HOME
/// redirect. No-op if the host doesn't have the dir (Linux).
///
/// Idempotent: replaces an existing symlink at the target if it
/// points elsewhere; leaves correct symlinks alone.
fn symlink_host_keychains_if_present(sandbox_home: &Path) -> Result<()> {
    let host_keychains = host_home().join("Library").join("Keychains");
    if !host_keychains.is_dir() {
        return Ok(());
    }
    let parent = sandbox_home.join("Library");
    let link = parent.join("Keychains");
    std::fs::create_dir_all(&parent).map_err(|e| Error::Io {
        message: format!(
            "failed to create sandbox Library dir {}: {e}",
            parent.display()
        ),
        source: e,
        working_dir: Some(sandbox_home.to_path_buf()),
    })?;
    // If a stale symlink/dir is in the way, remove and recreate.
    if link.exists() || link.symlink_metadata().is_ok() {
        let _ = std::fs::remove_file(&link).or_else(|_| std::fs::remove_dir_all(&link));
    }
    #[cfg(unix)]
    std::os::unix::fs::symlink(&host_keychains, &link).map_err(|e| Error::Io {
        message: format!(
            "failed to symlink {} -> {}: {e}",
            link.display(),
            host_keychains.display()
        ),
        source: e,
        working_dir: Some(sandbox_home.to_path_buf()),
    })?;
    Ok(())
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
            mode: SandboxMode::Env,
            base_dir: Some(tmp.path().to_path_buf()),
            name: "unit-test".to_string(),
            auth_strategy: AuthStrategy::None,
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
            mode: SandboxMode::Env,
            base_dir: Some(tmp.path().to_path_buf()),
            name: "apply-test".to_string(),
            auth_strategy: AuthStrategy::None,
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
        // Sandbox deliberately does NOT inject CLAUDE_CONFIG_DIR --
        // setting it makes claude look for `<dir>/.claude.json` in
        // place of the standard `<HOME>/.claude.json` and breaks
        // auth even when our copy is in HOME.
        assert!(
            !claude_cfg.env.contains_key("CLAUDE_CONFIG_DIR"),
            "sandbox.apply_to should not set CLAUDE_CONFIG_DIR"
        );
    }

    #[test]
    fn apply_to_does_not_overwrite_user_overrides() {
        let tmp = tempfile::tempdir().unwrap();
        let cfg = SandboxConfig {
            mode: SandboxMode::Env,
            base_dir: Some(tmp.path().to_path_buf()),
            name: "override-test".to_string(),
            auth_strategy: AuthStrategy::None,
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
        // XDG_CONFIG_HOME wasn't preset so the sandbox does fill it.
        assert_eq!(
            claude_cfg.env.get("XDG_CONFIG_HOME").map(String::as_str),
            Some(
                sandbox
                    .home()
                    .join(".config")
                    .display()
                    .to_string()
                    .as_str()
            )
        );
    }

    #[test]
    fn rerun_preserves_existing_files() {
        let tmp = tempfile::tempdir().unwrap();
        let cfg = SandboxConfig {
            mode: SandboxMode::Env,
            base_dir: Some(tmp.path().to_path_buf()),
            name: "persist-test".to_string(),
            auth_strategy: AuthStrategy::None,
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
    fn auth_strategy_inherit_copies_credentials_when_present() {
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
            mode: SandboxMode::Env,
            base_dir: Some(tmp.path().to_path_buf()),
            name: "creds-test".to_string(),
            auth_strategy: AuthStrategy::Inherit,
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

    #[test]
    fn auth_strategy_api_key_resolves_injects_and_fails_when_missing() {
        // Combined into one test because both halves manipulate the
        // same global ANTHROPIC_API_KEY env var; running them as
        // separate #[test] fns lets cargo's parallel runner race
        // them. Sequential within one fn is correct.
        let tmp = tempfile::tempdir().unwrap();
        let prev_key = env::var("ANTHROPIC_API_KEY").ok();

        // -- Half 1: env present, strategy resolves and injects. --
        // SAFETY: see prev_key save/restore below.
        unsafe { env::set_var("ANTHROPIC_API_KEY", "sk-test-fake") };
        let cfg_present = SandboxConfig {
            mode: SandboxMode::Env,
            base_dir: Some(tmp.path().to_path_buf()),
            name: "api-key-test-present".to_string(),
            auth_strategy: AuthStrategy::ApiKey,
            inherit_settings: false,
        };
        let sandbox = maybe_create(&cfg_present).unwrap().unwrap();
        assert_eq!(sandbox.auth_strategy(), AuthStrategy::ApiKey);
        let mut claude_cfg = ClaudeConfig::default();
        sandbox.apply_to(&mut claude_cfg);
        assert_eq!(
            claude_cfg.env.get(API_KEY_ENV).map(String::as_str),
            Some("sk-test-fake")
        );

        // -- Half 2: env missing, sandbox creation fails clearly. --
        unsafe { env::remove_var("ANTHROPIC_API_KEY") };
        let cfg_missing = SandboxConfig {
            mode: SandboxMode::Env,
            base_dir: Some(tmp.path().to_path_buf()),
            name: "api-key-test-missing".to_string(),
            auth_strategy: AuthStrategy::ApiKey,
            inherit_settings: false,
        };
        let result = maybe_create(&cfg_missing);
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("ANTHROPIC_API_KEY"),
            "error should name the missing env var"
        );

        // Restore.
        unsafe {
            match prev_key {
                Some(v) => env::set_var("ANTHROPIC_API_KEY", v),
                None => env::remove_var("ANTHROPIC_API_KEY"),
            }
        }
    }
}
