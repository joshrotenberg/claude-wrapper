//! `claude ultrareview` -- cloud-hosted multi-agent code review.

#[cfg(feature = "async")]
use crate::Claude;
use crate::command::ClaudeCommand;
#[cfg(feature = "async")]
use crate::error::Result;
#[cfg(feature = "async")]
use crate::exec;
use crate::exec::CommandOutput;

/// Run `claude ultrareview [target] [--json] [--timeout <minutes>]`.
///
/// Runs a cloud-hosted multi-agent code review and prints the findings.
/// With no target, reviews the current branch; pass a PR number/URL or a
/// base branch name to review that instead.
///
/// The review runs in the cloud and can take many minutes. Size the
/// [`Claude`] client timeout to at least the value passed to
/// [`Self::timeout`] (the CLI default is 30 minutes) so the wrapper does
/// not kill the process before the review finishes.
///
/// # Example
///
/// ```no_run
/// # #[cfg(feature = "async")] {
/// use claude_wrapper::{Claude, ClaudeCommand, UltrareviewCommand};
///
/// # async fn example() -> claude_wrapper::Result<()> {
/// let claude = Claude::builder().build()?;
/// let out = UltrareviewCommand::new()
///     .target("123")
///     .json()
///     .timeout(45)
///     .execute(&claude)
///     .await?;
/// println!("{}", out.stdout);
/// # Ok(()) }
/// # }
/// ```
#[derive(Debug, Clone, Default)]
pub struct UltrareviewCommand {
    target: Option<String>,
    json: bool,
    timeout_minutes: Option<u32>,
}

impl UltrareviewCommand {
    /// Create a new ultrareview command targeting the current branch.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Review a specific target instead of the current branch: a PR
    /// number/URL or a base branch name.
    #[must_use]
    pub fn target(mut self, target: impl Into<String>) -> Self {
        self.target = Some(target.into());
        self
    }

    /// Print the raw `bugs.json` payload instead of formatted findings
    /// (`--json`).
    #[must_use]
    pub fn json(mut self) -> Self {
        self.json = true;
        self
    }

    /// Maximum minutes to wait for the review to finish (`--timeout`).
    /// The CLI default is 30.
    #[must_use]
    pub fn timeout(mut self, minutes: u32) -> Self {
        self.timeout_minutes = Some(minutes);
        self
    }
}

impl ClaudeCommand for UltrareviewCommand {
    type Output = CommandOutput;

    fn args(&self) -> Vec<String> {
        let mut args = vec!["ultrareview".to_string()];
        if self.json {
            args.push("--json".to_string());
        }
        if let Some(minutes) = self.timeout_minutes {
            args.push("--timeout".to_string());
            args.push(minutes.to_string());
        }
        if let Some(ref target) = self.target {
            args.push(target.clone());
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
    fn ultrareview_defaults() {
        assert_eq!(
            ClaudeCommand::args(&UltrareviewCommand::new()),
            vec!["ultrareview"]
        );
    }

    #[test]
    fn ultrareview_with_target_json_timeout() {
        let cmd = UltrareviewCommand::new().target("123").json().timeout(45);
        assert_eq!(
            ClaudeCommand::args(&cmd),
            vec!["ultrareview", "--json", "--timeout", "45", "123"]
        );
    }

    #[test]
    fn ultrareview_target_only() {
        let cmd = UltrareviewCommand::new().target("main");
        assert_eq!(ClaudeCommand::args(&cmd), vec!["ultrareview", "main"]);
    }

    #[test]
    fn ultrareview_json_only() {
        let cmd = UltrareviewCommand::new().json();
        assert_eq!(ClaudeCommand::args(&cmd), vec!["ultrareview", "--json"]);
    }
}
