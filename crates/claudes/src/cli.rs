//! CLI definitions using clap.

use std::path::PathBuf;

use clap::{Parser, Subcommand};

/// Manifest-driven execution engine for headless Claude Code sessions.
#[derive(Debug, Parser)]
#[command(
    name = "claudes",
    version,
    about = "Manifest-driven execution engine for headless Claude Code sessions",
    long_about = "Manifest-driven execution engine for headless Claude Code sessions.\n\n\
        Run multiple Claude Code tasks in parallel with git worktree isolation,\n\
        dependency chains, breadcrumb context passing, and structured output.",
    after_long_help = "\
GETTING STARTED:\n\
  Interactive mode (recommended):\n\
    claudes                                    Launch orchestrator\n\
\n\
  From a manifest:\n\
    claudes run --manifest plan.json           Run tasks from manifest\n\
    claudes run -p 'fix the bug'              Run a single ad-hoc task\n\
    claudes run -p 'task 1' -p 'task 2'       Run parallel ad-hoc tasks\n\
\n\
  Generate manifests:\n\
    claudes plan -p 'do X' -p 'do Y'          Quick manifest (no AI)\n\
    claudes generate -p 'describe the work'   AI-generated manifest\n\
    claudes init                              Template with stub tasks\n\
\n\
  Monitor and fix:\n\
    claudes status                            Latest run results\n\
    claudes fix                               Re-run failed tasks\n\
    claudes metrics                           Stats across all runs\n\
    claudes clean                             Remove worktrees and state\n\
\n\
EXAMPLES:\n\
  # Fix three bugs in parallel, each in its own worktree\n\
  claudes run -p 'fix issue #12' -p 'fix issue #15' -p 'fix issue #20'\n\
\n\
  # Run a manifest with dependency chains\n\
  claudes run --manifest plan.toml\n\
\n\
  # Research pipeline with NDJSON output\n\
  claudes run --manifest research.json --output json | jpx --slurp '[?type == `result`]'\n\
\n\
  # Check what happened\n\
  claudes status --json\n\
\n\
MANIFEST FORMATS:\n\
  JSON (.json) and TOML (.toml) are supported.\n\
  Place claudes.toml in your project root for auto-discovery.\n\
\n\
OUTPUT MODES:\n\
  progress    In-place spinners with live status (default TTY)\n\
  json        Structured NDJSON on stdout (default piped)\n\
  quiet       Exit code only\n\
\n\
  Tracing is orthogonal: RUST_LOG=claudes=debug for tool calls,\n\
  RUST_LOG=claudes=info for task lifecycle.\n\
"
)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Option<Command>,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Generate a manifest without executing.
    Plan(PlanArgs),

    /// Generate a manifest from a prompt using Claude.
    Generate(GenerateArgs),

    /// Generate a manifest template with stub tasks.
    Init(InitArgs),

    /// Execute tasks from a manifest, config, or CLI args.
    Run(RunArgs),

    /// Show status of the most recent run.
    Status(StatusArgs),

    /// Re-run failed or timed-out tasks from a previous run.
    Fix(FixArgs),

    /// Aggregate stats from run history.
    Metrics(MetricsArgs),

    /// Remove worktrees and temporary state.
    Clean(CleanArgs),

    /// Start the MCP server (stdio transport).
    Serve(ServeArgs),
}

/// Arguments for `claudes run`.
#[derive(Debug, Parser)]
#[command(
    long_about = "Execute tasks from a manifest file, ad-hoc prompts, or auto-discovered claudes.toml.\n\n\
        Tasks run in parallel by default, each in an isolated git worktree.\n\
        Use --manifest for pre-built plans, or -p for quick ad-hoc tasks.",
    after_long_help = "\
EXAMPLES:\n\
  # Run from a manifest\n\
  claudes run --manifest plan.json\n\
\n\
  # Ad-hoc parallel tasks\n\
  claudes run -p 'fix the pagination bug' -p 'add unit tests for auth'\n\
\n\
  # Single task with overrides\n\
  claudes run -p 'refactor the parser' --model claude-opus-4-6 --effort high\n\
\n\
  # NDJSON output for piping\n\
  claudes run --manifest plan.json --output json | jpx --stream '[type, task]'\n\
\n\
  # Dry run to preview the manifest\n\
  claudes run -p 'do X' -p 'do Y' --dry-run\n\
"
)]
pub struct RunArgs {
    /// Run from a manifest file (all other generation options ignored).
    #[arg(long)]
    pub manifest: Option<PathBuf>,

    /// Run only the named task(s) from the manifest (repeatable).
    #[arg(long)]
    pub task: Vec<String>,

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

    /// Apply a named profile (from the manifest's profiles map) to all ad-hoc tasks.
    #[arg(long)]
    pub profile: Option<String>,

    /// Show generated manifest without executing.
    #[arg(long)]
    pub dry_run: bool,

    /// Exit code only, no streaming output.
    #[arg(long)]
    pub quiet: bool,

    /// Output mode (progress|json|quiet). Default: progress when TTY, json when piped.
    #[arg(long, default_value = "auto")]
    pub output: String,

    /// Overwrite existing worktrees.
    #[arg(long)]
    pub force: bool,

    /// Auto-cleanup worktrees after run (none|on-success|always; default: none).
    #[arg(long, default_value = "none")]
    pub cleanup: String,

    /// Skill files to inject into the system prompt (repeatable).
    #[arg(long)]
    pub skill: Vec<String>,

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

    /// Apply a named profile (from the manifest's profiles map) to all ad-hoc tasks.
    #[arg(long)]
    pub profile: Option<String>,
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

    /// Remove run state files from .claudes/runs/ and .claudes/latest.
    #[arg(long)]
    pub runs: bool,

    /// Remove local claudes/* branches that have been merged into main.
    #[arg(long)]
    pub branches: bool,
}

/// Arguments for `claudes metrics`.
#[derive(Debug, Parser)]
pub struct MetricsArgs {
    /// Limit to the last N runs.
    #[arg(long)]
    pub last: Option<usize>,

    /// Output as JSON instead of a table.
    #[arg(long)]
    pub json: bool,
}

/// Arguments for `claudes generate`.
#[derive(Debug, Parser)]
pub struct GenerateArgs {
    /// Prompt describing the tasks to generate.
    #[arg(short = 'p', long)]
    pub prompt: String,

    /// Override model used for generation.
    #[arg(long)]
    pub model: Option<String>,

    /// Write manifest to file (default: stdout).
    #[arg(short = 'o', long)]
    pub out: Option<PathBuf>,

    /// Read additional context from stdin.
    #[arg(long)]
    pub stdin: bool,

    /// Skip reading project context (claudes.toml, PROMPTING.md, CLAUDE.md).
    #[arg(long)]
    pub no_context: bool,
}

/// Arguments for `claudes serve`.
#[derive(Debug, Parser)]
pub struct ServeArgs {}

/// Arguments for `claudes fix`.
#[derive(Debug, Parser)]
pub struct FixArgs {
    /// Run ID to fix (default: latest run).
    #[arg(long)]
    pub run: Option<String>,

    /// Re-run only these task(s) (repeatable; default: all failed/timed-out).
    #[arg(long)]
    pub task: Vec<String>,

    /// Additional guidance to append to the fix prompt.
    #[arg(short = 'p')]
    pub prompt: Option<String>,

    /// Force overwrite if worktree state is inconsistent.
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
