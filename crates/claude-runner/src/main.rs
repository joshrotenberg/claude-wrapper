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
    /// Path to config file.
    #[arg(long, short, default_value = "runner.toml", global = true)]
    config: PathBuf,

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
    /// Issue number to process.
    number: u64,
    /// Repository directory (default: current directory).
    #[arg(long)]
    repo_dir: Option<PathBuf>,
    /// Dry run — show the plan without executing.
    #[arg(long)]
    dry_run: bool,
}

#[derive(Debug, Parser)]
struct RunArgs {
    /// Repository directory.
    #[arg(long)]
    repo_dir: Option<PathBuf>,
}

#[derive(Debug, Parser)]
struct WatchArgs {
    /// Override poll interval in seconds (uses per-route intervals by default).
    #[arg(long)]
    interval: Option<u64>,
    /// Only run a specific route.
    #[arg(long)]
    route: Option<String>,
    /// Repository directory.
    #[arg(long)]
    repo_dir: Option<PathBuf>,
}

#[derive(Debug, Parser)]
struct StatusArgs {
    /// Show a specific run by ID.
    #[arg(long)]
    run_id: Option<String>,
}

fn state_dir() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".claude-runner")
        .join("runs")
}

fn repo_dir(arg: &Option<PathBuf>, config: &claude_runner::RunnerConfig) -> PathBuf {
    arg.clone()
        .or_else(|| config.global.repo_dir.as_ref().map(PathBuf::from))
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")))
}

fn load_config(path: &std::path::Path) -> Result<claude_runner::RunnerConfig, ExitCode> {
    match claude_runner::RunnerConfig::from_file(path) {
        Ok(c) => Ok(c),
        Err(e) => {
            eprintln!("error loading config {}: {e}", path.display());
            Err(ExitCode::FAILURE)
        }
    }
}

#[tokio::main]
async fn main() -> ExitCode {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();

    let cli = Cli::parse();
    let config = match load_config(&cli.config) {
        Ok(c) => c,
        Err(code) => return code,
    };

    match cli.command {
        Command::Issue(args) => cmd_issue(args, &config).await,
        Command::Run(args) => cmd_run(args, &config).await,
        Command::Watch(args) => cmd_watch(args, &config).await,
        Command::Status(args) => cmd_status(args),
    }
}

async fn cmd_issue(args: IssueArgs, config: &claude_runner::RunnerConfig) -> ExitCode {
    let rd = repo_dir(&args.repo_dir, config);
    let sd = state_dir();

    if args.dry_run {
        let issue = match claude_runner::github::fetch_issue(&config.global.repo, args.number).await
        {
            Ok(i) => i,
            Err(e) => {
                eprintln!("error: {e}");
                return ExitCode::FAILURE;
            }
        };

        // Match route.
        let (route_name, route) = match config.match_route(&issue) {
            Some(r) => r,
            None => {
                eprintln!(
                    "no route matches issue #{} (labels: {:?})",
                    issue.number, issue.labels
                );
                return ExitCode::FAILURE;
            }
        };

        let template = match config.resolve_workflow(&route.workflow) {
            Some(t) => t,
            None => {
                eprintln!("unknown workflow: {}", route.workflow);
                return ExitCode::FAILURE;
            }
        };

        let branch = config.branch_for_issue(&issue);
        let plan = claude_runner::planner::create_plan(&issue, &template, &branch, None);

        println!("Issue:    #{} — {}", issue.number, issue.title);
        println!("Route:    {route_name}");
        println!("Workflow: {}", template.name);
        println!("Branch:   {branch}");
        if let Some(model) = config.effective_model(route) {
            println!("Model:    {model}");
        }
        println!("Stages:");
        for (i, stage) in plan.stages.iter().enumerate() {
            let optional = if stage.optional { " (optional)" } else { "" };
            println!("  {}. {}{optional}", i + 1, stage.kind_name());
        }
        if let Some(est) = claude_runner::state::estimate_cost(&template.name, &sd) {
            println!(
                "Estimated cost: ${:.2} - ${:.2} (avg ${:.2}, based on {} previous {} runs)",
                est.min, est.max, est.avg, est.count, est.workflow
            );
        }
        return ExitCode::SUCCESS;
    }

    match claude_runner::orchestrator::process_issue_with_config(args.number, config, &sd, &rd)
        .await
    {
        Ok(record) => print_run_result(&record),
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::FAILURE
        }
    }
}

async fn cmd_run(args: RunArgs, config: &claude_runner::RunnerConfig) -> ExitCode {
    let rd = repo_dir(&args.repo_dir, config);
    let sd = state_dir();

    let records = claude_runner::orchestrator::process_batch_with_config(config, &sd, &rd).await;

    println!();
    let succeeded = records
        .iter()
        .filter(|r| r.status == claude_runner::state::RunStatus::Succeeded)
        .count();
    let failed = records
        .iter()
        .filter(|r| r.status == claude_runner::state::RunStatus::Failed)
        .count();
    println!(
        "Processed {} issues: {succeeded} succeeded, {failed} failed",
        records.len()
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

async fn cmd_watch(args: WatchArgs, config: &claude_runner::RunnerConfig) -> ExitCode {
    let rd = repo_dir(&args.repo_dir, config);
    let sd = state_dir();

    // Use the override interval or the minimum interval across matched routes.
    let interval = args.interval.unwrap_or(60);
    let interval_dur = std::time::Duration::from_secs(interval);

    info!(
        repo = config.global.repo,
        interval_secs = interval,
        routes = config.routes.len(),
        "starting watch mode"
    );

    loop {
        let records =
            claude_runner::orchestrator::process_batch_with_config(config, &sd, &rd).await;
        if !records.is_empty() {
            let succeeded = records
                .iter()
                .filter(|r| r.status == claude_runner::state::RunStatus::Succeeded)
                .count();
            info!(
                processed = records.len(),
                succeeded = succeeded,
                "batch complete"
            );
        }
        tokio::time::sleep(interval_dur).await;
    }
}

fn cmd_status(args: StatusArgs) -> ExitCode {
    let sd = state_dir();

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
            Some(record) => print_run_result(&record),
            None => {
                eprintln!("no runs found");
                ExitCode::FAILURE
            }
        }
    }
}

fn print_run_result(record: &claude_runner::state::RunRecord) -> ExitCode {
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
