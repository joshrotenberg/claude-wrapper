//! `claude auto-mode` subcommands: inspect the auto-mode classifier
//! configuration.
//!
//! The CLI exposes three children under `auto-mode`:
//!
//! - [`AutoModeConfigCommand`] -- `auto-mode config`, the effective
//!   merged config as JSON.
//! - [`AutoModeDefaultsCommand`] -- `auto-mode defaults`, the default
//!   rules as JSON.
//! - [`AutoModeCritiqueCommand`] -- `auto-mode critique`, AI feedback
//!   on your custom auto-mode rules.

#[cfg(feature = "async")]
use crate::Claude;
use crate::command::ClaudeCommand;
#[cfg(feature = "async")]
use crate::error::Result;
#[cfg(feature = "async")]
use crate::exec;
use crate::exec::CommandOutput;

/// Print the effective auto-mode config as JSON.
///
/// Emits your settings where set and defaults otherwise, merged.
///
/// # Example
///
/// ```no_run
/// # #[cfg(feature = "async")] {
/// use claude_wrapper::{AutoModeConfigCommand, Claude, ClaudeCommand};
///
/// # async fn example() -> claude_wrapper::Result<()> {
/// let claude = Claude::builder().build()?;
/// let output = AutoModeConfigCommand::new().execute(&claude).await?;
/// println!("{}", output.stdout);
/// # Ok(()) }
/// # }
/// ```
#[derive(Debug, Clone, Default)]
pub struct AutoModeConfigCommand;

impl AutoModeConfigCommand {
    #[must_use]
    pub fn new() -> Self {
        Self
    }
}

impl ClaudeCommand for AutoModeConfigCommand {
    type Output = CommandOutput;

    fn args(&self) -> Vec<String> {
        vec!["auto-mode".to_string(), "config".to_string()]
    }

    #[cfg(feature = "async")]
    async fn execute(&self, claude: &Claude) -> Result<CommandOutput> {
        exec::run_claude(claude, self.args()).await
    }
}

/// Print the default auto-mode environment, allow, soft_deny, and
/// hard_deny rules as JSON. Useful as a reference when writing
/// custom rules. The soft/hard deny split distinguishes "warn but
/// allow when explicitly overridden" from "always block."
#[derive(Debug, Clone, Default)]
pub struct AutoModeDefaultsCommand;

impl AutoModeDefaultsCommand {
    #[must_use]
    pub fn new() -> Self {
        Self
    }
}

impl ClaudeCommand for AutoModeDefaultsCommand {
    type Output = CommandOutput;

    fn args(&self) -> Vec<String> {
        vec!["auto-mode".to_string(), "defaults".to_string()]
    }

    #[cfg(feature = "async")]
    async fn execute(&self, claude: &Claude) -> Result<CommandOutput> {
        exec::run_claude(claude, self.args()).await
    }
}

/// Get AI feedback on your custom auto-mode rules.
///
/// Takes an optional model override; without one the CLI picks its
/// default. Output is free-form text.
///
/// # Example
///
/// ```no_run
/// # #[cfg(feature = "async")] {
/// use claude_wrapper::{AutoModeCritiqueCommand, Claude, ClaudeCommand};
///
/// # async fn example() -> claude_wrapper::Result<()> {
/// let claude = Claude::builder().build()?;
/// let output = AutoModeCritiqueCommand::new()
///     .model("opus")
///     .execute(&claude)
///     .await?;
/// println!("{}", output.stdout);
/// # Ok(()) }
/// # }
/// ```
#[derive(Debug, Clone, Default)]
pub struct AutoModeCritiqueCommand {
    model: Option<String>,
}

impl AutoModeCritiqueCommand {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Override the model used for the critique.
    #[must_use]
    pub fn model(mut self, model: impl Into<String>) -> Self {
        self.model = Some(model.into());
        self
    }
}

impl ClaudeCommand for AutoModeCritiqueCommand {
    type Output = CommandOutput;

    fn args(&self) -> Vec<String> {
        let mut args = vec!["auto-mode".to_string(), "critique".to_string()];
        if let Some(ref model) = self.model {
            args.push("--model".to_string());
            args.push(model.clone());
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
    fn config_command_args() {
        let cmd = AutoModeConfigCommand::new();
        assert_eq!(
            ClaudeCommand::args(&cmd),
            vec!["auto-mode".to_string(), "config".to_string()]
        );
    }

    #[test]
    fn defaults_command_args() {
        let cmd = AutoModeDefaultsCommand::new();
        assert_eq!(
            ClaudeCommand::args(&cmd),
            vec!["auto-mode".to_string(), "defaults".to_string()]
        );
    }

    #[test]
    fn critique_command_defaults_omit_model() {
        let cmd = AutoModeCritiqueCommand::new();
        let args = ClaudeCommand::args(&cmd);
        assert_eq!(args, vec!["auto-mode".to_string(), "critique".to_string()]);
    }

    #[test]
    fn critique_command_with_model() {
        let cmd = AutoModeCritiqueCommand::new().model("opus");
        let args = ClaudeCommand::args(&cmd);
        assert_eq!(
            args,
            vec![
                "auto-mode".to_string(),
                "critique".to_string(),
                "--model".to_string(),
                "opus".to_string(),
            ]
        );
    }
}
