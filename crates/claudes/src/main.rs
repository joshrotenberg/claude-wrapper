use std::path::{Path, PathBuf};
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
        Command::Fix(args) => cmd_fix(args).await,
    }
}

async fn cmd_run(args: claudes::cli::RunArgs) -> ExitCode {
    let project_dir = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let global = claudes::manifest::load_global_defaults();

    // If --manifest is provided, load and run it directly.
    if let Some(manifest_path) = &args.manifest {
        let content = match std::fs::read_to_string(manifest_path) {
            Ok(c) => c,
            Err(e) => {
                eprintln!("error: cannot read manifest: {e}");
                return ExitCode::FAILURE;
            }
        };
        let mut manifest: claudes::Manifest = match serde_json::from_str(&content) {
            Ok(m) => m,
            Err(e) => {
                eprintln!("error: invalid manifest JSON: {e}");
                return ExitCode::FAILURE;
            }
        };
        if let Some(ref g) = global {
            manifest.apply_global_defaults(g);
        }

        let manifest_dir = manifest_path.parent().unwrap_or_else(|| Path::new("."));
        if let Err(e) = manifest.resolve_files(manifest_dir) {
            eprintln!("error: {e}");
            return ExitCode::FAILURE;
        }

        if let Err(e) = filter_tasks(&mut manifest, &args.task) {
            eprintln!("error: {e}");
            return ExitCode::FAILURE;
        }

        for warning in manifest.check_file_overlaps() {
            eprintln!("warning: {warning}");
        }

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
            let mut manifest: claudes::Manifest =
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
            if let Some(ref g) = global {
                manifest.apply_global_defaults(g);
            }

            let manifest_dir = manifest_path.parent().unwrap_or_else(|| Path::new("."));
            if let Err(e) = manifest.resolve_files(manifest_dir) {
                eprintln!("error: {e}");
                return ExitCode::FAILURE;
            }

            if let Err(e) = filter_tasks(&mut manifest, &args.task) {
                eprintln!("error: {e}");
                return ExitCode::FAILURE;
            }

            for warning in manifest.check_file_overlaps() {
                eprintln!("warning: {warning}");
            }

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
        args.profile.as_deref(),
    );

    let mut manifest = claudes::plan(&plan_opts);
    if let Some(ref g) = global {
        manifest.apply_global_defaults(g);
    }

    for warning in manifest.check_file_overlaps() {
        eprintln!("warning: {warning}");
    }

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
        Some(tokio::spawn(output::render_stream(
            rx,
            verbosity,
            args.no_color,
        )))
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
        args.profile.as_deref(),
    );

    let mut manifest = claudes::plan(&plan_opts);
    if let Some(ref g) = claudes::manifest::load_global_defaults() {
        manifest.apply_global_defaults(g);
    }
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

    let clean_worktrees = true; // always clean worktrees by default
    let clean_runs = args.runs || args.all;
    let clean_branches = args.branches || args.all;

    if clean_worktrees && clean_worktrees_impl(&project_dir, args.force).await == ExitCode::FAILURE
    {
        return ExitCode::FAILURE;
    }

    if clean_runs && clean_runs_impl(&project_dir) == ExitCode::FAILURE {
        return ExitCode::FAILURE;
    }

    if clean_branches && clean_branches_impl(&project_dir).await == ExitCode::FAILURE {
        return ExitCode::FAILURE;
    }

    ExitCode::SUCCESS
}

async fn cmd_fix(args: claudes::cli::FixArgs) -> ExitCode {
    let project_dir = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));

    let state = if let Some(ref run_id) = args.run {
        claudes::state::load_run(&project_dir, run_id)
    } else {
        claudes::state::load(&project_dir)
    };

    let state = match state {
        Some(s) => s,
        None => {
            eprintln!("error: no run state found (run `claudes run` first)");
            return ExitCode::FAILURE;
        }
    };

    let tasks_to_fix: Vec<&claudes::state::TaskState> = state
        .results
        .iter()
        .filter(|t| {
            if args.task.is_empty() {
                matches!(
                    t.status,
                    claudes::state::TaskStatus::Failed | claudes::state::TaskStatus::Timeout
                )
            } else {
                args.task.contains(&t.name)
            }
        })
        .collect();

    if tasks_to_fix.is_empty() {
        eprintln!("no failed or timed-out tasks to fix");
        return ExitCode::SUCCESS;
    }

    let mut all_succeeded = true;

    for task_state in tasks_to_fix {
        eprintln!("fixing task: {}", task_state.name);

        let original_task = state
            .manifest
            .tasks
            .iter()
            .find(|t| t.name == task_state.name);

        let original_prompt = original_task
            .map(|t| t.prompt.as_str())
            .unwrap_or("[unknown]");

        let error_context = task_state
            .error
            .as_deref()
            .unwrap_or("task timed out or failed with no error output");

        let mut fix_prompt = format!(
            "The previous task failed. Original prompt: {original_prompt}. \
             Error: {error_context}. Fix the issue."
        );
        if let Some(ref guidance) = args.prompt {
            fix_prompt.push_str(&format!(" {guidance}"));
        }

        let mut fix_task = original_task
            .cloned()
            .unwrap_or_else(|| claudes::Task::new(&task_state.name, ""));
        fix_task.prompt = fix_prompt;
        fix_task.isolation = Some(claudes::Isolation::None);

        let fix_manifest = claudes::Manifest::new(vec![fix_task]);

        let work_dir = PathBuf::from(&task_state.work_dir);
        if !work_dir.exists() {
            eprintln!(
                "error: work_dir for task '{}' no longer exists: {}",
                task_state.name, task_state.work_dir
            );
            all_succeeded = false;
            continue;
        }

        let started_at = chrono::Utc::now();

        let mut options = claudes::RunOptions {
            project_dir: work_dir,
            force: args.force,
            binary: None,
            env: vec![],
            cleanup: claudes::CleanupPolicy::default(),
            event_sender: None,
        };

        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        options.event_sender = Some(tx);
        let stream_handle =
            tokio::spawn(output::render_stream(rx, output::Verbosity::Default, false));

        let result = match claudes::run(&fix_manifest, &options).await {
            Ok(r) => r,
            Err(e) => {
                eprintln!("error: {e}");
                all_succeeded = false;
                continue;
            }
        };

        options.event_sender = None;
        let _ = stream_handle.await;

        let fix_state = claudes::state::build_state(&fix_manifest, &result, started_at);
        if let Err(e) = claudes::state::save(&project_dir, &fix_state) {
            tracing::warn!("failed to write fix state file: {e}");
        }

        output::print_summary(&result, output::OutputFormat::Text);

        if !result.all_succeeded() {
            all_succeeded = false;
        }
    }

    if all_succeeded {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}

async fn clean_worktrees_impl(project_dir: &Path, force: bool) -> ExitCode {
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
        if force {
            cmd_args.push("--force");
        }
        let path_str = entry.path().to_string_lossy().to_string();
        cmd_args.push(&path_str);

        let output = tokio::process::Command::new("git")
            .args(&cmd_args)
            .current_dir(project_dir)
            .output()
            .await;

        match output {
            Ok(o) if o.status.success() => {
                eprintln!("removed worktree: {name}");
                removed += 1;
            }
            Ok(o) => {
                let stderr = String::from_utf8_lossy(&o.stderr);
                eprintln!("failed to remove worktree {name}: {stderr}");
            }
            Err(e) => {
                eprintln!("failed to remove worktree {name}: {e}");
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

fn clean_runs_impl(project_dir: &Path) -> ExitCode {
    let runs_dir = project_dir.join(".claudes").join("runs");
    let latest_file = project_dir.join(".claudes").join("latest");
    let mut removed = 0;

    if runs_dir.exists() {
        let entries = match std::fs::read_dir(&runs_dir) {
            Ok(e) => e,
            Err(e) => {
                eprintln!("error reading .claudes/runs: {e}");
                return ExitCode::FAILURE;
            }
        };
        for entry in entries.flatten() {
            if let Err(e) = std::fs::remove_file(entry.path()) {
                eprintln!(
                    "failed to remove {}: {e}",
                    entry.file_name().to_string_lossy()
                );
            } else {
                removed += 1;
            }
        }
    }

    if latest_file.exists()
        && let Err(e) = std::fs::remove_file(&latest_file)
    {
        eprintln!("failed to remove .claudes/latest: {e}");
    }

    eprintln!("cleaned {removed} run state file(s)");
    ExitCode::SUCCESS
}

async fn clean_branches_impl(project_dir: &Path) -> ExitCode {
    // List local branches matching claudes/*
    let list_output = tokio::process::Command::new("git")
        .args(["branch", "--list", "claudes/*"])
        .current_dir(project_dir)
        .output()
        .await;

    let list_output = match list_output {
        Ok(o) => o,
        Err(e) => {
            eprintln!("failed to list branches: {e}");
            return ExitCode::FAILURE;
        }
    };

    let branches_raw = String::from_utf8_lossy(&list_output.stdout);
    let branches: Vec<&str> = branches_raw
        .lines()
        .map(|l| l.trim().trim_start_matches("* ").trim())
        .filter(|l| !l.is_empty())
        .collect();

    if branches.is_empty() {
        eprintln!("no claudes/* branches to clean");
        return ExitCode::SUCCESS;
    }

    // Get list of branches merged into main.
    let merged_output = tokio::process::Command::new("git")
        .args(["branch", "--merged", "main"])
        .current_dir(project_dir)
        .output()
        .await;

    let merged_set: std::collections::HashSet<String> = match merged_output {
        Ok(o) => String::from_utf8_lossy(&o.stdout)
            .lines()
            .map(|l| l.trim().trim_start_matches("* ").trim().to_string())
            .filter(|l| !l.is_empty())
            .collect(),
        Err(e) => {
            eprintln!("failed to check merged branches: {e}");
            return ExitCode::FAILURE;
        }
    };

    let mut removed = 0;
    for branch in &branches {
        if !merged_set.contains(*branch) {
            eprintln!("skipping unmerged branch: {branch}");
            continue;
        }

        let del_output = tokio::process::Command::new("git")
            .args(["branch", "-d", branch])
            .current_dir(project_dir)
            .output()
            .await;

        match del_output {
            Ok(o) if o.status.success() => {
                eprintln!("deleted branch: {branch}");
                removed += 1;
            }
            Ok(o) => {
                let stderr = String::from_utf8_lossy(&o.stderr);
                eprintln!("failed to delete branch {branch}: {stderr}");
            }
            Err(e) => {
                eprintln!("failed to delete branch {branch}: {e}");
            }
        }
    }

    eprintln!("deleted {removed} merged claudes/* branch(es)");
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
    profile: Option<&str>,
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
        profile: profile.map(String::from),
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

fn filter_tasks(manifest: &mut claudes::Manifest, task_names: &[String]) -> Result<(), String> {
    if task_names.is_empty() {
        return Ok(());
    }
    for name in task_names {
        if !manifest.tasks.iter().any(|t| &t.name == name) {
            return Err(format!("no task named '{name}' in manifest"));
        }
    }
    manifest.tasks.retain(|t| task_names.contains(&t.name));
    Ok(())
}
