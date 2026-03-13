use anyhow::Result;
use clap::Parser;
use std::path::PathBuf;

mod context;
mod decisioner;
mod run;

/// Multi-agent orchestrated CLI for Claude Code.
///
/// With arguments: runs a one-shot task (single or multi-agent).
/// Without arguments: launches an interactive coordinator session (planned).
#[derive(Parser, Debug)]
#[command(name = "claudes", version, about)]
struct Cli {
    /// The task to execute. Omit for interactive coordinator mode.
    prompt: Option<String>,

    /// Show the execution plan without running it.
    #[arg(long)]
    plan: bool,

    /// Run multiple prompts in parallel (one claude per prompt).
    #[arg(long, num_args = 2.., value_delimiter = ',')]
    parallel: Option<Vec<String>>,

    /// Run prompts as a sequential chain (output of each feeds the next).
    #[arg(long, num_args = 2.., value_delimiter = ',')]
    chain: Option<Vec<String>>,

    /// Maximum budget in USD for the entire run.
    #[arg(long)]
    max_budget: Option<f64>,

    /// Model to use (e.g., "claude-sonnet-4-20250514").
    #[arg(long, short)]
    model: Option<String>,

    /// Number of parallel slots (default: number of tasks, max 10).
    #[arg(long)]
    slots: Option<usize>,

    /// Working directory for claude sessions.
    #[arg(long, short = 'C')]
    working_dir: Option<PathBuf>,

    /// Decisioner strategy: auto, conservative, balanced, aggressive.
    #[arg(long, short, default_value = "auto")]
    strategy: decisioner::Strategy,

    /// Enable verbose output.
    #[arg(long, short)]
    verbose: bool,
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    // Initialize tracing based on verbosity.
    if cli.verbose {
        tracing_subscriber::fmt()
            .with_env_filter("claudes=debug,claude_pool=debug,claude_wrapper=debug")
            .init();
    } else {
        tracing_subscriber::fmt()
            .with_env_filter("claudes=info")
            .init();
    }

    // Determine execution mode.
    // Explicit modes (--parallel, --chain) bypass the decisioner.
    // Default prompt mode uses the decisioner to pick strategy.
    if let Some(ref prompts) = cli.parallel {
        run::parallel(&cli, prompts).await
    } else if let Some(ref steps) = cli.chain {
        run::chain(&cli, steps).await
    } else if let Some(ref prompt) = cli.prompt {
        if cli.plan {
            run::plan(&cli, prompt).await
        } else {
            run::auto(&cli, prompt).await
        }
    } else {
        // No args: interactive coordinator mode (planned).
        eprintln!("Interactive coordinator mode is not yet implemented.");
        eprintln!("Usage: claudes \"your task here\"");
        eprintln!("       claudes --parallel \"task1\" \"task2\" \"task3\"");
        eprintln!("       claudes --chain \"step1\" \"step2\" \"step3\"");
        std::process::exit(1);
    }
}
