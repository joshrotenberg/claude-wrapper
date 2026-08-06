//! Read-side access to Claude Code's on-disk **agent** definitions.
//!
//! Claude Code resolves user-level agents from
//! `~/.claude/agents/<name>.md`. Each file is plain markdown with a
//! YAML-style frontmatter block delimited by `---` lines. The
//! frontmatter carries the agent's metadata (name, description,
//! optional tool allow-list, optional model); the body is the agent's
//! system prompt.
//!
//! This module is read-only on purpose -- mutations (create / update
//! / delete) are tracked separately so consumers that only want to
//! introspect the agent set don't need to opt into write semantics.
//!
//! Two levels of granularity:
//!
//! - [`AgentsRoot::list`] -- enumerate every agent at the root with
//!   summary metadata (name, description, tools, model, file path).
//! - [`AgentsRoot::get`] -- read one agent's full record including
//!   the prompt body.
//!
//! # Frontmatter format
//!
//! Real-world agents look like:
//!
//! ```text
//! ---
//! name: rust-qa
//! description: Use PROACTIVELY before declaring Rust work done...
//! tools: Read, Grep, Glob, Bash
//! model: sonnet
//! ---
//!
//! You are a Rust quality gate. ...
//! ```
//!
//! The parser is permissive: only `name`, `description`, `tools`,
//! `model`, and `skills` are typed. `tools` is a comma-separated
//! list; `skills` is usually a YAML block sequence:
//!
//! ```text
//! skills:
//!   - sandbox-preflight
//!   - durable-context
//! ```
//!
//! Any other `key: value` pairs land in [`Agent::extra`] so unknown
//! future keys survive a round trip. Frontmatter is optional -- a
//! body-only file parses fine, with `name` defaulting to the file
//! stem.
//!
//! Sequences under keys other than `skills` reach [`Agent::extra`]
//! joined by `", "`, since `extra` holds raw strings. Writing such an
//! agent back out renders them comma-joined on one line rather than
//! as a block sequence.
//!
//! YAML block scalars are supported for any key, which is how
//! multi-line descriptions are usually written:
//!
//! ```text
//! ---
//! name: auditor
//! description: >-
//!   Use when surveying a codebase against a rubric. Read-only:
//!   never edits files, opens PRs, or commits.
//! ---
//! ```
//!
//! `>` folds the block into one line (blank lines become newlines),
//! `|` preserves the line breaks, and the chomping indicator
//! (`-` / none / `+`) controls the trailing newline. Continuation
//! lines are part of the value even when they contain a colon.
//!
//! # Example
//!
//! ```no_run
//! use claude_wrapper::artifacts::AgentsRoot;
//!
//! # fn example() -> claude_wrapper::Result<()> {
//! let root = AgentsRoot::home()?;
//! for summary in root.list()? {
//!     println!("{}: {}", summary.name, summary.description.as_deref().unwrap_or(""));
//! }
//! let agent = root.get("rust-qa")?;
//! println!("{}", agent.body);
//! # Ok(()) }
//! ```
//!
//! # Slug, name, file stem
//!
//! By convention an agent's `name` matches its filename stem:
//! `rust-qa.md` carries `name: rust-qa`. The two can diverge -- the
//! parser keeps both. [`AgentsRoot::get`] looks up by file stem
//! (because that's what the filesystem indexes), not by the
//! frontmatter `name`.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::error::{Error, Result};

/// Root directory of Claude Code's user-level agent definitions.
/// Defaults to `~/.claude/agents`; override with [`AgentsRoot::at`]
/// for tests or non-default installs.
#[derive(Debug, Clone)]
pub struct AgentsRoot {
    path: PathBuf,
}

impl AgentsRoot {
    /// Resolve the default `~/.claude/agents`. Errors if `$HOME`
    /// (or the platform-specific user home) cannot be determined.
    pub fn home() -> Result<Self> {
        let home = home_dir().ok_or_else(|| Error::Artifacts {
            message: "could not determine user home directory".to_string(),
        })?;
        Ok(Self {
            path: home.join(".claude").join("agents"),
        })
    }

    /// Use a specific path as the agents root. Useful for tests
    /// (point at a tempdir) and for non-default installs.
    pub fn at(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    /// The configured root directory.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// List every `*.md` agent at the root, sorted by file stem.
    ///
    /// Returns an empty vec if the root directory doesn't exist (a
    /// fresh Claude Code install with no user agents). Files that
    /// fail to parse contribute a tracing warning and are skipped
    /// rather than failing the whole listing.
    pub fn list(&self) -> Result<Vec<AgentSummary>> {
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
            match parse_agent_file(&path, &stem) {
                Ok(agent) => out.push(AgentSummary::from_agent(&agent)),
                Err(e) => tracing::warn!(?path, "skipping agent: {e}"),
            }
        }
        out.sort_by(|a, b| a.file_stem.cmp(&b.file_stem));
        Ok(out)
    }

    /// Read one agent by file stem (i.e. the basename of `<stem>.md`
    /// under the root). Errors if no such file exists or it fails
    /// to parse.
    pub fn get(&self, file_stem: &str) -> Result<Agent> {
        let path = self.path.join(format!("{file_stem}.md"));
        if !path.exists() {
            return Err(Error::Artifacts {
                message: format!("no agent at {}", path.display()),
            });
        }
        parse_agent_file(&path, file_stem)
    }

    /// Write (create or overwrite) an agent at `<file_stem>.md`.
    ///
    /// Atomic: writes to a temp file in the same directory and
    /// renames into place, so a crash mid-write can't leave a
    /// partially-written file. Creates the agents root directory
    /// if it doesn't exist.
    ///
    /// `file_stem` is validated for path traversal and reserved
    /// names (empty, `.`, `..`, embedded slashes / NUL bytes).
    /// To fail when the agent already exists instead of overwriting,
    /// use [`Self::write_new`].
    pub fn write(&self, file_stem: &str, input: AgentWriteInput) -> Result<()> {
        self.write_inner(file_stem, input, true)
    }

    /// Like [`Self::write`] but errors if the agent already exists.
    /// Useful for "create only" flows where overwriting an existing
    /// agent would be a bug.
    pub fn write_new(&self, file_stem: &str, input: AgentWriteInput) -> Result<()> {
        self.write_inner(file_stem, input, false)
    }

    fn write_inner(
        &self,
        file_stem: &str,
        input: AgentWriteInput,
        allow_overwrite: bool,
    ) -> Result<()> {
        validate_stem(file_stem)?;
        fs::create_dir_all(&self.path)?;
        let path = self.path.join(format!("{file_stem}.md"));
        if !allow_overwrite && path.exists() {
            return Err(Error::Artifacts {
                message: format!("agent already exists at {}", path.display()),
            });
        }

        let markdown = render_agent_markdown(file_stem, &input);

        // Atomic write: tempfile in same dir, then rename. Same-dir
        // tempfile keeps the rename a single inode operation on most
        // filesystems.
        let tmp = self.path.join(format!(".{file_stem}.md.tmp"));
        fs::write(&tmp, markdown)?;
        if let Err(e) = fs::rename(&tmp, &path) {
            // Best-effort cleanup; the rename failure is the real error.
            let _ = fs::remove_file(&tmp);
            return Err(e.into());
        }
        Ok(())
    }

    /// Remove the `<file_stem>.md` agent. Errors if no such file
    /// exists.
    pub fn delete(&self, file_stem: &str) -> Result<()> {
        validate_stem(file_stem)?;
        let path = self.path.join(format!("{file_stem}.md"));
        if !path.exists() {
            return Err(Error::Artifacts {
                message: format!("no agent at {}", path.display()),
            });
        }
        fs::remove_file(&path)?;
        Ok(())
    }
}

/// Input to [`AgentsRoot::write`] / [`AgentsRoot::write_new`].
///
/// Mirrors the parsed [`Agent`] minus the derived bits
/// (`file_stem` and `file_path` are determined by where the agent
/// is being written). `body` is required; everything else is
/// optional and omitted from the rendered frontmatter when empty.
#[derive(Debug, Clone, Default)]
pub struct AgentWriteInput {
    /// Frontmatter `name`. Defaults to the `file_stem` argument
    /// when absent.
    pub name: Option<String>,
    /// Frontmatter `description`. Omitted when None.
    pub description: Option<String>,
    /// Frontmatter `tools` as a list; rendered comma-joined.
    /// Empty list omits the key entirely.
    pub tools: Vec<String>,
    /// Frontmatter `model`. Omitted when None.
    pub model: Option<String>,
    /// Frontmatter `skills` as a list; rendered as a YAML block
    /// sequence. Empty list omits the key entirely.
    pub skills: Vec<String>,
    /// Body of the agent prompt. Trimmed of surrounding whitespace
    /// before write.
    pub body: String,
    /// Additional frontmatter key/value pairs preserved verbatim.
    /// Iterated in sorted order for deterministic output.
    pub extra: BTreeMap<String, String>,
}

fn render_agent_markdown(file_stem: &str, input: &AgentWriteInput) -> String {
    let name = input.name.as_deref().unwrap_or(file_stem);
    let mut out = String::from("---\n");
    push_frontmatter_field(&mut out, "name", name);
    if let Some(desc) = &input.description {
        push_frontmatter_field(&mut out, "description", desc);
    }
    if !input.tools.is_empty() {
        push_frontmatter_field(&mut out, "tools", &input.tools.join(", "));
    }
    if let Some(model) = &input.model {
        push_frontmatter_field(&mut out, "model", model);
    }
    // Rendered as a block sequence rather than comma-joined: YAML
    // reads `skills: a, b` as a plain scalar, not a list.
    if !input.skills.is_empty() {
        out.push_str("skills:\n");
        for skill in &input.skills {
            out.push_str(&format!("  - {skill}\n"));
        }
    }
    for (k, v) in &input.extra {
        push_frontmatter_field(&mut out, k, v);
    }
    out.push_str("---\n\n");
    out.push_str(input.body.trim());
    out.push('\n');
    out
}

/// Render one frontmatter field.
///
/// Single-line values are written as plain `key: value`. Multi-line
/// values go out as a literal block scalar, with the chomping
/// indicator chosen to preserve the exact trailing newlines: a bare
/// `key: value` would leave the continuation lines at the top level,
/// where the reader takes them for new keys.
fn push_frontmatter_field(out: &mut String, key: &str, value: &str) {
    let core = value.trim_end_matches('\n');
    if !value.contains('\n') || core.is_empty() {
        out.push_str(&format!("{key}: {core}\n"));
        return;
    }
    let trailing = value.len() - core.len();
    let indicator = match trailing {
        0 => "|-",
        1 => "|",
        _ => "|+",
    };
    out.push_str(&format!("{key}: {indicator}\n"));
    for line in core.lines() {
        if line.is_empty() {
            out.push('\n');
        } else {
            out.push_str(&format!("  {line}\n"));
        }
    }
    out.push_str(&"\n".repeat(trailing.saturating_sub(1)));
}

fn validate_stem(stem: &str) -> Result<()> {
    if stem.is_empty() {
        return Err(Error::Artifacts {
            message: "file_stem cannot be empty".into(),
        });
    }
    if stem == "." || stem == ".." {
        return Err(Error::Artifacts {
            message: format!("file_stem cannot be {stem:?}"),
        });
    }
    if stem.contains('/') || stem.contains('\\') || stem.contains('\0') {
        return Err(Error::Artifacts {
            message: format!("file_stem contains invalid characters: {stem:?}"),
        });
    }
    Ok(())
}

/// Lightweight metadata for one agent, returned by
/// [`AgentsRoot::list`]. Strips the body to keep listings cheap.
#[derive(Debug, Clone, Serialize)]
pub struct AgentSummary {
    /// Filename stem (`<stem>.md`). The canonical handle for lookup.
    pub file_stem: String,
    /// Frontmatter `name` if present; falls back to `file_stem`.
    pub name: String,
    /// Frontmatter `description` if present.
    pub description: Option<String>,
    /// Frontmatter `tools` parsed as a comma-separated list.
    pub tools: Vec<String>,
    /// Frontmatter `model` if present.
    pub model: Option<String>,
    /// Frontmatter `skills` parsed as a list.
    pub skills: Vec<String>,
    /// Absolute path to the source file.
    pub file_path: PathBuf,
    /// File size in bytes; useful for cheap UI hints.
    pub size_bytes: u64,
}

impl AgentSummary {
    fn from_agent(a: &Agent) -> Self {
        let size_bytes = fs::metadata(&a.file_path)
            .map(|m| m.len())
            .unwrap_or_default();
        Self {
            file_stem: a.file_stem.clone(),
            name: a.name.clone(),
            description: a.description.clone(),
            tools: a.tools.clone(),
            model: a.model.clone(),
            skills: a.skills.clone(),
            file_path: a.file_path.clone(),
            size_bytes,
        }
    }
}

/// Full agent record returned by [`AgentsRoot::get`].
#[derive(Debug, Clone, Serialize)]
pub struct Agent {
    /// Filename stem (`<stem>.md`). The canonical handle for lookup.
    pub file_stem: String,
    /// Frontmatter `name` if present; falls back to `file_stem`.
    pub name: String,
    /// Frontmatter `description` if present.
    pub description: Option<String>,
    /// Frontmatter `tools` parsed as a comma-separated list.
    pub tools: Vec<String>,
    /// Frontmatter `model` if present.
    pub model: Option<String>,
    /// Frontmatter `skills` parsed as a list. Accepts a YAML block
    /// sequence, a flow sequence, or a comma-separated scalar.
    pub skills: Vec<String>,
    /// Absolute path to the source file.
    pub file_path: PathBuf,
    /// Markdown body after the frontmatter block (trimmed of
    /// leading/trailing blank lines).
    pub body: String,
    /// Frontmatter keys other than the typed ones. Preserves
    /// unknown future fields verbatim as raw strings.
    pub extra: BTreeMap<String, String>,
}

fn parse_agent_file(path: &Path, file_stem: &str) -> Result<Agent> {
    let raw = fs::read_to_string(path)?;
    let (frontmatter, body) = split_frontmatter(&raw);

    let mut name = file_stem.to_string();
    let mut description = None;
    let mut tools = Vec::new();
    let mut model = None;
    let mut skills = Vec::new();
    let mut extra = BTreeMap::new();

    if let Some(fm) = frontmatter {
        for (key, value) in frontmatter_entries(fm) {
            match key.as_str() {
                "name" if !value.is_empty() => name = value,
                "description" if !value.is_empty() => description = Some(value),
                "tools" if !value.is_empty() => tools = split_list(&value),
                "model" if !value.is_empty() => model = Some(value),
                "skills" if !value.is_empty() => skills = split_list(&value),
                _ => {
                    extra.insert(key, value);
                }
            }
        }
    }

    Ok(Agent {
        file_stem: file_stem.to_string(),
        name,
        description,
        tools,
        model,
        skills,
        file_path: path.to_path_buf(),
        body: body.trim().to_string(),
        extra,
    })
}

/// Parse a frontmatter block into ordered `(key, value)` pairs.
///
/// Shared by the agent, skill, and command readers so all three
/// artifact types accept the same frontmatter shapes.
///
/// The parser stays permissive and flat: every `key: value` line at
/// any indentation becomes an entry, and lines without a colon are
/// skipped. Duplicate keys are returned in file order, so callers
/// that fold into a map get last-wins.
///
/// It understands two structural features.
///
/// **Block scalars.** A value of `>`, `>-`, `>+`, `|`, `|-`, or `|+`
/// (with an optional explicit indentation digit and an optional
/// trailing comment) consumes the indented block that follows:
///
/// - `>` folds line breaks into spaces; a blank line becomes a
///   newline, and more-indented lines keep their breaks.
/// - `|` preserves line breaks verbatim.
/// - The chomping indicator sets the trailing newline: `-` strips
///   it, the default clips to one, `+` keeps every one.
///
/// Without this, a folded `description` yields the indicator itself
/// as the value and its continuation lines leak out as bogus keys
/// (any line containing a colon) or vanish (any line without one).
///
/// **Block sequences.** An empty value followed by more-indented
/// `- item` lines yields those items joined by `", "`, matching the
/// comma-separated form `tools:` already uses. So
///
/// ```text
/// skills:
///   - sandbox-preflight
///   - durable-context
/// ```
///
/// becomes `("skills", "sandbox-preflight, durable-context")`.
/// Without this the key yields an empty value and the items vanish
/// (no colon to split on) or, if an item contains a colon, leak out
/// as a bogus `- item` key. Use [`split_list`] to get the items back
/// as a `Vec`.
///
/// Nested mappings are deliberately *not* structural: they keep
/// flattening into bare keys, which [`crate::memory`] depends on to
/// read `type:` out of a `metadata:` block.
pub(crate) fn frontmatter_entries(fm: &str) -> Vec<(String, String)> {
    let lines: Vec<&str> = fm.lines().collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < lines.len() {
        let line = lines[i];
        i += 1;
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let Some((k, v)) = trimmed.split_once(':') else {
            continue;
        };
        let key = k.trim();
        if key.is_empty() {
            continue;
        }
        let rest = v.trim();
        match parse_block_header(rest) {
            Some(header) => {
                let (value, consumed) = read_block_scalar(&lines[i..], indent_width(line), header);
                i += consumed;
                out.push((key.to_string(), value));
            }
            // An empty value may be the head of a block sequence.
            // `read_block_sequence` returns None for anything else
            // (a nested mapping, a genuinely empty value), leaving
            // those lines to the flat path exactly as before.
            None if rest.is_empty() => match read_block_sequence(&lines[i..], indent_width(line)) {
                Some((items, consumed)) => {
                    i += consumed;
                    out.push((key.to_string(), items.join(", ")));
                }
                None => out.push((key.to_string(), String::new())),
            },
            None => out.push((key.to_string(), rest.to_string())),
        }
    }
    out
}

/// Read the block sequence that follows a key with an empty value.
///
/// `lines` starts at the line after the key. Returns the item texts
/// and how many lines they span, or `None` when the block isn't a
/// plain sequence.
///
/// Every non-blank line in the block must be a `- item` entry. That
/// rules out nested mappings (`metadata:` followed by `type: x`),
/// which [`crate::memory`] relies on the flat path flattening, and
/// item bodies that continue onto their own lines (`- matcher: Bash`
/// followed by an indented `command:`). Both keep their existing
/// behavior rather than being half-parsed here.
fn read_block_sequence(lines: &[&str], parent_indent: usize) -> Option<(Vec<String>, usize)> {
    let block = lines
        .iter()
        .take_while(|l| l.trim().is_empty() || indent_width(l) > parent_indent)
        .count();
    // Trailing blank lines belong to whatever follows, so the
    // sequence ends at its last non-blank line. No non-blank line
    // means no sequence.
    let end = lines[..block]
        .iter()
        .rposition(|l| !l.trim().is_empty())
        .map(|i| i + 1)?;

    let mut items = Vec::new();
    for line in &lines[..end] {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        // `-` must be followed by whitespace (or end the line) to be
        // an item marker; `-foo` is a scalar that happens to start
        // with a dash.
        let item = trimmed.strip_prefix('-')?;
        if !item.is_empty() && !item.starts_with([' ', '\t']) {
            return None;
        }
        items.push(item.trim().to_string());
    }
    Some((items, end))
}

/// Split a comma-separated frontmatter list value.
///
/// Accepts both spellings Claude Code frontmatter uses for these
/// keys: the bare form (`Read, Grep`) and the YAML flow sequence
/// (`[Read, Grep]`). Block sequences arrive here already joined by
/// [`frontmatter_entries`]. Empty items are dropped.
pub(crate) fn split_list(value: &str) -> Vec<String> {
    let inner = value
        .strip_prefix('[')
        .and_then(|v| v.strip_suffix(']'))
        .unwrap_or(value);
    inner
        .split(',')
        .map(|t| t.trim().to_string())
        .filter(|t| !t.is_empty())
        .collect()
}

/// How a block scalar treats trailing line breaks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Chomp {
    /// `-`: drop the trailing line break entirely.
    Strip,
    /// Default: keep exactly one trailing line break.
    Clip,
    /// `+`: keep every trailing line break.
    Keep,
}

/// A parsed block-scalar header (`>` / `|` plus modifiers).
#[derive(Debug, Clone, Copy)]
struct BlockHeader {
    /// `|` preserves line breaks; `>` folds them into spaces.
    literal: bool,
    chomp: Chomp,
    /// Explicit indentation indicator, relative to the key's indent.
    indent: Option<usize>,
}

/// Recognize a block-scalar header. Returns `None` for anything that
/// isn't one, so plain values (including a value that merely starts
/// with `>`) fall through to the flat path unchanged.
fn parse_block_header(rest: &str) -> Option<BlockHeader> {
    // A header is the indicator plus modifiers, optionally followed
    // by whitespace and a `#` comment. Anything else is a plain value.
    let (head, tail) = match rest.split_once(char::is_whitespace) {
        Some((h, t)) => (h, t.trim_start()),
        None => (rest, ""),
    };
    if !tail.is_empty() && !tail.starts_with('#') {
        return None;
    }

    let mut chars = head.chars();
    let literal = match chars.next()? {
        '|' => true,
        '>' => false,
        _ => return None,
    };
    let mut chomp = Chomp::Clip;
    let mut indent = None;
    for c in chars {
        match c {
            '-' | '+' if chomp == Chomp::Clip => {
                chomp = if c == '-' { Chomp::Strip } else { Chomp::Keep };
            }
            '1'..='9' if indent.is_none() => indent = Some(c as usize - '0' as usize),
            _ => return None,
        }
    }
    Some(BlockHeader {
        literal,
        chomp,
        indent,
    })
}

/// Read the block that follows a block-scalar header.
///
/// `lines` starts at the line after the header. Returns the scalar
/// value and how many lines it consumed. The block is every
/// following line that is blank or indented deeper than
/// `parent_indent` (the indentation of the key line itself).
fn read_block_scalar(lines: &[&str], parent_indent: usize, header: BlockHeader) -> (String, usize) {
    let consumed = lines
        .iter()
        .take_while(|l| l.trim().is_empty() || indent_width(l) > parent_indent)
        .count();
    let block = &lines[..consumed];

    // Content indentation: the explicit indicator if given, else the
    // indentation of the first non-blank line.
    let content_indent = match header.indent {
        Some(n) => parent_indent + n,
        None => block
            .iter()
            .find(|l| !l.trim().is_empty())
            .map(|l| indent_width(l))
            .unwrap_or(parent_indent + 1),
    };
    let stripped: Vec<&str> = block
        .iter()
        .map(|l| &l[indent_width(l).min(content_indent)..])
        .collect();

    // Trailing blank lines are the chomping tail, not content.
    let end = stripped
        .iter()
        .rposition(|l| !l.trim().is_empty())
        .map(|i| i + 1)
        .unwrap_or(0);
    let trailing_blanks = stripped.len() - end;
    let content = &stripped[..end];

    let mut value = if header.literal {
        content
            .iter()
            .map(|l| l.trim_end())
            .collect::<Vec<_>>()
            .join("\n")
    } else {
        fold_block(content)
    };
    match header.chomp {
        Chomp::Strip => {}
        Chomp::Clip => {
            if !value.is_empty() {
                value.push('\n');
            }
        }
        Chomp::Keep => {
            let n = if value.is_empty() {
                trailing_blanks
            } else {
                trailing_blanks + 1
            };
            value.push_str(&"\n".repeat(n));
        }
    }
    (value, consumed)
}

/// Fold a `>` block: line breaks between plain lines become spaces,
/// blank lines become newlines, and breaks adjacent to a
/// more-indented line stay newlines.
fn fold_block(lines: &[&str]) -> String {
    let mut out = String::new();
    let mut blank_run = 0usize;
    let mut have_content = false;
    let mut prev_more_indented = false;
    for line in lines {
        if line.trim().is_empty() {
            blank_run += 1;
            continue;
        }
        let more_indented = line.starts_with([' ', '\t']);
        if blank_run > 0 {
            out.push_str(&"\n".repeat(blank_run));
        } else if have_content {
            if more_indented || prev_more_indented {
                out.push('\n');
            } else {
                out.push(' ');
            }
        }
        out.push_str(line.trim_end());
        blank_run = 0;
        have_content = true;
        prev_more_indented = more_indented;
    }
    out
}

/// Width of a line's leading whitespace. Spaces and tabs are one
/// column each; YAML forbids tabs in indentation anyway.
fn indent_width(line: &str) -> usize {
    line.len() - line.trim_start_matches([' ', '\t']).len()
}

/// Split a markdown file into (optional frontmatter body, content
/// after the frontmatter). Frontmatter is delimited by a leading
/// `---` line and a closing `---` line. Anything else returns
/// `(None, full_text)`.
pub(crate) fn split_frontmatter(raw: &str) -> (Option<&str>, &str) {
    let mut lines = raw.split_inclusive('\n');
    let Some(first) = lines.next() else {
        return (None, raw);
    };
    if first.trim_end_matches(['\n', '\r']) != "---" {
        return (None, raw);
    }
    let after_first = first.len();
    let mut cursor = after_first;
    for line in lines {
        let len = line.len();
        if line.trim_end_matches(['\n', '\r']) == "---" {
            let fm = &raw[after_first..cursor];
            let body_start = cursor + len;
            let body = &raw[body_start..];
            return (Some(fm), body);
        }
        cursor += len;
    }
    (None, raw)
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

    fn write_agent(dir: &Path, file_stem: &str, contents: &str) -> PathBuf {
        let path = dir.join(format!("{file_stem}.md"));
        let mut f = fs::File::create(&path).expect("create md");
        f.write_all(contents.as_bytes()).expect("write md");
        path
    }

    fn fixture_root() -> tempfile::TempDir {
        let tmp = tempfile::tempdir().expect("tempdir");
        write_agent(
            tmp.path(),
            "rust-qa",
            "---\nname: rust-qa\ndescription: Rust quality gate\ntools: Read, Grep, Bash\nmodel: sonnet\n---\n\nYou are a Rust quality gate.\n",
        );
        write_agent(
            tmp.path(),
            "no-frontmatter",
            "Just a body, no frontmatter at all.\n",
        );
        write_agent(
            tmp.path(),
            "minimal",
            "---\nname: minimal\ndescription: Minimal agent\n---\nBody here.\n",
        );
        // A file with an unknown extra key should round-trip.
        write_agent(
            tmp.path(),
            "weird",
            "---\nname: weird\ndescription: has extras\ncustom_key: custom_value\n---\nbody\n",
        );
        // Non-md file should be ignored by list().
        let other = tmp.path().join("README.txt");
        fs::write(&other, "ignore me").expect("write txt");
        tmp
    }

    #[test]
    fn list_returns_only_md_files_sorted() {
        let tmp = fixture_root();
        let root = AgentsRoot::at(tmp.path());
        let agents = root.list().expect("list");
        let stems: Vec<&str> = agents.iter().map(|a| a.file_stem.as_str()).collect();
        assert_eq!(stems, ["minimal", "no-frontmatter", "rust-qa", "weird"]);
    }

    #[test]
    fn list_missing_root_returns_empty() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let root = AgentsRoot::at(tmp.path().join("does-not-exist"));
        let agents = root.list().expect("list");
        assert!(agents.is_empty());
    }

    #[test]
    fn list_typed_metadata() {
        let tmp = fixture_root();
        let root = AgentsRoot::at(tmp.path());
        let agents = root.list().expect("list");
        let rust_qa = agents
            .iter()
            .find(|a| a.file_stem == "rust-qa")
            .expect("rust-qa");
        assert_eq!(rust_qa.name, "rust-qa");
        assert_eq!(rust_qa.description.as_deref(), Some("Rust quality gate"));
        assert_eq!(rust_qa.tools, vec!["Read", "Grep", "Bash"]);
        assert_eq!(rust_qa.model.as_deref(), Some("sonnet"));
        assert!(rust_qa.size_bytes > 0);
    }

    #[test]
    fn list_no_frontmatter_falls_back_to_stem() {
        let tmp = fixture_root();
        let root = AgentsRoot::at(tmp.path());
        let agents = root.list().expect("list");
        let nf = agents
            .iter()
            .find(|a| a.file_stem == "no-frontmatter")
            .expect("no-frontmatter");
        assert_eq!(nf.name, "no-frontmatter");
        assert_eq!(nf.description, None);
        assert!(nf.tools.is_empty());
        assert!(nf.model.is_none());
    }

    #[test]
    fn get_returns_full_agent_with_body() {
        let tmp = fixture_root();
        let root = AgentsRoot::at(tmp.path());
        let agent = root.get("rust-qa").expect("get rust-qa");
        assert_eq!(agent.name, "rust-qa");
        assert_eq!(agent.body, "You are a Rust quality gate.");
    }

    #[test]
    fn get_no_frontmatter_returns_full_body() {
        let tmp = fixture_root();
        let root = AgentsRoot::at(tmp.path());
        let agent = root.get("no-frontmatter").expect("get");
        assert_eq!(agent.body, "Just a body, no frontmatter at all.");
        assert_eq!(agent.name, "no-frontmatter");
        assert!(agent.tools.is_empty());
    }

    #[test]
    fn get_unknown_id_errors() {
        let tmp = fixture_root();
        let root = AgentsRoot::at(tmp.path());
        let err = root.get("nope").unwrap_err();
        assert!(err.to_string().to_lowercase().contains("no agent"));
    }

    #[test]
    fn extra_keys_round_trip_as_strings() {
        let tmp = fixture_root();
        let root = AgentsRoot::at(tmp.path());
        let agent = root.get("weird").expect("get weird");
        assert_eq!(
            agent.extra.get("custom_key").map(String::as_str),
            Some("custom_value")
        );
    }

    #[test]
    fn split_frontmatter_with_block() {
        let raw = "---\nname: x\n---\nbody text\n";
        let (fm, body) = split_frontmatter(raw);
        assert_eq!(fm, Some("name: x\n"));
        assert_eq!(body, "body text\n");
    }

    #[test]
    fn split_frontmatter_no_block() {
        let raw = "no frontmatter here\nsecond line\n";
        let (fm, body) = split_frontmatter(raw);
        assert_eq!(fm, None);
        assert_eq!(body, raw);
    }

    #[test]
    fn split_frontmatter_open_no_close_returns_full() {
        // An opening --- with no matching close shouldn't swallow
        // the file. Conservative behavior: treat as no frontmatter.
        let raw = "---\nname: x\nstill no close here\n";
        let (fm, body) = split_frontmatter(raw);
        assert_eq!(fm, None);
        assert_eq!(body, raw);
    }

    // -- block scalars -------------------------------------------------

    /// The exact shape that motivated block-scalar support: a folded
    /// description whose continuation lines contain colons. Before,
    /// the value was the `>-` indicator itself, `Read-only` leaked
    /// into `extra`, and the colon-free line was dropped.
    #[test]
    fn folded_description_with_colons_is_one_value() {
        let tmp = tempfile::tempdir().expect("tempdir");
        write_agent(
            tmp.path(),
            "auditor",
            concat!(
                "---\n",
                "name: auditor\n",
                "description: >-\n",
                "  Use when surveying a codebase against a rubric and generating a backlog of\n",
                "  GitHub issues. Read-only: never edits files, opens PRs, or commits. Accepts:\n",
                "  \"audit <domain> in <repo>\", dispatched by dispatcher for audit+remediate shape.\n",
                "tools: Read, Glob, Grep, Bash\n",
                "model: sonnet\n",
                "---\n\nBody.\n",
            ),
        );
        let root = AgentsRoot::at(tmp.path());
        let agent = root.get("auditor").expect("get");
        assert_eq!(
            agent.description.as_deref(),
            Some(
                "Use when surveying a codebase against a rubric and generating a backlog of \
                 GitHub issues. Read-only: never edits files, opens PRs, or commits. Accepts: \
                 \"audit <domain> in <repo>\", dispatched by dispatcher for audit+remediate shape."
            )
        );
        // Continuation lines must not leak out as keys.
        assert!(agent.extra.is_empty(), "extra: {:?}", agent.extra);
        // Keys after the block still parse.
        assert_eq!(agent.tools, vec!["Read", "Glob", "Grep", "Bash"]);
        assert_eq!(agent.model.as_deref(), Some("sonnet"));
        assert_eq!(agent.body, "Body.");
    }

    #[test]
    fn literal_block_preserves_newlines() {
        let tmp = tempfile::tempdir().expect("tempdir");
        write_agent(
            tmp.path(),
            "lit",
            "---\nname: lit\ndescription: |-\n  first line\n  second: line\n\n  after blank\nmodel: sonnet\n---\nbody\n",
        );
        let root = AgentsRoot::at(tmp.path());
        let agent = root.get("lit").expect("get");
        assert_eq!(
            agent.description.as_deref(),
            Some("first line\nsecond: line\n\nafter blank")
        );
        assert_eq!(agent.model.as_deref(), Some("sonnet"));
    }

    #[test]
    fn plain_single_line_values_are_unchanged() {
        let entries = frontmatter_entries("name: x\ndescription: a: b\nmodel: sonnet\n");
        assert_eq!(
            entries,
            vec![
                ("name".to_string(), "x".to_string()),
                // Only the first colon splits; the rest is the value.
                ("description".to_string(), "a: b".to_string()),
                ("model".to_string(), "sonnet".to_string()),
            ]
        );
    }

    #[test]
    fn values_starting_with_indicator_char_are_not_blocks() {
        // `> not an indicator` is a plain value, not a block header.
        let entries = frontmatter_entries("description: > plain text\nmodel: sonnet\n");
        assert_eq!(
            entries,
            vec![
                ("description".to_string(), "> plain text".to_string()),
                ("model".to_string(), "sonnet".to_string()),
            ]
        );
    }

    #[test]
    fn chomping_controls_trailing_newline() {
        let cases = [
            (">-", "one two"),
            (">", "one two\n"),
            (">+", "one two\n\n\n"),
            ("|-", "one\ntwo"),
            ("|", "one\ntwo\n"),
            ("|+", "one\ntwo\n\n\n"),
        ];
        for (indicator, expected) in cases {
            let fm = format!("description: {indicator}\n  one\n  two\n\n\nmodel: sonnet\n");
            let entries = frontmatter_entries(&fm);
            assert_eq!(
                entries,
                vec![
                    ("description".to_string(), expected.to_string()),
                    ("model".to_string(), "sonnet".to_string()),
                ],
                "indicator {indicator:?}"
            );
        }
    }

    #[test]
    fn folded_block_keeps_more_indented_lines_on_their_own_lines() {
        let entries = frontmatter_entries(
            "description: >-\n  intro line\n    indented literal\n  tail line\n",
        );
        assert_eq!(
            entries,
            vec![(
                "description".to_string(),
                "intro line\n  indented literal\ntail line".to_string()
            )]
        );
    }

    #[test]
    fn explicit_indentation_indicator_is_honored() {
        // `|4` sets the content indent explicitly, so the two extra
        // spaces on the second line are part of the value.
        let entries = frontmatter_entries("description: |4-\n    one\n      two\n");
        assert_eq!(
            entries,
            vec![("description".to_string(), "one\n  two".to_string())]
        );
    }

    #[test]
    fn block_scalar_at_end_of_frontmatter() {
        let entries = frontmatter_entries("name: x\ndescription: >-\n  only value\n");
        assert_eq!(
            entries,
            vec![
                ("name".to_string(), "x".to_string()),
                ("description".to_string(), "only value".to_string()),
            ]
        );
    }

    #[test]
    fn empty_block_scalar_yields_empty_value() {
        let entries = frontmatter_entries("description: >-\nmodel: sonnet\n");
        assert_eq!(
            entries,
            vec![
                ("description".to_string(), String::new()),
                ("model".to_string(), "sonnet".to_string()),
            ]
        );
    }

    #[test]
    fn block_header_trailing_comment_is_ignored() {
        let entries = frontmatter_entries("description: >- # why\n  folded text\n");
        assert_eq!(
            entries,
            vec![("description".to_string(), "folded text".to_string())]
        );
    }

    #[test]
    fn empty_value_keys_dont_overwrite_defaults() {
        let tmp = tempfile::tempdir().expect("tempdir");
        write_agent(
            tmp.path(),
            "empty-name",
            "---\nname:\ndescription: keeps stem as name\n---\nbody\n",
        );
        let root = AgentsRoot::at(tmp.path());
        let agent = root.get("empty-name").expect("get");
        assert_eq!(agent.name, "empty-name");
    }

    // -- block sequences -----------------------------------------------

    #[test]
    fn block_sequence_becomes_comma_joined_value() {
        let entries = frontmatter_entries("skills:\n  - alpha\n  - beta\n  - gamma\n");
        assert_eq!(
            entries,
            vec![("skills".to_string(), "alpha, beta, gamma".to_string())]
        );
    }

    #[test]
    fn block_sequence_followed_by_another_key() {
        let entries = frontmatter_entries("skills:\n  - alpha\n  - beta\nmodel: sonnet\nname: x\n");
        assert_eq!(
            entries,
            vec![
                ("skills".to_string(), "alpha, beta".to_string()),
                ("model".to_string(), "sonnet".to_string()),
                ("name".to_string(), "x".to_string()),
            ]
        );
    }

    #[test]
    fn empty_sequence_yields_empty_value() {
        // A key with nothing indented under it is YAML null, not a
        // sequence. It keeps the pre-existing empty-value behavior.
        let entries = frontmatter_entries("skills:\nmodel: sonnet\n");
        assert_eq!(
            entries,
            vec![
                ("skills".to_string(), String::new()),
                ("model".to_string(), "sonnet".to_string()),
            ]
        );
    }

    #[test]
    fn empty_sequence_at_end_of_frontmatter() {
        let entries = frontmatter_entries("name: x\nskills:\n");
        assert_eq!(
            entries,
            vec![
                ("name".to_string(), "x".to_string()),
                ("skills".to_string(), String::new()),
            ]
        );
    }

    #[test]
    fn sequence_item_containing_colon_stays_one_item() {
        // Without sequence support this line splits on the colon and
        // leaks out as a bogus `- Use when` key.
        let entries = frontmatter_entries("tags:\n  - Use when: needed\n  - simple\n");
        assert_eq!(
            entries,
            vec![("tags".to_string(), "Use when: needed, simple".to_string())]
        );
    }

    #[test]
    fn blank_lines_around_sequence_are_not_swallowed() {
        let entries = frontmatter_entries("skills:\n  - alpha\n\n  - beta\n\nmodel: sonnet\n");
        assert_eq!(
            entries,
            vec![
                ("skills".to_string(), "alpha, beta".to_string()),
                ("model".to_string(), "sonnet".to_string()),
            ]
        );
    }

    #[test]
    fn bare_dash_item_is_an_empty_string() {
        let entries = frontmatter_entries("skills:\n  -\n  - beta\n");
        assert_eq!(entries, vec![("skills".to_string(), ", beta".to_string())]);
    }

    #[test]
    fn nested_mapping_still_flattens() {
        // `crate::memory` reads `type:` out of a `metadata:` block by
        // relying on this flattening, so a nested mapping must not be
        // mistaken for a sequence.
        let entries = frontmatter_entries("metadata:\n  type: reference\n  origin: abc\n");
        assert_eq!(
            entries,
            vec![
                ("metadata".to_string(), String::new()),
                ("type".to_string(), "reference".to_string()),
                ("origin".to_string(), "abc".to_string()),
            ]
        );
    }

    #[test]
    fn sequence_of_mappings_is_left_to_the_flat_path() {
        // The second line isn't a `- item`, so the block isn't a plain
        // sequence and keeps its previous (flat) parse.
        let entries = frontmatter_entries("hooks:\n  - matcher: Bash\n    command: fmt\n");
        assert_eq!(
            entries,
            vec![
                ("hooks".to_string(), String::new()),
                ("- matcher".to_string(), "Bash".to_string()),
                ("command".to_string(), "fmt".to_string()),
            ]
        );
    }

    #[test]
    fn dash_prefixed_scalar_is_not_a_sequence() {
        // `-5` is a value, not an item marker.
        let entries = frontmatter_entries("weird:\n  -5\n");
        assert_eq!(entries, vec![("weird".to_string(), String::new())]);
    }

    #[test]
    fn split_list_accepts_bare_and_flow_forms() {
        assert_eq!(split_list("a, b, c"), vec!["a", "b", "c"]);
        assert_eq!(split_list("[a, b, c]"), vec!["a", "b", "c"]);
        assert_eq!(split_list("[]"), Vec::<String>::new());
        assert_eq!(split_list("solo"), vec!["solo"]);
        // Brackets must be balanced to be stripped.
        assert_eq!(split_list("[a"), vec!["[a"]);
    }

    /// The exact shape that motivated block-sequence support: the real
    /// `~/.claude/agents/auditor.md`. Before, `skills` landed in
    /// `extra` as an empty string and the three items were dropped.
    #[test]
    fn agent_skills_block_sequence_parses() {
        let tmp = tempfile::tempdir().expect("tempdir");
        write_agent(
            tmp.path(),
            "auditor",
            concat!(
                "---\n",
                "name: auditor\n",
                "description: Surveys a codebase against a rubric.\n",
                "tools: Read, Glob, Grep, Bash\n",
                "model: sonnet\n",
                "skills:\n",
                "  - sandbox-preflight\n",
                "  - durable-context\n",
                "  - audit-protocol\n",
                "---\n\nYou are the auditor.\n",
            ),
        );
        let root = AgentsRoot::at(tmp.path());
        let agent = root.get("auditor").expect("get");
        assert_eq!(
            agent.skills,
            vec!["sandbox-preflight", "durable-context", "audit-protocol"]
        );
        // The key must not also land in extra.
        assert!(agent.extra.is_empty(), "extra: {:?}", agent.extra);
        assert_eq!(agent.tools, vec!["Read", "Glob", "Grep", "Bash"]);
        assert_eq!(agent.model.as_deref(), Some("sonnet"));
        assert_eq!(agent.body, "You are the auditor.");

        // list() carries skills too.
        let summary = root.list().expect("list").into_iter().next().expect("one");
        assert_eq!(summary.skills, agent.skills);
    }

    #[test]
    fn agent_skills_accepts_flow_and_scalar_forms() {
        let tmp = tempfile::tempdir().expect("tempdir");
        write_agent(tmp.path(), "flow", "---\nskills: [a, b]\n---\nbody\n");
        write_agent(tmp.path(), "scalar", "---\nskills: a, b\n---\nbody\n");
        let root = AgentsRoot::at(tmp.path());
        assert_eq!(root.get("flow").expect("get").skills, vec!["a", "b"]);
        assert_eq!(root.get("scalar").expect("get").skills, vec!["a", "b"]);
    }

    #[test]
    fn agent_without_skills_has_empty_list() {
        let tmp = fixture_root();
        let root = AgentsRoot::at(tmp.path());
        assert!(root.get("rust-qa").expect("get").skills.is_empty());
    }

    #[test]
    fn skills_round_trip_through_write_as_a_block_sequence() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let root = AgentsRoot::at(tmp.path());
        let input = AgentWriteInput {
            name: Some("auditor".into()),
            skills: vec!["sandbox-preflight".into(), "durable-context".into()],
            body: "b".into(),
            ..Default::default()
        };
        root.write("auditor", input).expect("write");

        // Rendered as a real YAML sequence, not `skills: a, b` (which
        // YAML would read as a plain scalar).
        let raw = fs::read_to_string(tmp.path().join("auditor.md")).expect("read");
        assert!(
            raw.contains("skills:\n  - sandbox-preflight\n  - durable-context\n"),
            "raw: {raw}"
        );
        assert_eq!(
            root.get("auditor").expect("get").skills,
            vec!["sandbox-preflight", "durable-context"]
        );
    }

    // -- write / write_new / delete -----------------------------------

    fn input_with_body(body: &str) -> AgentWriteInput {
        AgentWriteInput {
            body: body.into(),
            ..Default::default()
        }
    }

    #[test]
    fn write_creates_new_agent_round_trips_via_get() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let root = AgentsRoot::at(tmp.path());
        let input = AgentWriteInput {
            name: Some("my-agent".into()),
            description: Some("does the thing".into()),
            tools: vec!["Read".into(), "Bash".into()],
            model: Some("sonnet".into()),
            skills: vec!["durable-context".into()],
            body: "You are an agent.".into(),
            extra: BTreeMap::new(),
        };
        root.write("my-agent", input).expect("write");

        let agent = root.get("my-agent").expect("get");
        assert_eq!(agent.name, "my-agent");
        assert_eq!(agent.description.as_deref(), Some("does the thing"));
        assert_eq!(agent.tools, vec!["Read", "Bash"]);
        assert_eq!(agent.model.as_deref(), Some("sonnet"));
        assert_eq!(agent.body, "You are an agent.");
    }

    #[test]
    fn write_overwrites_existing_agent() {
        let tmp = fixture_root();
        let root = AgentsRoot::at(tmp.path());
        // rust-qa exists in the fixture.
        let input = AgentWriteInput {
            description: Some("rewritten".into()),
            body: "new body".into(),
            ..Default::default()
        };
        root.write("rust-qa", input).expect("overwrite");
        let agent = root.get("rust-qa").expect("get");
        assert_eq!(agent.description.as_deref(), Some("rewritten"));
        assert_eq!(agent.body, "new body");
        // tools/model from the original should be gone -- write
        // replaces the whole file.
        assert!(agent.tools.is_empty(), "tools: {:?}", agent.tools);
        assert!(agent.model.is_none());
    }

    #[test]
    fn write_new_errors_when_already_exists() {
        let tmp = fixture_root();
        let root = AgentsRoot::at(tmp.path());
        let err = root
            .write_new("rust-qa", input_with_body("body"))
            .unwrap_err();
        assert!(err.to_string().contains("already exists"), "err: {err}");
    }

    #[test]
    fn write_new_succeeds_for_fresh_stem() {
        let tmp = fixture_root();
        let root = AgentsRoot::at(tmp.path());
        root.write_new("brand-new", input_with_body("hello"))
            .expect("write_new");
        let agent = root.get("brand-new").expect("get");
        assert_eq!(agent.body, "hello");
    }

    #[test]
    fn write_creates_root_directory_if_missing() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let root = AgentsRoot::at(tmp.path().join("does-not-exist-yet"));
        root.write("foo", input_with_body("body")).expect("write");
        let agent = root.get("foo").expect("get");
        assert_eq!(agent.body, "body");
    }

    #[test]
    fn write_defaults_name_to_file_stem_when_absent() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let root = AgentsRoot::at(tmp.path());
        root.write("my-stem", input_with_body("b")).expect("write");
        let agent = root.get("my-stem").expect("get");
        assert_eq!(agent.name, "my-stem");
    }

    #[test]
    fn write_preserves_extra_keys() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let root = AgentsRoot::at(tmp.path());
        let mut extra = BTreeMap::new();
        extra.insert("custom_key".into(), "custom_value".into());
        let input = AgentWriteInput {
            body: "b".into(),
            extra,
            ..Default::default()
        };
        root.write("ex", input).expect("write");
        let agent = root.get("ex").expect("get");
        assert_eq!(
            agent.extra.get("custom_key").map(String::as_str),
            Some("custom_value")
        );
    }

    #[test]
    fn write_omits_optional_keys_when_unset() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let root = AgentsRoot::at(tmp.path());
        root.write("min", input_with_body("body only"))
            .expect("write");
        let raw = std::fs::read_to_string(tmp.path().join("min.md")).unwrap();
        assert!(!raw.contains("description:"), "raw: {raw}");
        assert!(!raw.contains("tools:"), "raw: {raw}");
        assert!(!raw.contains("model:"), "raw: {raw}");
    }

    #[test]
    fn write_rejects_path_traversal() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let root = AgentsRoot::at(tmp.path());
        for bad in ["", ".", "..", "a/b", "a\\b", "a\0b"] {
            let err = root.write(bad, input_with_body("b")).unwrap_err();
            assert!(
                err.to_string().to_lowercase().contains("file_stem"),
                "bad stem {bad:?} not rejected: {err}"
            );
        }
    }

    #[test]
    fn delete_removes_file() {
        let tmp = fixture_root();
        let root = AgentsRoot::at(tmp.path());
        assert!(root.get("rust-qa").is_ok());
        root.delete("rust-qa").expect("delete");
        let err = root.get("rust-qa").unwrap_err();
        assert!(err.to_string().contains("no agent"), "err: {err}");
    }

    #[test]
    fn delete_unknown_stem_errors() {
        let tmp = fixture_root();
        let root = AgentsRoot::at(tmp.path());
        let err = root.delete("nope").unwrap_err();
        assert!(err.to_string().contains("no agent"), "err: {err}");
    }

    #[test]
    fn delete_rejects_path_traversal() {
        let tmp = fixture_root();
        let root = AgentsRoot::at(tmp.path());
        for bad in ["", ".", "..", "a/b", "a\\b"] {
            let err = root.delete(bad).unwrap_err();
            assert!(
                err.to_string().to_lowercase().contains("file_stem"),
                "bad stem {bad:?} not rejected: {err}"
            );
        }
    }

    #[test]
    fn write_round_trips_multi_line_description() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let root = AgentsRoot::at(tmp.path());
        // A description with embedded newlines and a colon: written
        // as a plain `key: value` it would corrupt the frontmatter.
        let desc = "first line\nsecond: line\n\nafter blank";
        let input = AgentWriteInput {
            description: Some(desc.into()),
            model: Some("sonnet".into()),
            body: "b".into(),
            ..Default::default()
        };
        root.write("multi", input).expect("write");

        let raw = std::fs::read_to_string(tmp.path().join("multi.md")).expect("read");
        assert!(raw.contains("description: |-\n"), "raw: {raw}");

        let agent = root.get("multi").expect("get");
        assert_eq!(agent.description.as_deref(), Some(desc));
        assert_eq!(agent.model.as_deref(), Some("sonnet"));
        assert!(agent.extra.is_empty(), "extra: {:?}", agent.extra);
    }

    #[test]
    fn write_round_trips_trailing_newlines_in_description() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let root = AgentsRoot::at(tmp.path());
        for desc in ["a\nb", "a\nb\n", "a\nb\n\n\n"] {
            let input = AgentWriteInput {
                description: Some(desc.into()),
                body: "b".into(),
                ..Default::default()
            };
            root.write("chomp", input).expect("write");
            let agent = root.get("chomp").expect("get");
            assert_eq!(agent.description.as_deref(), Some(desc), "desc {desc:?}");
        }
    }

    #[test]
    fn render_orders_canonical_keys_before_extras() {
        let mut extra = BTreeMap::new();
        extra.insert("zzz_last".into(), "v".into());
        extra.insert("aaa_first".into(), "v".into());
        let input = AgentWriteInput {
            name: Some("n".into()),
            description: Some("d".into()),
            tools: vec!["t1".into(), "t2".into()],
            model: Some("haiku".into()),
            skills: vec!["s1".into(), "s2".into()],
            body: "body".into(),
            extra,
        };
        let md = render_agent_markdown("stem", &input);
        let lines: Vec<&str> = md.lines().collect();
        // Header
        assert_eq!(lines[0], "---");
        // Canonical order: name, description, tools, model, skills,
        // then sorted extras.
        assert_eq!(lines[1], "name: n");
        assert_eq!(lines[2], "description: d");
        assert_eq!(lines[3], "tools: t1, t2");
        assert_eq!(lines[4], "model: haiku");
        assert_eq!(lines[5], "skills:");
        assert_eq!(lines[6], "  - s1");
        assert_eq!(lines[7], "  - s2");
        assert_eq!(lines[8], "aaa_first: v");
        assert_eq!(lines[9], "zzz_last: v");
        assert_eq!(lines[10], "---");
    }
}
