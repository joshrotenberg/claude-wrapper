//! `claude project` subcommand wrappers.
//!
//! Currently exposes [`ProjectPurgeCommand`] for `claude project
//! purge`, which deletes all Claude Code state for a project --
//! transcripts, tasks, file history, and config entry. Destructive;
//! callers running headless should always pass
//! [`ProjectPurgeCommand::yes`] to skip the confirmation prompt
//! (the CLI hangs on the prompt when stdin / stdout is not a TTY).

#[cfg(feature = "async")]
use crate::Claude;
use crate::command::ClaudeCommand;
#[cfg(feature = "async")]
use crate::error::Result;
#[cfg(feature = "async")]
use crate::exec;
use crate::exec::CommandOutput;

/// Delete all Claude Code state for a project.
///
/// Wraps `claude project purge [path]`. Removes transcripts, tasks,
/// file history, and the project's config entry. Use [`Self::path`]
/// to target a specific project, or [`Self::all`] to purge every
/// known project (the two are mutually exclusive at the CLI level;
/// passing both lets the CLI decide).
///
/// **Headless callers should pass [`Self::yes`]** -- the CLI
/// requires `-y` whenever stdin/stdout isn't a TTY and otherwise
/// waits on a confirmation prompt that no one is around to answer.
/// Combine with [`Self::dry_run`] to preview without deleting.
///
/// # Example
///
/// ```no_run
/// use claude_wrapper::{Claude, ClaudeCommand, ProjectPurgeCommand};
///
/// # async fn example() -> claude_wrapper::Result<()> {
/// let claude = Claude::builder().build()?;
/// // Dry-run, no confirmation needed since nothing changes:
/// let preview = ProjectPurgeCommand::new()
///     .path("/some/project/path")
///     .dry_run()
///     .execute(&claude)
///     .await?;
/// println!("{}", preview.stdout);
/// # Ok(()) }
/// ```
#[derive(Debug, Clone, Default)]
pub struct ProjectPurgeCommand {
    path: Option<String>,
    all: bool,
    dry_run: bool,
    interactive: bool,
    yes: bool,
}

impl ProjectPurgeCommand {
    /// Create a new purge command. Without [`Self::path`] or
    /// [`Self::all`], the CLI defaults to the current directory.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Purge state for the project at this path (positional `[path]`).
    /// Mutually exclusive with [`Self::all`] at the CLI level.
    #[must_use]
    pub fn path(mut self, path: impl Into<String>) -> Self {
        self.path = Some(path.into());
        self
    }

    /// Purge state for every known project (`--all`). Mutually
    /// exclusive with [`Self::path`] at the CLI level.
    #[must_use]
    pub fn all(mut self) -> Self {
        self.all = true;
        self
    }

    /// List what would be deleted without deleting anything
    /// (`--dry-run`). Safe to combine with [`Self::all`] for a
    /// preview of full-purge scope.
    #[must_use]
    pub fn dry_run(mut self) -> Self {
        self.dry_run = true;
        self
    }

    /// Prompt for each item before deleting (`-i, --interactive`).
    /// Only useful in TTY contexts; headless callers should leave
    /// this off and pair [`Self::dry_run`] with [`Self::yes`] for
    /// scoped automation.
    #[must_use]
    pub fn interactive(mut self) -> Self {
        self.interactive = true;
        self
    }

    /// Skip the confirmation prompt (`-y`). **Required for non-TTY
    /// callers** -- without it the CLI will hang waiting on stdin.
    /// Every wrapper consumer running under `execute()` is non-TTY
    /// by definition.
    #[must_use]
    pub fn yes(mut self) -> Self {
        self.yes = true;
        self
    }
}

impl ClaudeCommand for ProjectPurgeCommand {
    type Output = CommandOutput;

    fn args(&self) -> Vec<String> {
        let mut args = vec!["project".to_string(), "purge".to_string()];
        if self.all {
            args.push("--all".to_string());
        }
        if self.dry_run {
            args.push("--dry-run".to_string());
        }
        if self.interactive {
            args.push("--interactive".to_string());
        }
        if self.yes {
            args.push("--yes".to_string());
        }
        if let Some(ref path) = self.path {
            args.push(path.clone());
        }
        args
    }

    #[cfg(feature = "async")]
    async fn execute(&self, claude: &Claude) -> Result<CommandOutput> {
        exec::run_claude(claude, self.args()).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn purge_defaults_to_bare_subcommand() {
        let cmd = ProjectPurgeCommand::new();
        assert_eq!(ClaudeCommand::args(&cmd), vec!["project", "purge"]);
    }

    #[test]
    fn purge_with_path_passes_positional() {
        let cmd = ProjectPurgeCommand::new().path("/tmp/old-project");
        assert_eq!(
            ClaudeCommand::args(&cmd),
            vec!["project", "purge", "/tmp/old-project"]
        );
    }

    #[test]
    fn purge_all_emits_flag() {
        let cmd = ProjectPurgeCommand::new().all().yes();
        assert_eq!(
            ClaudeCommand::args(&cmd),
            vec!["project", "purge", "--all", "--yes"]
        );
    }

    #[test]
    fn purge_dry_run_with_yes_is_safe_preview() {
        let cmd = ProjectPurgeCommand::new().dry_run().yes();
        assert_eq!(
            ClaudeCommand::args(&cmd),
            vec!["project", "purge", "--dry-run", "--yes"]
        );
    }

    #[test]
    fn purge_all_flags() {
        let cmd = ProjectPurgeCommand::new()
            .path("/proj")
            .all()
            .dry_run()
            .interactive()
            .yes();
        // CLI accepts both --all and [path]; we emit them in
        // canonical order and let the CLI decide.
        assert_eq!(
            ClaudeCommand::args(&cmd),
            vec![
                "project",
                "purge",
                "--all",
                "--dry-run",
                "--interactive",
                "--yes",
                "/proj"
            ]
        );
    }

    #[test]
    fn purge_path_lands_last_so_positional_is_unambiguous() {
        let cmd = ProjectPurgeCommand::new().yes().path("./me");
        let args = ClaudeCommand::args(&cmd);
        // The positional must be the final arg; anything else risks
        // being interpreted as the path.
        assert_eq!(args.last().map(String::as_str), Some("./me"));
    }
}
