use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Parser, Subcommand};
use tracing::info;

/// Autonomous GitHub issue runner — turns issues into pull requests.
#[derive(Debug, Parser)]
#[command(
    name = "claude-runner",
    version,
    about,
    long_about = "Autonomous GitHub issue runner that processes issues through\n\
        configurable workflow templates (plan -> implement -> test -> PR).\n\n\
        Agent-agnostic: uses Claude by default, pluggable for other agents."
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Process a single issue by number.
    Issue(IssueArgs),
    /// Run once — poll for eligible issues and process them.
    Run(RunArgs),
    /// Watch mode — continuously poll and process issues.
    Watch(WatchArgs),
    /// Show run history and status.
    Status(StatusArgs),
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
    config: PathBuf,
    /// Repository directory (default: current directory).
    #[arg(long)]
    repo_dir: Option<PathBuf>,
    /// Dry run — show the plan without executing.
    #[arg(long)]
    dry_run: bool,
}

#[derive(Debug, Parser)]
struct RunArgs {
    /// Repository in owner/name format.
    #[arg(long)]
    repo: String,
    /// Path to policy config file.
    #[arg(long, default_value = "runner.toml")]
    config: PathBuf,
    /// Repository directory.
    #[arg(long)]
    repo_dir: Option<PathBuf>,
}

#[derive(Debug, Parser)]
struct WatchArgs {
    /// Repository in owner/name format.
    #[arg(long)]
    repo: String,
    /// Path to policy config file.
    #[arg(long, default_value = "runner.toml")]
    config: PathBuf,
    /// Poll interval in seconds.
    #[arg(long, default_value = "300")]
    interval: u64,
    /// Repository directory.
    #[arg(long)]
    repo_dir: Option<PathBuf>,
}

#[derive(Debug, Parser)]
struct StatusArgs {
    /// Show a specific run by ID.
    #[arg(long)]
    run_id: Option<String>,
    /// State directory.
    #[arg(long)]
    state_dir: Option<PathBuf>,
}

fn state_dir() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".claude-runner")
        .join("runs")
}

fn repo_dir(arg: &Option<PathBuf>) -> PathBuf {
    arg.clone()
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")))
}

#[tokio::main]
async fn main() -> ExitCode {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("claude_runner=info")),
        )
        .init();

    let cli = Cli::parse();

    match cli.command {
        Command::Issue(args) => cmd_issue(args).await,
        Command::Run(args) => cmd_run(args).await,
        Command::Watch(args) => cmd_watch(args).await,
        Command::Status(args) => cmd_status(args),
    }
}

async fn cmd_issue(args: IssueArgs) -> ExitCode {
    let policy = match claude_runner::policy::RepoPolicy::from_file(&args.config) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("error loading config: {e}");
            return ExitCode::FAILURE;
        }
    };

    let rd = repo_dir(&args.repo_dir);
    let sd = state_dir();

    if args.dry_run {
        // Fetch and show the plan without executing.
        let issue = match claude_runner::github::fetch_issue(&args.repo, args.number).await {
            Ok(i) => i,
            Err(e) => {
                eprintln!("error: {e}");
                return ExitCode::FAILURE;
            }
        };
        let template = claude_runner::workflow::select_workflow(&issue, &policy);
        let branch = policy.branch_for_issue(&issue);
        let plan = claude_runner::planner::create_plan(&issue, &template, &branch);

        println!("Issue: #{} — {}", issue.number, issue.title);
        println!("Workflow: {}", template.name);
        println!("Branch: {branch}");
        println!("Stages:");
        for (i, stage) in plan.stages.iter().enumerate() {
            let optional = if stage.optional { " (optional)" } else { "" };
            println!("  {}. {}{optional}", i + 1, stage.kind_name());
        }
        return ExitCode::SUCCESS;
    }

    match claude_runner::process_issue(&args.repo, args.number, &policy, &sd, &rd).await {
        Ok(record) => {
            println!();
            println!(
                "Run {} — {} (issue #{})",
                record.run_id,
                record.status_text(),
                record.issue_number
            );
            for stage in &record.stages {
                let cost = stage
                    .result
                    .as_ref()
                    .and_then(|r| r.cost_usd)
                    .map(|c| format!("  ${c:.2}"))
                    .unwrap_or_default();
                let duration = stage
                    .result
                    .as_ref()
                    .map(|r| format!("  {:.0}s", r.duration_secs))
                    .unwrap_or_default();
                println!(
                    "  {:<15} {:?}{duration}{cost}",
                    stage.kind_name(),
                    stage.status
                );
            }
            if let Some(pr) = record.pr_number {
                println!("PR: #{pr}");
            }
            if let Some(cost) = record.total_cost_usd {
                println!("Total cost: ${cost:.2}");
            }
            if record.status == claude_runner::state::RunStatus::Succeeded {
                ExitCode::SUCCESS
            } else {
                ExitCode::FAILURE
            }
        }
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::FAILURE
        }
    }
}

async fn cmd_run(args: RunArgs) -> ExitCode {
    let policy = match claude_runner::policy::RepoPolicy::from_file(&args.config) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("error loading config: {e}");
            return ExitCode::FAILURE;
        }
    };

    let rd = repo_dir(&args.repo_dir);
    let sd = state_dir();

    let records = claude_runner::process_batch(&args.repo, &policy, &sd, &rd).await;

    println!();
    println!(
        "Processed {} issues: {} succeeded, {} failed",
        records.len(),
        records
            .iter()
            .filter(|r| r.status == claude_runner::state::RunStatus::Succeeded)
            .count(),
        records
            .iter()
            .filter(|r| r.status == claude_runner::state::RunStatus::Failed)
            .count(),
    );

    if records
        .iter()
        .all(|r| r.status == claude_runner::state::RunStatus::Succeeded)
    {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}

async fn cmd_watch(args: WatchArgs) -> ExitCode {
    let policy = match claude_runner::policy::RepoPolicy::from_file(&args.config) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("error loading config: {e}");
            return ExitCode::FAILURE;
        }
    };

    let rd = repo_dir(&args.repo_dir);
    let sd = state_dir();
    let interval = std::time::Duration::from_secs(args.interval);

    info!(
        repo = args.repo,
        interval_secs = args.interval,
        "starting watch mode"
    );

    loop {
        let records = claude_runner::process_batch(&args.repo, &policy, &sd, &rd).await;
        if !records.is_empty() {
            info!(
                processed = records.len(),
                succeeded = records
                    .iter()
                    .filter(|r| r.status == claude_runner::state::RunStatus::Succeeded)
                    .count(),
                "batch complete"
            );
        }
        tokio::time::sleep(interval).await;
    }
}

fn cmd_status(args: StatusArgs) -> ExitCode {
    let sd = args.state_dir.unwrap_or_else(state_dir);

    if let Some(ref run_id) = args.run_id {
        match claude_runner::state::load_run(run_id, &sd) {
            Some(record) => {
                println!("{}", serde_json::to_string_pretty(&record).unwrap());
                ExitCode::SUCCESS
            }
            None => {
                eprintln!("run not found: {run_id}");
                ExitCode::FAILURE
            }
        }
    } else {
        match claude_runner::state::load_latest(&sd) {
            Some(record) => {
                println!("{}", serde_json::to_string_pretty(&record).unwrap());
                ExitCode::SUCCESS
            }
            None => {
                eprintln!("no runs found");
                ExitCode::FAILURE
            }
        }
    }
}
