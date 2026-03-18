//! CLI definitions using clap.

use std::path::PathBuf;

use clap::{Parser, Subcommand};

/// Manifest-driven execution engine for headless Claude Code sessions.
#[derive(Debug, Parser)]
#[command(name = "claudes", version, about)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Execute tasks from a manifest, config, or CLI args.
    Run(RunArgs),

    /// Generate a manifest without executing.
    Plan(PlanArgs),

    /// Generate a manifest template with stub tasks.
    Init(InitArgs),

    /// Show status of the most recent run.
    Status(StatusArgs),

    /// Remove worktrees and temporary state.
    Clean(CleanArgs),
}

/// Arguments for `claudes run`.
#[derive(Debug, Parser)]
pub struct RunArgs {
    /// Run from a manifest file (all other generation options ignored).
    #[arg(long)]
    pub manifest: Option<PathBuf>,

    /// Task prompt (repeatable for multiple tasks).
    #[arg(short, long)]
    pub prompt: Vec<String>,

    /// Read prompt from stdin.
    #[arg(long)]
    pub stdin: bool,

    /// Override model.
    #[arg(long)]
    pub model: Option<String>,

    /// Override timeout (e.g. "30m", "1h", or seconds).
    #[arg(long)]
    pub timeout: Option<String>,

    /// Override max turns.
    #[arg(long)]
    pub max_turns: Option<u32>,

    /// Override budget in USD.
    #[arg(long)]
    pub max_budget_usd: Option<f64>,

    /// Override effort level (low|medium|high).
    #[arg(long)]
    pub effort: Option<String>,

    /// Override permission mode.
    #[arg(long)]
    pub permission_mode: Option<String>,

    /// Override allowed tools (comma-separated).
    #[arg(long)]
    pub allowed_tools: Option<String>,

    /// Override disallowed tools (comma-separated).
    #[arg(long)]
    pub disallowed_tools: Option<String>,

    /// Append to system prompt.
    #[arg(long)]
    pub append_system_prompt: Option<String>,

    /// Override isolation (worktree|clone|none).
    #[arg(long)]
    pub isolation: Option<String>,

    /// Show generated manifest without executing.
    #[arg(long)]
    pub dry_run: bool,

    /// Exit code only, no streaming output.
    #[arg(long)]
    pub quiet: bool,

    /// Output format (text|json).
    #[arg(long, default_value = "text")]
    pub output: String,

    /// Overwrite existing worktrees.
    #[arg(long)]
    pub force: bool,

    /// Auto-cleanup worktrees after run (none|on-success|always; default: none).
    #[arg(long, default_value = "none")]
    pub cleanup: String,

    /// Increase output verbosity (repeat for more detail: -v, -vv).
    #[arg(short = 'v', long = "verbose", action = clap::ArgAction::Count)]
    pub verbose: u8,

    /// Disable colored output.
    #[arg(long)]
    pub no_color: bool,
}

/// Arguments for `claudes status`.
#[derive(Debug, Parser)]
pub struct StatusArgs {
    /// Run ID to show (default: latest run).
    pub run_id: Option<String>,

    /// List all runs.
    #[arg(long)]
    pub list: bool,

    /// Output as JSON instead of a table.
    #[arg(long)]
    pub json: bool,
}

/// Arguments for `claudes plan`.
#[derive(Debug, Parser)]
pub struct PlanArgs {
    /// Task prompt (repeatable for multiple tasks).
    #[arg(short, long)]
    pub prompt: Vec<String>,

    /// Read prompt from stdin.
    #[arg(long)]
    pub stdin: bool,

    /// Write manifest to file (default: stdout).
    #[arg(short, long)]
    pub out: Option<PathBuf>,

    /// Override model.
    #[arg(long)]
    pub model: Option<String>,

    /// Override timeout.
    #[arg(long)]
    pub timeout: Option<String>,

    /// Override max turns.
    #[arg(long)]
    pub max_turns: Option<u32>,

    /// Override budget in USD.
    #[arg(long)]
    pub max_budget_usd: Option<f64>,

    /// Override effort level.
    #[arg(long)]
    pub effort: Option<String>,

    /// Override permission mode.
    #[arg(long)]
    pub permission_mode: Option<String>,

    /// Override allowed tools (comma-separated).
    #[arg(long)]
    pub allowed_tools: Option<String>,

    /// Override disallowed tools (comma-separated).
    #[arg(long)]
    pub disallowed_tools: Option<String>,

    /// Append to system prompt.
    #[arg(long)]
    pub append_system_prompt: Option<String>,

    /// Override isolation (worktree|clone|none).
    #[arg(long)]
    pub isolation: Option<String>,
}

/// Arguments for `claudes init`.
#[derive(Debug, Parser)]
pub struct InitArgs {
    /// Number of task stubs to generate.
    #[arg(long, default_value = "1")]
    pub tasks: usize,

    /// Set model on each task stub.
    #[arg(long)]
    pub model: Option<String>,

    /// Set isolation on each task stub (worktree|clone|none).
    #[arg(long)]
    pub isolation: Option<String>,

    /// Write manifest to file (default: stdout).
    #[arg(short, long)]
    pub out: Option<PathBuf>,
}

/// Arguments for `claudes clean`.
#[derive(Debug, Parser)]
pub struct CleanArgs {
    /// Remove all worktrees (default: only completed tasks).
    #[arg(long)]
    pub all: bool,

    /// Force remove even with uncommitted changes.
    #[arg(long)]
    pub force: bool,
}

/// Parse a timeout string like "30m", "1h", "3600" into seconds.
pub fn parse_timeout(s: &str) -> Result<u64, String> {
    let s = s.trim();
    if let Ok(secs) = s.parse::<u64>() {
        return Ok(secs);
    }

    let (num_str, unit) = s.split_at(s.len() - 1);
    let num: u64 = num_str
        .parse()
        .map_err(|_| format!("invalid timeout: {s}"))?;

    match unit {
        "s" => Ok(num),
        "m" => Ok(num * 60),
        "h" => Ok(num * 3600),
        _ => Err(format!("invalid timeout unit: {unit} (use s, m, or h)")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_timeout_seconds() {
        assert_eq!(parse_timeout("3600").unwrap(), 3600);
    }

    #[test]
    fn parse_timeout_minutes() {
        assert_eq!(parse_timeout("30m").unwrap(), 1800);
    }

    #[test]
    fn parse_timeout_hours() {
        assert_eq!(parse_timeout("1h").unwrap(), 3600);
    }

    #[test]
    fn parse_timeout_with_s() {
        assert_eq!(parse_timeout("90s").unwrap(), 90);
    }

    #[test]
    fn parse_timeout_invalid() {
        assert!(parse_timeout("abc").is_err());
        assert!(parse_timeout("30x").is_err());
    }
}
