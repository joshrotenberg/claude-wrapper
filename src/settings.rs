//! Read-side access to Claude Code's on-disk **settings** files.
//!
//! Claude Code reads up to four JSON files in increasing order of
//! precedence (later layers override earlier ones):
//!
//! 1. `~/.claude/settings.json` -- user defaults
//! 2. `~/.claude/settings.local.json` -- user-private overrides
//! 3. `<project>/.claude/settings.json` -- project-shared (checked
//!    into version control)
//! 4. `<project>/.claude/settings.local.json` -- project-private
//!    overrides (typically gitignored)
//!
//! Plus a managed (enterprise) layer that lives outside these paths
//! and isn't covered here.
//!
//! This module reads each layer as opaque [`serde_json::Value`] and
//! returns them side-by-side, **without merging**. Claude Code's
//! merge semantics are non-trivial and not fully documented (some
//! object fields deep-merge, some arrays concatenate, some replace),
//! and reproducing them here would risk diverging from the binary's
//! actual behavior in subtle ways. Callers who want an "effective"
//! view can apply their own merge with full knowledge of which
//! source produced which value -- the per-layer split makes that
//! attribution possible.
//!
//! # What's in a settings file
//!
//! Field names worth knowing (not exhaustive; survives unknown keys
//! since values are kept as raw JSON):
//!
//! - `env` -- environment variables passed to spawned `claude`
//!   subprocesses. **Often contains secrets** (`ANTHROPIC_API_KEY`,
//!   tokens, etc.). Use [`redact_env_values`] before forwarding to
//!   a less-trusted consumer.
//! - `permissions` -- `{ allow: [...], deny: [...] }` Bash-pattern
//!   and tool-name allow/deny lists.
//! - `hooks` -- map keyed by hook event (`PreToolUse`,
//!   `PostToolUse`, `UserPromptSubmit`, etc.); values are arrays of
//!   shell-command hook configs.
//! - `statusLine` -- statusline command config.
//! - `enabledPlugins` -- map of plugin keys to enabled state.
//! - `enableAllProjectMcpServers`, `enabledMcpjsonServers` -- MCP
//!   server gates.
//!
//! # Example
//!
//! ```no_run
//! use claude_wrapper::settings::SettingsLoader;
//!
//! # fn example() -> claude_wrapper::Result<()> {
//! let layers = SettingsLoader::home()?
//!     .project_root("/path/to/repo")
//!     .load()?;
//!
//! if let Some(user) = &layers.user {
//!     println!("user settings present: {} top-level keys",
//!         user.as_object().map(|o| o.len()).unwrap_or(0));
//! }
//! # Ok(()) }
//! ```

use std::fs;
use std::path::{Path, PathBuf};

use serde::Serialize;
use serde_json::Value;

use crate::error::{Error, Result};

/// One of the four settings layers Claude Code can read.
///
/// Ordering reflects Claude Code's precedence (variants later in
/// the enum take precedence over earlier ones), but this module
/// does not apply that precedence -- it just reports what's on disk.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SettingsLayer {
    /// `~/.claude/settings.json` -- user defaults.
    User,
    /// `~/.claude/settings.local.json` -- user-private overrides.
    UserLocal,
    /// `<project>/.claude/settings.json` -- project-shared.
    Project,
    /// `<project>/.claude/settings.local.json` -- project-private.
    ProjectLocal,
}

impl SettingsLayer {
    /// The filename component of this layer (the part after
    /// `.claude/`). User and project layers share filenames; the
    /// directory determines which root they came from.
    pub fn filename(self) -> &'static str {
        match self {
            Self::User | Self::Project => "settings.json",
            Self::UserLocal | Self::ProjectLocal => "settings.local.json",
        }
    }

    /// All four layers, low-to-high precedence (User → ProjectLocal).
    pub fn all() -> [Self; 4] {
        [
            Self::User,
            Self::UserLocal,
            Self::Project,
            Self::ProjectLocal,
        ]
    }
}

/// Builder/loader for the settings layers.
///
/// Defaults to reading from `~/.claude/`. If
/// [`Self::project_root`] is set, also reads from
/// `<project_root>/.claude/`. Missing files are not errors -- the
/// corresponding [`Settings`] field stays `None`.
#[derive(Debug, Clone)]
pub struct SettingsLoader {
    user_root: PathBuf,
    project_root: Option<PathBuf>,
}

impl SettingsLoader {
    /// Construct with the default user root `~/.claude/` and no
    /// project root. Add a project root with [`Self::project_root`].
    pub fn home() -> Result<Self> {
        let home = home_dir().ok_or_else(|| Error::Artifacts {
            message: "could not determine user home directory".to_string(),
        })?;
        Ok(Self {
            user_root: home.join(".claude"),
            project_root: None,
        })
    }

    /// Construct with explicit roots. Useful for tests (point both
    /// at tempdirs) and non-default installs. `project_root` may be
    /// `None` if no project context applies.
    pub fn at(user_root: impl Into<PathBuf>, project_root: Option<PathBuf>) -> Self {
        Self {
            user_root: user_root.into(),
            project_root,
        }
    }

    /// Set or override the project root. Note this is the `.claude`
    /// *parent* -- i.e. the project directory itself, not
    /// `<project>/.claude`. The loader appends `/.claude` internally.
    #[must_use]
    pub fn project_root(mut self, dir: impl Into<PathBuf>) -> Self {
        self.project_root = Some(dir.into());
        self
    }

    /// The configured user root (typically `~/.claude`).
    pub fn user_root_path(&self) -> &Path {
        &self.user_root
    }

    /// The configured project root, if any.
    pub fn project_root_path(&self) -> Option<&Path> {
        self.project_root.as_deref()
    }

    /// Read all four layers. Missing files become `None`; malformed
    /// JSON files raise an error so callers don't silently lose data.
    pub fn load(&self) -> Result<Settings> {
        Ok(Settings {
            user: read_layer(&self.user_root.join("settings.json"))?,
            user_local: read_layer(&self.user_root.join("settings.local.json"))?,
            project: match &self.project_root {
                Some(root) => read_layer(&root.join(".claude").join("settings.json"))?,
                None => None,
            },
            project_local: match &self.project_root {
                Some(root) => read_layer(&root.join(".claude").join("settings.local.json"))?,
                None => None,
            },
            paths: SettingsPaths {
                user: self.user_root.join("settings.json"),
                user_local: self.user_root.join("settings.local.json"),
                project: self
                    .project_root
                    .as_ref()
                    .map(|r| r.join(".claude").join("settings.json")),
                project_local: self
                    .project_root
                    .as_ref()
                    .map(|r| r.join(".claude").join("settings.local.json")),
            },
        })
    }
}

/// Parsed settings layers as returned by [`SettingsLoader::load`].
///
/// Each field is the raw JSON of one layer, or `None` if the file
/// doesn't exist. No merging is performed -- precedence is a
/// caller-side concern.
#[derive(Debug, Clone, Serialize)]
pub struct Settings {
    /// `<user_root>/settings.json` content, or None if absent.
    pub user: Option<Value>,
    /// `<user_root>/settings.local.json` content, or None if absent.
    pub user_local: Option<Value>,
    /// `<project_root>/.claude/settings.json` content, or None if
    /// the project root wasn't configured or the file doesn't exist.
    pub project: Option<Value>,
    /// `<project_root>/.claude/settings.local.json` content, or
    /// None similarly.
    pub project_local: Option<Value>,
    /// Absolute paths the loader checked, regardless of whether the
    /// files existed. Useful for diagnostics ("where would the
    /// project layer come from?").
    pub paths: SettingsPaths,
}

impl Settings {
    /// Return the loaded value for one layer.
    pub fn get(&self, layer: SettingsLayer) -> Option<&Value> {
        match layer {
            SettingsLayer::User => self.user.as_ref(),
            SettingsLayer::UserLocal => self.user_local.as_ref(),
            SettingsLayer::Project => self.project.as_ref(),
            SettingsLayer::ProjectLocal => self.project_local.as_ref(),
        }
    }
}

/// Absolute paths the loader would read, regardless of which files
/// exist. Returned alongside [`Settings`] so diagnostics can show
/// "the project layer *would* come from `<path>`" even when the file
/// isn't there.
#[derive(Debug, Clone, Serialize)]
pub struct SettingsPaths {
    /// Path to the user settings file (`~/.claude/settings.json`).
    pub user: PathBuf,
    /// Path to the user-local settings file (`settings.local.json`).
    pub user_local: PathBuf,
    /// Path to the project settings file, when a project root is known.
    pub project: Option<PathBuf>,
    /// Path to the project-local settings file, when a project root is known.
    pub project_local: Option<PathBuf>,
}

/// Replace every value under the top-level `env` object with
/// `"<redacted>"`, keeping the keys visible. Safe to call on any
/// JSON value, including non-objects and objects without an `env`
/// field. Other top-level keys (`permissions`, `hooks`, etc.) are
/// left untouched.
///
/// Callers that forward settings to less-trusted consumers should
/// apply this. Hosts that want the raw values must explicitly
/// opt in.
pub fn redact_env_values(value: &mut Value) {
    let Some(obj) = value.as_object_mut() else {
        return;
    };
    let Some(env) = obj.get_mut("env") else {
        return;
    };
    let Some(env_obj) = env.as_object_mut() else {
        return;
    };
    for (_, v) in env_obj.iter_mut() {
        *v = Value::String("<redacted>".to_string());
    }
}

fn read_layer(path: &Path) -> Result<Option<Value>> {
    match fs::read_to_string(path) {
        Ok(raw) => {
            let parsed: Value = serde_json::from_str(&raw).map_err(|e| Error::Artifacts {
                message: format!("settings file {} is not valid JSON: {e}", path.display()),
            })?;
            Ok(Some(parsed))
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e.into()),
    }
}

fn home_dir() -> Option<PathBuf> {
    if let Ok(h) = std::env::var("HOME")
        && !h.is_empty()
    {
        return Some(PathBuf::from(h));
    }
    if let Ok(h) = std::env::var("USERPROFILE")
        && !h.is_empty()
    {
        return Some(PathBuf::from(h));
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn write_layer(path: &Path, value: &Value) {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).expect("mkdir parent");
        }
        fs::write(path, serde_json::to_string_pretty(value).expect("ser")).expect("write");
    }

    #[test]
    fn load_with_only_user_layer_present() {
        let user_root = tempfile::tempdir().expect("tempdir");
        write_layer(
            &user_root.path().join("settings.json"),
            &json!({"env": {"FOO": "bar"}, "permissions": {"allow": ["Bash(ls *)"]}}),
        );
        let loader = SettingsLoader::at(user_root.path(), None);
        let layers = loader.load().expect("load");
        assert!(layers.user.is_some());
        assert!(layers.user_local.is_none());
        assert!(layers.project.is_none());
        assert!(layers.project_local.is_none());
        assert_eq!(
            layers.user.as_ref().unwrap()["env"]["FOO"].as_str(),
            Some("bar")
        );
    }

    #[test]
    fn load_with_all_four_layers() {
        let user_root = tempfile::tempdir().expect("user");
        let project_root = tempfile::tempdir().expect("project");
        write_layer(
            &user_root.path().join("settings.json"),
            &json!({"layer": "user"}),
        );
        write_layer(
            &user_root.path().join("settings.local.json"),
            &json!({"layer": "user_local"}),
        );
        write_layer(
            &project_root.path().join(".claude").join("settings.json"),
            &json!({"layer": "project"}),
        );
        write_layer(
            &project_root
                .path()
                .join(".claude")
                .join("settings.local.json"),
            &json!({"layer": "project_local"}),
        );
        let layers = SettingsLoader::at(user_root.path(), Some(project_root.path().to_path_buf()))
            .load()
            .expect("load");
        assert_eq!(
            layers.user.as_ref().unwrap()["layer"].as_str(),
            Some("user")
        );
        assert_eq!(
            layers.user_local.as_ref().unwrap()["layer"].as_str(),
            Some("user_local")
        );
        assert_eq!(
            layers.project.as_ref().unwrap()["layer"].as_str(),
            Some("project")
        );
        assert_eq!(
            layers.project_local.as_ref().unwrap()["layer"].as_str(),
            Some("project_local")
        );
    }

    #[test]
    fn missing_root_dir_treated_as_empty_not_error() {
        let user_root = tempfile::tempdir().expect("user");
        let layers = SettingsLoader::at(user_root.path(), None)
            .load()
            .expect("load");
        assert!(layers.user.is_none());
        assert!(layers.user_local.is_none());
    }

    #[test]
    fn project_root_unset_means_no_project_layers() {
        let user_root = tempfile::tempdir().expect("user");
        write_layer(&user_root.path().join("settings.json"), &json!({"x": 1}));
        let layers = SettingsLoader::at(user_root.path(), None)
            .load()
            .expect("load");
        assert!(layers.user.is_some());
        assert!(layers.project.is_none());
        assert!(layers.project_local.is_none());
        assert!(layers.paths.project.is_none());
    }

    #[test]
    fn malformed_json_returns_error() {
        let user_root = tempfile::tempdir().expect("user");
        fs::write(user_root.path().join("settings.json"), "{not json").expect("write");
        let err = SettingsLoader::at(user_root.path(), None)
            .load()
            .unwrap_err();
        assert!(err.to_string().contains("not valid JSON"), "got: {err}");
    }

    #[test]
    fn paths_reflect_configured_roots() {
        let user_root = tempfile::tempdir().expect("user");
        let project_root = tempfile::tempdir().expect("project");
        let layers = SettingsLoader::at(user_root.path(), Some(project_root.path().to_path_buf()))
            .load()
            .expect("load");
        assert_eq!(layers.paths.user, user_root.path().join("settings.json"));
        assert_eq!(
            layers.paths.project,
            Some(project_root.path().join(".claude").join("settings.json"))
        );
    }

    #[test]
    fn get_indexes_by_layer() {
        let user_root = tempfile::tempdir().expect("user");
        write_layer(
            &user_root.path().join("settings.json"),
            &json!({"k": "user"}),
        );
        write_layer(
            &user_root.path().join("settings.local.json"),
            &json!({"k": "user_local"}),
        );
        let layers = SettingsLoader::at(user_root.path(), None)
            .load()
            .expect("load");
        assert_eq!(
            layers.get(SettingsLayer::User).unwrap()["k"].as_str(),
            Some("user")
        );
        assert_eq!(
            layers.get(SettingsLayer::UserLocal).unwrap()["k"].as_str(),
            Some("user_local")
        );
        assert!(layers.get(SettingsLayer::Project).is_none());
    }

    // -- env redaction ------------------------------------------------

    #[test]
    fn redact_env_replaces_values_keeps_keys() {
        let mut v = json!({
            "env": {"ANTHROPIC_API_KEY": "sk-xxx", "DEBUG": "1"},
            "permissions": {"allow": ["Bash(ls *)"]},
        });
        redact_env_values(&mut v);
        assert_eq!(v["env"]["ANTHROPIC_API_KEY"].as_str(), Some("<redacted>"));
        assert_eq!(v["env"]["DEBUG"].as_str(), Some("<redacted>"));
        // Permissions untouched.
        assert_eq!(v["permissions"]["allow"][0].as_str(), Some("Bash(ls *)"));
    }

    #[test]
    fn redact_env_noop_when_no_env_field() {
        let mut v = json!({"permissions": {"allow": ["Bash(ls *)"]}});
        let before = v.clone();
        redact_env_values(&mut v);
        assert_eq!(v, before);
    }

    #[test]
    fn redact_env_noop_on_non_object_root() {
        let mut v = json!(["not", "an", "object"]);
        let before = v.clone();
        redact_env_values(&mut v);
        assert_eq!(v, before);
    }

    #[test]
    fn redact_env_noop_when_env_not_object() {
        let mut v = json!({"env": "weird-but-tolerated"});
        let before = v.clone();
        redact_env_values(&mut v);
        assert_eq!(v, before);
    }
}
