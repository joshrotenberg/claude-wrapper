use std::process::ExitCode;

use clap::{Parser, Subcommand};

/// Autonomous GitHub issue runner — turns issues into pull requests.
#[derive(Debug, Parser)]
#[command(name = "claude-runner", version, about)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Run once — poll for eligible issues and process them.
    Run(RunArgs),
    /// Watch mode — continuously poll and process issues.
    Watch(WatchArgs),
    /// Process a single issue by number.
    Issue(IssueArgs),
    /// Show run history and status.
    Status(StatusArgs),
}

#[derive(Debug, Parser)]
struct RunArgs {
    /// Repository in owner/name format.
    #[arg(long)]
    repo: String,
    /// Path to policy config file.
    #[arg(long, default_value = "runner.toml")]
    config: String,
    /// Dry run — show what would be done without doing it.
    #[arg(long)]
    dry_run: bool,
}

#[derive(Debug, Parser)]
struct WatchArgs {
    /// Repository in owner/name format.
    #[arg(long)]
    repo: String,
    /// Path to policy config file.
    #[arg(long, default_value = "runner.toml")]
    config: String,
    /// Poll interval in seconds.
    #[arg(long, default_value = "300")]
    interval: u64,
}

#[derive(Debug, Parser)]
struct IssueArgs {
    /// Repository in owner/name format.
    #[arg(long)]
    repo: String,
    /// Issue number to process.
    #[arg(long)]
    number: u64,
    /// Path to policy config file.
    #[arg(long, default_value = "runner.toml")]
    config: String,
}

#[derive(Debug, Parser)]
struct StatusArgs {
    /// Repository in owner/name format.
    #[arg(long)]
    repo: Option<String>,
}

#[tokio::main]
async fn main() -> ExitCode {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();

    let cli = Cli::parse();

    match cli.command {
        Command::Run(_args) => {
            eprintln!("run: not yet implemented");
            ExitCode::FAILURE
        }
        Command::Watch(_args) => {
            eprintln!("watch: not yet implemented");
            ExitCode::FAILURE
        }
        Command::Issue(_args) => {
            eprintln!("issue: not yet implemented");
            ExitCode::FAILURE
        }
        Command::Status(_args) => {
            eprintln!("status: not yet implemented");
            ExitCode::FAILURE
        }
    }
}
