//! Read-side access to Claude Code's on-disk **skill** definitions.
//!
//! Claude Code resolves user-level skills from
//! `~/.claude/skills/<name>/SKILL.md`. Unlike agents (which are flat
//! `.md` files), each skill is a *directory* containing a `SKILL.md`
//! plus optional bundled assets (`scripts/`, `reference/`, etc.).
//! The frontmatter on `SKILL.md` carries the skill's metadata (name,
//! description); the body is the skill's instructions.
//!
//! This module is read-only on purpose -- mutations (create / update
//! / delete) are deferred. Creating a skill is more involved than a
//! file write because it implies directory layout and optional
//! scaffold assets.
//!
//! Two levels of granularity:
//!
//! - [`SkillsRoot::list`] -- enumerate every skill at the root with
//!   summary metadata (name, description, dir path, has_assets).
//! - [`SkillsRoot::get`] -- read one skill's full record including
//!   the instructions body.
//!
//! # Frontmatter format
//!
//! Real-world skills look like:
//!
//! ```text
//! ---
//! name: recall
//! description: Search mente for memories by topic, text, tags, or ranked search
//! ---
//!
//! # Search mente for memories
//! ...
//! ```
//!
//! The parser is permissive: only `name` and `description` are typed.
//! Any other `key: value` pairs land in [`Skill::extra`] so unknown
//! future keys survive a round trip. Frontmatter is optional -- a
//! body-only `SKILL.md` parses fine, with `name` defaulting to the
//! directory stem.
//!
//! YAML block scalars (`>`, `>-`, `>+`, `|`, `|-`, `|+`) are
//! supported for any key, which is how multi-line descriptions are
//! usually written. `>` folds the block into one line, `|` preserves
//! the line breaks, and the chomping indicator controls the trailing
//! newline. Continuation lines are part of the value even when they
//! contain a colon.
//!
//! # Example
//!
//! ```no_run
//! use claude_wrapper::skills::SkillsRoot;
//!
//! # fn example() -> claude_wrapper::Result<()> {
//! let root = SkillsRoot::home()?;
//! for summary in root.list()? {
//!     println!("{}: {}", summary.name, summary.description.as_deref().unwrap_or(""));
//! }
//! let skill = root.get("recall")?;
//! println!("{}", skill.body);
//! # Ok(()) }
//! ```
//!
//! # Stem, name, directory
//!
//! By convention a skill's `name` matches its directory name:
//! `~/.claude/skills/recall/SKILL.md` carries `name: recall`. The
//! two can diverge -- the parser keeps both. [`SkillsRoot::get`]
//! looks up by directory stem (because that's what the filesystem
//! indexes), not by the frontmatter `name`.
//!
//! # Pointing at a different root
//!
//! The default is `~/.claude/skills`. Pass an explicit path to
//! [`SkillsRoot::at`] to point at a different directory -- a tempdir
//! in tests, a non-default Claude Code install. The on-disk layout
//! (`<root>/<stem>/SKILL.md`) is the same regardless of root.
//! [`SkillsRoot::scheduled_tasks_home`] points the same reader at
//! `~/.claude/scheduled-tasks`, whose entries share the SKILL.md
//! format.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::artifacts::{frontmatter_entries, split_frontmatter};
use crate::error::{Error, Result};

/// Root directory of Claude Code's user-level skill definitions.
/// Defaults to `~/.claude/skills`; override with [`SkillsRoot::at`]
/// for tests or non-default installs.
#[derive(Debug, Clone)]
pub struct SkillsRoot {
    path: PathBuf,
}

impl SkillsRoot {
    /// Resolve the default `~/.claude/skills`. Errors if `$HOME`
    /// (or the platform-specific user home) cannot be determined.
    pub fn home() -> Result<Self> {
        let home = home_dir().ok_or_else(|| Error::Artifacts {
            message: "could not determine user home directory".to_string(),
        })?;
        Ok(Self {
            path: home.join(".claude").join("skills"),
        })
    }

    /// Use a specific path as the skills root. Useful for tests
    /// (point at a tempdir) and for non-default installs.
    pub fn at(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    /// Resolve `~/.claude/scheduled-tasks` as the root.
    ///
    /// Scheduled-task definitions use the same on-disk shape as
    /// skills (`<name>/SKILL.md` with `name` / `description`
    /// frontmatter and a prompt body), so the skills reader serves
    /// them with a different root instead of a duplicate module.
    /// Scheduling metadata (cron expression, enablement) is NOT in
    /// these files; only the definition is exposed here. The layout
    /// is undocumented Claude Code internal state (observed against
    /// CLI 2.1.219).
    pub fn scheduled_tasks_home() -> Result<Self> {
        let home = home_dir().ok_or_else(|| Error::Artifacts {
            message: "could not determine user home directory".to_string(),
        })?;
        Ok(Self {
            path: home.join(".claude").join("scheduled-tasks"),
        })
    }

    /// The configured root directory.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// List every skill directory at the root, sorted by directory
    /// stem.
    ///
    /// A "skill" is any direct child directory of the root that
    /// contains a `SKILL.md`. Directories without `SKILL.md` and
    /// non-directory entries are ignored. Returns an empty vec if
    /// the root itself doesn't exist (a fresh Claude Code install
    /// with no user skills). Directories whose `SKILL.md` fails to
    /// parse contribute a tracing warning and are skipped rather
    /// than failing the whole listing.
    pub fn list(&self) -> Result<Vec<SkillSummary>> {
        let entries = match fs::read_dir(&self.path) {
            Ok(it) => it,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(e) => return Err(e.into()),
        };

        let mut out = Vec::new();
        for entry in entries.flatten() {
            let dir = entry.path();
            if !dir.is_dir() {
                continue;
            }
            let stem = match dir.file_name().and_then(|s| s.to_str()) {
                Some(s) => s.to_string(),
                None => continue,
            };
            let skill_md = dir.join("SKILL.md");
            if !skill_md.is_file() {
                continue;
            }
            match parse_skill_file(&skill_md, &dir, &stem) {
                Ok(skill) => out.push(SkillSummary::from_skill(&skill)),
                Err(e) => tracing::warn!(?skill_md, "skipping skill: {e}"),
            }
        }
        out.sort_by(|a, b| a.dir_stem.cmp(&b.dir_stem));
        Ok(out)
    }

    /// Read one skill by directory stem (i.e. the basename of the
    /// `<stem>/` directory under the root). Errors if no such
    /// directory exists, it has no `SKILL.md`, or the file fails to
    /// parse.
    pub fn get(&self, dir_stem: &str) -> Result<Skill> {
        let dir = self.path.join(dir_stem);
        let skill_md = dir.join("SKILL.md");
        if !skill_md.is_file() {
            return Err(Error::Artifacts {
                message: format!("no skill at {}", dir.display()),
            });
        }
        parse_skill_file(&skill_md, &dir, dir_stem)
    }
}

/// Lightweight metadata for one skill, returned by
/// [`SkillsRoot::list`]. Strips the body to keep listings cheap.
#[derive(Debug, Clone, Serialize)]
pub struct SkillSummary {
    /// Directory stem (the basename of `<stem>/` under the root).
    /// The canonical handle for lookup.
    pub dir_stem: String,
    /// Frontmatter `name` if present; falls back to `dir_stem`.
    pub name: String,
    /// Frontmatter `description` if present.
    pub description: Option<String>,
    /// Absolute path to the skill's directory.
    pub dir_path: PathBuf,
    /// Absolute path to the source `SKILL.md`.
    pub file_path: PathBuf,
    /// `SKILL.md` size in bytes; useful for cheap UI hints.
    pub size_bytes: u64,
    /// True if the skill directory contains sibling files or
    /// subdirectories beyond `SKILL.md` (e.g. `scripts/`,
    /// `reference/`). Listing the sibling paths themselves is
    /// deferred; callers that need the inventory can stat the
    /// directory directly via [`Self::dir_path`].
    pub has_assets: bool,
}

impl SkillSummary {
    fn from_skill(s: &Skill) -> Self {
        let size_bytes = fs::metadata(&s.file_path)
            .map(|m| m.len())
            .unwrap_or_default();
        Self {
            dir_stem: s.dir_stem.clone(),
            name: s.name.clone(),
            description: s.description.clone(),
            dir_path: s.dir_path.clone(),
            file_path: s.file_path.clone(),
            size_bytes,
            has_assets: s.has_assets,
        }
    }
}

/// Full skill record returned by [`SkillsRoot::get`].
#[derive(Debug, Clone, Serialize)]
pub struct Skill {
    /// Directory stem (the basename of `<stem>/` under the root).
    /// The canonical handle for lookup.
    pub dir_stem: String,
    /// Frontmatter `name` if present; falls back to `dir_stem`.
    pub name: String,
    /// Frontmatter `description` if present.
    pub description: Option<String>,
    /// Absolute path to the skill's directory.
    pub dir_path: PathBuf,
    /// Absolute path to the source `SKILL.md`.
    pub file_path: PathBuf,
    /// Markdown body after the frontmatter block (trimmed of
    /// leading/trailing blank lines).
    pub body: String,
    /// Frontmatter keys other than the typed ones. Preserves
    /// unknown future fields verbatim as raw strings.
    pub extra: BTreeMap<String, String>,
    /// True if the skill directory contains sibling files or
    /// subdirectories beyond `SKILL.md`. See
    /// [`SkillSummary::has_assets`] for the deferred-inventory
    /// rationale.
    pub has_assets: bool,
}

fn parse_skill_file(file_path: &Path, dir_path: &Path, dir_stem: &str) -> Result<Skill> {
    let raw = fs::read_to_string(file_path)?;
    let (frontmatter, body) = split_frontmatter(&raw);

    let mut name = dir_stem.to_string();
    let mut description = None;
    let mut extra = BTreeMap::new();

    if let Some(fm) = frontmatter {
        for (key, value) in frontmatter_entries(fm) {
            match key.as_str() {
                "name" if !value.is_empty() => name = value,
                "description" if !value.is_empty() => description = Some(value),
                _ => {
                    extra.insert(key, value);
                }
            }
        }
    }

    Ok(Skill {
        dir_stem: dir_stem.to_string(),
        name,
        description,
        dir_path: dir_path.to_path_buf(),
        file_path: file_path.to_path_buf(),
        body: body.trim().to_string(),
        extra,
        has_assets: directory_has_assets(dir_path),
    })
}

fn directory_has_assets(dir: &Path) -> bool {
    let entries = match fs::read_dir(dir) {
        Ok(it) => it,
        Err(_) => return false,
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        // Skip the canonical SKILL.md itself; anything else is an asset.
        if name == "SKILL.md" {
            continue;
        }
        return true;
    }
    false
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

    fn write_skill(root: &Path, stem: &str, contents: &str) -> PathBuf {
        let dir = root.join(stem);
        fs::create_dir_all(&dir).expect("create skill dir");
        let path = dir.join("SKILL.md");
        let mut f = fs::File::create(&path).expect("create SKILL.md");
        f.write_all(contents.as_bytes()).expect("write SKILL.md");
        path
    }

    fn fixture_root() -> tempfile::TempDir {
        let tmp = tempfile::tempdir().expect("tempdir");
        write_skill(
            tmp.path(),
            "recall",
            "---\nname: recall\ndescription: Search mente for memories\n---\n\nSearch for: $ARGUMENTS\n",
        );
        write_skill(
            tmp.path(),
            "no-frontmatter",
            "Just a body, no frontmatter at all.\n",
        );
        write_skill(
            tmp.path(),
            "weird",
            "---\nname: weird\ndescription: has extras\ncustom_key: custom_value\n---\nbody\n",
        );
        // A skill with bundled assets (scripts/).
        write_skill(
            tmp.path(),
            "bundled",
            "---\nname: bundled\ndescription: has scripts\n---\nbody\n",
        );
        let scripts = tmp.path().join("bundled").join("scripts");
        fs::create_dir_all(&scripts).expect("create scripts dir");
        fs::write(scripts.join("helper.sh"), "#!/bin/sh\n").expect("write helper");
        // A directory without SKILL.md should be ignored.
        let bogus = tmp.path().join("not-a-skill");
        fs::create_dir_all(&bogus).expect("create bogus");
        fs::write(bogus.join("README.md"), "not a skill").expect("write README");
        // A non-directory entry at the root should be ignored.
        fs::write(tmp.path().join("loose-file.md"), "ignore me").expect("write loose");
        tmp
    }

    #[test]
    fn list_returns_only_skill_dirs_sorted() {
        let tmp = fixture_root();
        let root = SkillsRoot::at(tmp.path());
        let skills = root.list().expect("list");
        let stems: Vec<&str> = skills.iter().map(|s| s.dir_stem.as_str()).collect();
        assert_eq!(stems, ["bundled", "no-frontmatter", "recall", "weird"]);
    }

    #[test]
    fn list_missing_root_returns_empty() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let root = SkillsRoot::at(tmp.path().join("does-not-exist"));
        let skills = root.list().expect("list");
        assert!(skills.is_empty());
    }

    #[test]
    fn list_typed_metadata() {
        let tmp = fixture_root();
        let root = SkillsRoot::at(tmp.path());
        let skills = root.list().expect("list");
        let recall = skills
            .iter()
            .find(|s| s.dir_stem == "recall")
            .expect("recall");
        assert_eq!(recall.name, "recall");
        assert_eq!(
            recall.description.as_deref(),
            Some("Search mente for memories")
        );
        assert!(recall.size_bytes > 0);
        assert!(!recall.has_assets);
    }

    #[test]
    fn list_detects_bundled_assets() {
        let tmp = fixture_root();
        let root = SkillsRoot::at(tmp.path());
        let skills = root.list().expect("list");
        let bundled = skills
            .iter()
            .find(|s| s.dir_stem == "bundled")
            .expect("bundled");
        assert!(bundled.has_assets, "expected has_assets=true for bundled");
    }

    #[test]
    fn list_no_frontmatter_falls_back_to_stem() {
        let tmp = fixture_root();
        let root = SkillsRoot::at(tmp.path());
        let skills = root.list().expect("list");
        let nf = skills
            .iter()
            .find(|s| s.dir_stem == "no-frontmatter")
            .expect("no-frontmatter");
        assert_eq!(nf.name, "no-frontmatter");
        assert_eq!(nf.description, None);
    }

    #[test]
    fn get_returns_full_skill_with_body() {
        let tmp = fixture_root();
        let root = SkillsRoot::at(tmp.path());
        let skill = root.get("recall").expect("get recall");
        assert_eq!(skill.name, "recall");
        assert_eq!(skill.body, "Search for: $ARGUMENTS");
        assert!(!skill.has_assets);
    }

    #[test]
    fn get_no_frontmatter_returns_full_body() {
        let tmp = fixture_root();
        let root = SkillsRoot::at(tmp.path());
        let skill = root.get("no-frontmatter").expect("get");
        assert_eq!(skill.body, "Just a body, no frontmatter at all.");
        assert_eq!(skill.name, "no-frontmatter");
    }

    #[test]
    fn get_unknown_id_errors() {
        let tmp = fixture_root();
        let root = SkillsRoot::at(tmp.path());
        let err = root.get("nope").unwrap_err();
        assert!(err.to_string().to_lowercase().contains("no skill"));
    }

    #[test]
    fn extra_keys_round_trip_as_strings() {
        let tmp = fixture_root();
        let root = SkillsRoot::at(tmp.path());
        let skill = root.get("weird").expect("get weird");
        assert_eq!(
            skill.extra.get("custom_key").map(String::as_str),
            Some("custom_value")
        );
    }

    #[test]
    fn folded_description_with_colons_is_one_value() {
        let tmp = tempfile::tempdir().expect("tempdir");
        write_skill(
            tmp.path(),
            "folded",
            concat!(
                "---\n",
                "name: folded\n",
                "description: >-\n",
                "  Use when surveying a codebase against a rubric. Read-only: never\n",
                "  edits files, opens PRs, or commits.\n",
                "---\n\nBody.\n",
            ),
        );
        let root = SkillsRoot::at(tmp.path());
        let skill = root.get("folded").expect("get");
        assert_eq!(
            skill.description.as_deref(),
            Some(
                "Use when surveying a codebase against a rubric. Read-only: never \
                 edits files, opens PRs, or commits."
            )
        );
        assert!(skill.extra.is_empty(), "extra: {:?}", skill.extra);
        assert_eq!(skill.body, "Body.");
    }

    #[test]
    fn literal_description_preserves_newlines() {
        let tmp = tempfile::tempdir().expect("tempdir");
        write_skill(
            tmp.path(),
            "lit",
            "---\nname: lit\ndescription: |-\n  one\n  two: three\n---\nbody\n",
        );
        let root = SkillsRoot::at(tmp.path());
        let skill = root.get("lit").expect("get");
        assert_eq!(skill.description.as_deref(), Some("one\ntwo: three"));
    }

    #[test]
    fn empty_value_keys_dont_overwrite_defaults() {
        let tmp = tempfile::tempdir().expect("tempdir");
        write_skill(
            tmp.path(),
            "empty-name",
            "---\nname:\ndescription: keeps stem as name\n---\nbody\n",
        );
        let root = SkillsRoot::at(tmp.path());
        let skill = root.get("empty-name").expect("get");
        assert_eq!(skill.name, "empty-name");
    }

    #[test]
    fn scheduled_tasks_home_points_at_scheduled_tasks() {
        if let Ok(root) = SkillsRoot::scheduled_tasks_home() {
            assert!(root.path().ends_with(".claude/scheduled-tasks"));
        }
    }

    #[test]
    fn list_ignores_dirs_without_skill_md() {
        let tmp = fixture_root();
        // Fixture has `not-a-skill/` with only a README; it must be skipped.
        let root = SkillsRoot::at(tmp.path());
        let skills = root.list().expect("list");
        assert!(!skills.iter().any(|s| s.dir_stem == "not-a-skill"));
    }
}
