//! Read-side access to Claude Code's on-disk **custom slash command**
//! definitions.
//!
//! Claude Code resolves custom slash commands from `*.md` files at:
//!
//! - `~/.claude/commands/<name>.md` -- user-level
//! - `<project>/.claude/commands/<name>.md` -- project-level
//!
//! Plus plugin-provided commands under
//! `~/.claude/plugins/<plugin>/commands/`, which this module does
//! not enumerate (the plugin feature surfaces those separately).
//!
//! # What's NOT covered
//!
//! **Built-in slash commands** like `/help`, `/clear`, `/config`,
//! `/init` are hardcoded in the `claude` binary. They have no disk
//! representation, are not listed by `claude --help`, and aren't
//! introspectable from the CLI. Consumers learn about them from
//! Claude Code's own UX. We deliberately do not attempt to mirror
//! a built-in list here -- doing so would silently rot every time
//! Claude Code adds or renames one.
//!
//! **Skills** also surface as slash commands (`/recall`,
//! `/draft-pr-first`, etc.) but they're a separate on-disk artifact
//! type and are not loaded by this module. A consumer that wants
//! "the full slash command universe at this moment" combines this
//! list with a separate skills enumeration (and accepts that
//! built-ins aren't represented).
//!
//! # Two levels of granularity
//!
//! - [`CommandsRoot::list`] -- enumerate every `*.md` command at
//!   the root with summary metadata.
//! - [`CommandsRoot::get`] -- read one command's full record
//!   including the prompt body and any unknown frontmatter keys.
//!
//! # Frontmatter format
//!
//! Real-world commands look like:
//!
//! ```text
//! ---
//! description: Open a PR for the current branch
//! argument-hint: <pr title>
//! allowed-tools: Bash(git *), Bash(gh *)
//! model: sonnet
//! ---
//!
//! Open a pull request titled "$ARGUMENTS" ...
//! ```
//!
//! The parser is permissive: only `description`, `argument-hint`,
//! `allowed-tools`, `model`, and `disable-model-invocation` are
//! typed. Any other `key: value` pairs land in [`Command::extra`].
//! Frontmatter is optional -- a body-only file parses fine, with
//! `description` left `None`.
//!
//! Note the dashes in `argument-hint` / `allowed-tools` /
//! `disable-model-invocation`: that's how Claude Code spells the
//! keys on disk. The typed fields use Rust-friendly snake_case
//! names.
//!
//! # Example
//!
//! ```no_run
//! use claude_wrapper::commands::CommandsRoot;
//!
//! # fn example() -> claude_wrapper::Result<()> {
//! let root = CommandsRoot::user()?;
//! for summary in root.list()? {
//!     println!("/{}: {}", summary.file_stem,
//!         summary.description.as_deref().unwrap_or(""));
//! }
//! # Ok(()) }
//! ```

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::artifacts::split_frontmatter;
use crate::error::{Error, Result};

/// Root directory of one set of slash command definitions
/// (`<root>/<stem>.md`). Use [`Self::user`] for the user-level root
/// at `~/.claude/commands`, [`Self::project`] for a project's
/// `<dir>/.claude/commands`, or [`Self::at`] to point at an
/// arbitrary directory for tests.
#[derive(Debug, Clone)]
pub struct CommandsRoot {
    path: PathBuf,
}

impl CommandsRoot {
    /// Resolve the user-level commands root at `~/.claude/commands`.
    /// Errors if `$HOME` cannot be determined.
    pub fn user() -> Result<Self> {
        let home = home_dir().ok_or_else(|| Error::Artifacts {
            message: "could not determine user home directory".to_string(),
        })?;
        Ok(Self {
            path: home.join(".claude").join("commands"),
        })
    }

    /// Resolve a project-level commands root at
    /// `<project_dir>/.claude/commands`. The `project_dir` is the
    /// project root itself (the `.claude/commands` suffix is
    /// appended internally).
    pub fn project(project_dir: impl Into<PathBuf>) -> Self {
        let mut p: PathBuf = project_dir.into();
        p.push(".claude");
        p.push("commands");
        Self { path: p }
    }

    /// Use a specific path as the commands root. Useful for tests.
    pub fn at(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    /// The configured root directory.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// List every `*.md` command at the root, sorted by file stem.
    ///
    /// Returns an empty vec if the root directory doesn't exist (a
    /// project or user without custom commands). Files that fail
    /// to parse contribute a tracing warning and are skipped.
    pub fn list(&self) -> Result<Vec<CommandSummary>> {
        let entries = match fs::read_dir(&self.path) {
            Ok(it) => it,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(e) => return Err(e.into()),
        };

        let mut out = Vec::new();
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|s| s.to_str()) != Some("md") {
                continue;
            }
            let stem = match path.file_stem().and_then(|s| s.to_str()) {
                Some(s) => s.to_string(),
                None => continue,
            };
            match parse_command_file(&path, &stem) {
                Ok(cmd) => out.push(CommandSummary::from_command(&cmd)),
                Err(e) => tracing::warn!(?path, "skipping command: {e}"),
            }
        }
        out.sort_by(|a, b| a.file_stem.cmp(&b.file_stem));
        Ok(out)
    }

    /// Read one command by file stem. Errors if no such file exists
    /// or it fails to parse.
    pub fn get(&self, file_stem: &str) -> Result<Command> {
        let path = self.path.join(format!("{file_stem}.md"));
        if !path.exists() {
            return Err(Error::Artifacts {
                message: format!("no command at {}", path.display()),
            });
        }
        parse_command_file(&path, file_stem)
    }
}

/// Lightweight metadata for one slash command, returned by
/// [`CommandsRoot::list`].
#[derive(Debug, Clone, Serialize)]
pub struct CommandSummary {
    /// Filename stem (`<stem>.md`). This is the slash command name --
    /// `/<stem>` is what users type.
    pub file_stem: String,
    /// Frontmatter `description` if present.
    pub description: Option<String>,
    /// Frontmatter `argument-hint` if present -- placeholder text
    /// shown next to `$ARGUMENTS` in the UI.
    pub argument_hint: Option<String>,
    /// Frontmatter `allowed-tools` parsed as a comma-separated list.
    /// Empty when absent.
    pub allowed_tools: Vec<String>,
    /// Frontmatter `model` if present (model override for this
    /// command).
    pub model: Option<String>,
    /// Frontmatter `disable-model-invocation` if present and `true`.
    /// `None` when absent.
    pub disable_model_invocation: Option<bool>,
    /// Absolute path to the source file.
    pub file_path: PathBuf,
    /// File size in bytes; useful for cheap UI hints.
    pub size_bytes: u64,
}

impl CommandSummary {
    fn from_command(c: &Command) -> Self {
        let size_bytes = fs::metadata(&c.file_path)
            .map(|m| m.len())
            .unwrap_or_default();
        Self {
            file_stem: c.file_stem.clone(),
            description: c.description.clone(),
            argument_hint: c.argument_hint.clone(),
            allowed_tools: c.allowed_tools.clone(),
            model: c.model.clone(),
            disable_model_invocation: c.disable_model_invocation,
            file_path: c.file_path.clone(),
            size_bytes,
        }
    }
}

/// Full command record returned by [`CommandsRoot::get`].
#[derive(Debug, Clone, Serialize)]
pub struct Command {
    /// Filename stem (`<stem>.md`). The slash command name.
    pub file_stem: String,
    /// Frontmatter `description` if present.
    pub description: Option<String>,
    /// Frontmatter `argument-hint` if present.
    pub argument_hint: Option<String>,
    /// Frontmatter `allowed-tools` parsed as a comma-separated list.
    pub allowed_tools: Vec<String>,
    /// Frontmatter `model` if present.
    pub model: Option<String>,
    /// Frontmatter `disable-model-invocation` if present.
    pub disable_model_invocation: Option<bool>,
    /// Absolute path to the source file.
    pub file_path: PathBuf,
    /// Markdown body after the frontmatter block (trimmed). The
    /// prompt template; supports `$ARGUMENTS` substitution.
    pub body: String,
    /// Frontmatter keys other than the typed ones. Preserves
    /// unknown future fields verbatim as raw strings.
    pub extra: BTreeMap<String, String>,
}

fn parse_command_file(path: &Path, file_stem: &str) -> Result<Command> {
    let raw = fs::read_to_string(path)?;
    let (frontmatter, body) = split_frontmatter(&raw);

    let mut description = None;
    let mut argument_hint = None;
    let mut allowed_tools = Vec::new();
    let mut model = None;
    let mut disable_model_invocation = None;
    let mut extra = BTreeMap::new();

    if let Some(fm) = frontmatter {
        for line in fm.lines() {
            let trimmed = line.trim();
            if trimmed.is_empty() {
                continue;
            }
            let Some((k, v)) = trimmed.split_once(':') else {
                continue;
            };
            let key = k.trim();
            let value = v.trim().to_string();
            match key {
                "description" if !value.is_empty() => description = Some(value),
                "argument-hint" if !value.is_empty() => argument_hint = Some(value),
                "allowed-tools" if !value.is_empty() => {
                    allowed_tools = value
                        .split(',')
                        .map(|t| t.trim().to_string())
                        .filter(|t| !t.is_empty())
                        .collect();
                }
                "model" if !value.is_empty() => model = Some(value),
                "disable-model-invocation" if !value.is_empty() => {
                    disable_model_invocation = Some(matches!(
                        value.to_ascii_lowercase().as_str(),
                        "true" | "yes" | "1"
                    ));
                }
                _ if !key.is_empty() => {
                    extra.insert(key.to_string(), value);
                }
                _ => {}
            }
        }
    }

    Ok(Command {
        file_stem: file_stem.to_string(),
        description,
        argument_hint,
        allowed_tools,
        model,
        disable_model_invocation,
        file_path: path.to_path_buf(),
        body: body.trim().to_string(),
        extra,
    })
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
    use std::io::Write;

    fn write_command(dir: &Path, file_stem: &str, contents: &str) -> PathBuf {
        let path = dir.join(format!("{file_stem}.md"));
        let mut f = fs::File::create(&path).expect("create md");
        f.write_all(contents.as_bytes()).expect("write md");
        path
    }

    fn fixture_root() -> tempfile::TempDir {
        let tmp = tempfile::tempdir().expect("tempdir");
        write_command(
            tmp.path(),
            "open-pr",
            "---\ndescription: Open a PR for the current branch\nargument-hint: <pr title>\nallowed-tools: Bash(git *), Bash(gh *)\nmodel: sonnet\n---\n\nOpen a pull request titled \"$ARGUMENTS\".\n",
        );
        write_command(
            tmp.path(),
            "no-frontmatter",
            "Just a body, no frontmatter at all.\n",
        );
        write_command(
            tmp.path(),
            "weird",
            "---\ndescription: has extras\ncustom_key: custom_value\ndisable-model-invocation: true\n---\nbody\n",
        );
        // Non-md file ignored.
        fs::write(tmp.path().join("README.txt"), "ignore").expect("write txt");
        tmp
    }

    #[test]
    fn list_returns_only_md_files_sorted() {
        let tmp = fixture_root();
        let root = CommandsRoot::at(tmp.path());
        let cmds = root.list().expect("list");
        let stems: Vec<&str> = cmds.iter().map(|c| c.file_stem.as_str()).collect();
        assert_eq!(stems, ["no-frontmatter", "open-pr", "weird"]);
    }

    #[test]
    fn list_missing_root_returns_empty() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let root = CommandsRoot::at(tmp.path().join("does-not-exist"));
        assert!(root.list().expect("list").is_empty());
    }

    #[test]
    fn list_typed_metadata() {
        let tmp = fixture_root();
        let root = CommandsRoot::at(tmp.path());
        let cmds = root.list().expect("list");
        let pr = cmds.iter().find(|c| c.file_stem == "open-pr").unwrap();
        assert_eq!(
            pr.description.as_deref(),
            Some("Open a PR for the current branch")
        );
        assert_eq!(pr.argument_hint.as_deref(), Some("<pr title>"));
        assert_eq!(pr.allowed_tools, vec!["Bash(git *)", "Bash(gh *)"]);
        assert_eq!(pr.model.as_deref(), Some("sonnet"));
        assert!(pr.disable_model_invocation.is_none());
        assert!(pr.size_bytes > 0);
    }

    #[test]
    fn list_no_frontmatter_parses_clean() {
        let tmp = fixture_root();
        let root = CommandsRoot::at(tmp.path());
        let cmds = root.list().expect("list");
        let nf = cmds
            .iter()
            .find(|c| c.file_stem == "no-frontmatter")
            .unwrap();
        assert!(nf.description.is_none());
        assert!(nf.allowed_tools.is_empty());
    }

    #[test]
    fn get_returns_full_command_with_body() {
        let tmp = fixture_root();
        let root = CommandsRoot::at(tmp.path());
        let cmd = root.get("open-pr").expect("get");
        assert_eq!(cmd.file_stem, "open-pr");
        assert!(cmd.body.starts_with("Open a pull request"));
    }

    #[test]
    fn get_no_frontmatter_returns_full_body() {
        let tmp = fixture_root();
        let root = CommandsRoot::at(tmp.path());
        let cmd = root.get("no-frontmatter").expect("get");
        assert_eq!(cmd.body, "Just a body, no frontmatter at all.");
    }

    #[test]
    fn get_unknown_id_errors() {
        let tmp = fixture_root();
        let root = CommandsRoot::at(tmp.path());
        let err = root.get("nope").unwrap_err();
        assert!(err.to_string().to_lowercase().contains("no command"));
    }

    #[test]
    fn extras_round_trip() {
        let tmp = fixture_root();
        let root = CommandsRoot::at(tmp.path());
        let cmd = root.get("weird").expect("get");
        assert_eq!(
            cmd.extra.get("custom_key").map(String::as_str),
            Some("custom_value")
        );
    }

    #[test]
    fn disable_model_invocation_parses_bool() {
        let tmp = fixture_root();
        let root = CommandsRoot::at(tmp.path());
        let cmd = root.get("weird").expect("get");
        assert_eq!(cmd.disable_model_invocation, Some(true));
    }

    #[test]
    fn project_helper_appends_dot_claude_commands() {
        let p = CommandsRoot::project("/tmp/repo");
        assert!(p.path().ends_with(".claude/commands"));
        assert!(p.path().starts_with("/tmp/repo"));
    }
}
