use std::path::PathBuf;
use std::process::ExitCode;

use clap::Parser;
use tracing_subscriber::EnvFilter;

use claudes::cli::{Cli, Command, parse_timeout};
use claudes::output::{self, OutputFormat, Verbosity};
use claudes::planner::PlanOptions;

#[tokio::main]
async fn main() -> ExitCode {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .init();

    let cli = Cli::parse();

    match cli.command {
        Command::Run(args) => cmd_run(args).await,
        Command::Plan(args) => cmd_plan(args).await,
        Command::Init(args) => cmd_init(args).await,
        Command::Status(args) => cmd_status(args).await,
        Command::Clean(args) => cmd_clean(args).await,
    }
}

async fn cmd_run(args: claudes::cli::RunArgs) -> ExitCode {
    let project_dir = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));

    // If --manifest is provided, load and run it directly.
    if let Some(manifest_path) = &args.manifest {
        let content = match std::fs::read_to_string(manifest_path) {
            Ok(c) => c,
            Err(e) => {
                eprintln!("error: cannot read manifest: {e}");
                return ExitCode::FAILURE;
            }
        };
        let manifest: claudes::Manifest = match serde_json::from_str(&content) {
            Ok(m) => m,
            Err(e) => {
                eprintln!("error: invalid manifest JSON: {e}");
                return ExitCode::FAILURE;
            }
        };

        let options = claudes::RunOptions {
            project_dir,
            force: args.force,
            binary: None,
            env: vec![],
            cleanup: parse_cleanup(&args.cleanup),
            event_sender: None,
        };

        if args.dry_run {
            println!("{}", serde_json::to_string_pretty(&manifest).unwrap());
            return ExitCode::SUCCESS;
        }

        return execute_manifest(&manifest, &options, &args).await;
    }

    // Otherwise, generate a manifest from CLI args or auto-discover one.
    let prompts = collect_prompts(&args.prompt, args.stdin).await;
    if prompts.is_empty() {
        // No -p prompts — try auto-discovering a project manifest.
        if let Some(manifest_path) = claudes::manifest::Manifest::discover(&project_dir) {
            let content = match std::fs::read_to_string(&manifest_path) {
                Ok(c) => c,
                Err(e) => {
                    eprintln!("error: cannot read manifest: {e}");
                    return ExitCode::FAILURE;
                }
            };
            let manifest: claudes::Manifest =
                if manifest_path.extension().and_then(|e| e.to_str()) == Some("toml") {
                    match toml::from_str(&content) {
                        Ok(m) => m,
                        Err(e) => {
                            eprintln!("error: invalid manifest TOML: {e}");
                            return ExitCode::FAILURE;
                        }
                    }
                } else {
                    match serde_json::from_str(&content) {
                        Ok(m) => m,
                        Err(e) => {
                            eprintln!("error: invalid manifest JSON: {e}");
                            return ExitCode::FAILURE;
                        }
                    }
                };

            let options = claudes::RunOptions {
                project_dir,
                force: args.force,
                binary: None,
                env: vec![],
                cleanup: parse_cleanup(&args.cleanup),
                event_sender: None,
            };

            if args.dry_run {
                println!("{}", serde_json::to_string_pretty(&manifest).unwrap());
                return ExitCode::SUCCESS;
            }

            return execute_manifest(&manifest, &options, &args).await;
        }

        eprintln!(
            "error: no prompts provided and no manifest file found \
             (use -p, --manifest, or create claudes.toml)"
        );
        return ExitCode::FAILURE;
    }

    let plan_opts = build_plan_options(
        prompts,
        args.model.as_deref(),
        args.timeout.as_deref(),
        args.max_turns,
        args.max_budget_usd,
        args.effort.as_deref(),
        args.permission_mode.as_deref(),
        args.allowed_tools.as_deref(),
        args.disallowed_tools.as_deref(),
        args.append_system_prompt.as_deref(),
        args.isolation.as_deref(),
    );

    let manifest = claudes::plan(&plan_opts);

    if args.dry_run {
        println!("{}", serde_json::to_string_pretty(&manifest).unwrap());
        return ExitCode::SUCCESS;
    }

    let options = claudes::RunOptions {
        project_dir,
        force: args.force,
        binary: None,
        env: vec![],
        cleanup: claudes::CleanupPolicy::default(),
        event_sender: None,
    };

    execute_manifest(&manifest, &options, &args).await
}

async fn execute_manifest(
    manifest: &claudes::Manifest,
    options: &claudes::RunOptions,
    args: &claudes::cli::RunArgs,
) -> ExitCode {
    let format = match args.output.as_str() {
        "json" => OutputFormat::Json,
        _ if args.quiet => OutputFormat::Quiet,
        _ => OutputFormat::Text,
    };

    let verbosity = Verbosity::from(args.verbose);
    let started_at = chrono::Utc::now();

    // Set up streaming if we're in text mode.
    let mut options = options.clone();
    let stream_handle = if format == OutputFormat::Text {
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        options.event_sender = Some(tx);
        Some(tokio::spawn(output::render_stream(rx, verbosity)))
    } else {
        None
    };

    let result = match claudes::run(manifest, &options).await {
        Ok(result) => result,
        Err(e) => {
            eprintln!("error: {e}");
            return ExitCode::FAILURE;
        }
    };

    // Drop the sender so the renderer finishes.
    options.event_sender = None;
    if let Some(handle) = stream_handle {
        let _ = handle.await;
    }

    // Write state file.
    let state = claudes::state::build_state(manifest, &result, started_at);
    if let Err(e) = claudes::state::save(&options.project_dir, &state) {
        tracing::warn!("failed to write state file: {e}");
    }

    output::print_summary(&result, format);
    if result.all_succeeded() {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}

async fn cmd_plan(args: claudes::cli::PlanArgs) -> ExitCode {
    let prompts = collect_prompts(&args.prompt, args.stdin).await;
    if prompts.is_empty() {
        eprintln!("error: no prompts provided (use -p or --stdin)");
        return ExitCode::FAILURE;
    }

    let plan_opts = build_plan_options(
        prompts,
        args.model.as_deref(),
        args.timeout.as_deref(),
        args.max_turns,
        args.max_budget_usd,
        args.effort.as_deref(),
        args.permission_mode.as_deref(),
        args.allowed_tools.as_deref(),
        args.disallowed_tools.as_deref(),
        args.append_system_prompt.as_deref(),
        args.isolation.as_deref(),
    );

    let manifest = claudes::plan(&plan_opts);
    let json = serde_json::to_string_pretty(&manifest).unwrap();

    if let Some(out_path) = &args.out {
        if let Err(e) = std::fs::write(out_path, &json) {
            eprintln!("error: cannot write manifest: {e}");
            return ExitCode::FAILURE;
        }
        eprintln!("manifest written to {}", out_path.display());
    } else {
        println!("{json}");
    }

    ExitCode::SUCCESS
}

async fn cmd_init(args: claudes::cli::InitArgs) -> ExitCode {
    let isolation = args.isolation.as_deref().map(|s| match s {
        "none" => claudes::Isolation::None,
        "clone" => claudes::Isolation::Clone {
            base_dir: ".worktrees".into(),
        },
        _ => claudes::Isolation::Worktree {
            base_dir: ".worktrees".into(),
        },
    });

    let tasks: Vec<claudes::Task> = (1..=args.tasks)
        .map(|i| {
            let mut task = claudes::Task::new(
                format!("task-{i}"),
                "TODO: describe what this task should do",
            );
            task.model = args.model.clone();
            task.isolation = isolation.clone();
            task
        })
        .collect();

    let manifest = claudes::Manifest::new(tasks);
    let json = serde_json::to_string_pretty(&manifest).unwrap();

    if let Some(out_path) = &args.out {
        if let Err(e) = std::fs::write(out_path, &json) {
            eprintln!("error: cannot write manifest: {e}");
            return ExitCode::FAILURE;
        }
        eprintln!("manifest written to {}", out_path.display());
    } else {
        println!("{json}");
    }

    ExitCode::SUCCESS
}

async fn cmd_status(args: claudes::cli::StatusArgs) -> ExitCode {
    let project_dir = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));

    if args.list {
        let runs = claudes::state::list_runs(&project_dir);
        claudes::state::print_status_list(&runs);
        return ExitCode::SUCCESS;
    }

    let state = if let Some(run_id) = &args.run_id {
        claudes::state::load_run(&project_dir, run_id)
    } else {
        claudes::state::load(&project_dir)
    };

    match state {
        Some(state) => {
            if args.json {
                claudes::state::print_status_json(&state);
            } else {
                claudes::state::print_status(&state);
            }
            ExitCode::SUCCESS
        }
        None => {
            eprintln!("no state file found (run `claudes run` first)");
            ExitCode::FAILURE
        }
    }
}

async fn cmd_clean(args: claudes::cli::CleanArgs) -> ExitCode {
    let project_dir = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let worktrees_dir = project_dir.join(".worktrees");

    if !worktrees_dir.exists() {
        eprintln!("no worktrees to clean");
        return ExitCode::SUCCESS;
    }

    let mut removed = 0;
    let entries = match std::fs::read_dir(&worktrees_dir) {
        Ok(e) => e,
        Err(e) => {
            eprintln!("error reading .worktrees: {e}");
            return ExitCode::FAILURE;
        }
    };

    for entry in entries.flatten() {
        if !entry.path().is_dir() {
            continue;
        }

        let name = entry.file_name().to_string_lossy().to_string();
        let mut cmd_args = vec!["worktree", "remove"];
        if args.force {
            cmd_args.push("--force");
        }
        let path_str = entry.path().to_string_lossy().to_string();
        cmd_args.push(&path_str);

        let output = tokio::process::Command::new("git")
            .args(&cmd_args)
            .current_dir(&project_dir)
            .output()
            .await;

        match output {
            Ok(o) if o.status.success() => {
                eprintln!("removed: {name}");
                removed += 1;
            }
            Ok(o) => {
                let stderr = String::from_utf8_lossy(&o.stderr);
                eprintln!("failed to remove {name}: {stderr}");
            }
            Err(e) => {
                eprintln!("failed to remove {name}: {e}");
            }
        }
    }

    // Remove the base dir if empty.
    if removed > 0 {
        let _ = std::fs::remove_dir(&worktrees_dir);
    }

    eprintln!("cleaned {removed} worktree(s)");
    ExitCode::SUCCESS
}

async fn collect_prompts(cli_prompts: &[String], read_stdin: bool) -> Vec<String> {
    let mut prompts: Vec<String> = cli_prompts.to_vec();

    if read_stdin {
        let mut input = String::new();
        if let Ok(n) = std::io::Read::read_to_string(&mut std::io::stdin(), &mut input)
            && n > 0
        {
            let trimmed = input.trim().to_string();
            if !trimmed.is_empty() {
                prompts.push(trimmed);
            }
        }
    }

    prompts
}

#[allow(clippy::too_many_arguments)]
fn build_plan_options(
    prompts: Vec<String>,
    model: Option<&str>,
    timeout: Option<&str>,
    max_turns: Option<u32>,
    max_budget_usd: Option<f64>,
    effort: Option<&str>,
    permission_mode: Option<&str>,
    allowed_tools: Option<&str>,
    disallowed_tools: Option<&str>,
    append_system_prompt: Option<&str>,
    isolation: Option<&str>,
) -> PlanOptions {
    let timeout_secs = timeout.and_then(|t| match parse_timeout(t) {
        Ok(s) => Some(s),
        Err(e) => {
            eprintln!("warning: {e}, ignoring timeout");
            None
        }
    });

    PlanOptions {
        prompts,
        model: model.map(String::from),
        timeout_secs,
        max_turns,
        max_budget_usd,
        effort: effort.map(String::from),
        permission_mode: permission_mode.map(String::from),
        allowed_tools: allowed_tools.map(|s| s.split(',').map(|t| t.trim().to_string()).collect()),
        disallowed_tools: disallowed_tools
            .map(|s| s.split(',').map(|t| t.trim().to_string()).collect()),
        append_system_prompt: append_system_prompt.map(String::from),
        isolation: isolation.map(String::from),
        ..Default::default()
    }
}

fn parse_cleanup(s: &str) -> claudes::CleanupPolicy {
    match s {
        "on-success" => claudes::CleanupPolicy::OnSuccess,
        "always" => claudes::CleanupPolicy::Always,
        _ => claudes::CleanupPolicy::None,
    }
}
