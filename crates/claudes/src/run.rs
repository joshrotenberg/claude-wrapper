use std::collections::HashMap;
use std::time::Instant;

use anyhow::{Context, Result};
use claude_pool::chain::{ChainStep, StepAction, StepFailurePolicy};
use claude_pool::pool::Pool;
use claude_pool::store::InMemoryStore;
use claude_pool::types::{PoolConfig, TaskResult};
use claude_wrapper::Claude;

use crate::Cli;
use crate::context::TaskContext;
use crate::decisioner::{ClaudeDecisioner, Decisioner, ExecutionPlan};

/// Build a Claude client from CLI options.
fn build_claude(cli: &Cli) -> Result<Claude> {
    let mut builder = Claude::builder();
    if let Some(ref dir) = cli.working_dir {
        builder = builder.working_dir(dir);
    }
    builder.build().context("Failed to find claude binary")
}

/// Build a pool from CLI options with the given slot count.
async fn build_pool(cli: &Cli, claude: Claude, slots: usize) -> Result<Pool<InMemoryStore>> {
    let mut config = PoolConfig::default();
    if let Some(ref model) = cli.model {
        config.model = Some(model.clone());
    }
    if let Some(budget) = cli.max_budget {
        config.budget_microdollars = Some((budget * 1_000_000.0) as u64);
    }

    Ok(Pool::builder(claude)
        .slots(slots)
        .config(config)
        .build()
        .await?)
}

/// Print a task result summary.
fn print_result(result: &TaskResult, elapsed: std::time::Duration) {
    println!("{}", result.output);
    eprintln!();
    eprintln!("---");
    eprintln!(
        "Cost: ${:.4}  Duration: {:.1}s  Turns: {}  Model: {}",
        result.cost_microdollars as f64 / 1_000_000.0,
        elapsed.as_secs_f64(),
        result.turns_used,
        result.model.as_deref().unwrap_or("unknown"),
    );
    if !result.success {
        eprintln!("Status: FAILED");
        if let Some(ref stderr) = result.stderr {
            eprintln!("Stderr: {}", stderr);
        }
    }
}

/// Run multiple tasks in parallel.
pub async fn parallel(cli: &Cli, prompts: &[String]) -> Result<()> {
    let claude = build_claude(cli)?;
    let slot_count = cli.slots.unwrap_or(prompts.len().min(10));
    let pool = build_pool(cli, claude, slot_count).await?;
    let start = Instant::now();

    eprintln!(
        "Running {} tasks in parallel ({} slots)...",
        prompts.len(),
        slot_count
    );

    let prompt_refs: Vec<&str> = prompts.iter().map(|s| s.as_str()).collect();
    let results = pool.fan_out(&prompt_refs).await?;

    let total_cost: u64 = results.iter().map(|r| r.cost_microdollars).sum();
    let succeeded = results.iter().filter(|r| r.success).count();

    for (i, result) in results.iter().enumerate() {
        eprintln!();
        eprintln!("=== Task {} ===", i + 1);
        println!("{}", result.output);
    }

    eprintln!();
    eprintln!("---");
    eprintln!(
        "Total: {}/{} succeeded  Cost: ${:.4}  Duration: {:.1}s",
        succeeded,
        results.len(),
        total_cost as f64 / 1_000_000.0,
        start.elapsed().as_secs_f64(),
    );

    pool.drain().await?;
    Ok(())
}

/// Run a sequential chain.
pub async fn chain(cli: &Cli, step_prompts: &[String]) -> Result<()> {
    let claude = build_claude(cli)?;
    let pool = build_pool(cli, claude, 1).await?;
    let start = Instant::now();

    eprintln!("Running chain with {} steps...", step_prompts.len());

    let steps: Vec<ChainStep> = step_prompts
        .iter()
        .enumerate()
        .map(|(i, prompt)| ChainStep {
            name: format!("step-{}", i + 1),
            action: StepAction::Prompt {
                prompt: prompt.clone(),
            },
            config: None,
            failure_policy: StepFailurePolicy::default(),
            output_vars: HashMap::new(),
        })
        .collect();

    let skills = claude_pool::skill::SkillRegistry::new();
    let result = claude_pool::chain::execute_chain(&pool, &skills, &steps).await?;

    println!("{}", result.final_output);
    eprintln!();
    eprintln!("---");
    eprintln!(
        "Chain: {}/{} steps succeeded  Cost: ${:.4}  Duration: {:.1}s",
        result.steps.iter().filter(|s| s.success).count(),
        result.steps.len(),
        result.total_cost_microdollars as f64 / 1_000_000.0,
        start.elapsed().as_secs_f64(),
    );

    pool.drain().await?;
    Ok(())
}

/// Run a task using the decisioner to pick the best strategy.
pub async fn auto(cli: &Cli, prompt: &str) -> Result<()> {
    let claude = build_claude(cli)?;
    let start = Instant::now();

    // Gather codebase context.
    let context = TaskContext::gather(cli.working_dir.as_deref()).await?;

    // Ask the decisioner for a plan.
    eprintln!("Analyzing task...");
    let decisioner = ClaudeDecisioner::new(&claude);
    let plan = match decisioner
        .decide(prompt, &context, cli.strategy, cli.max_budget)
        .await
    {
        Ok(plan) => plan,
        Err(e) => {
            eprintln!("Decisioner failed ({}), falling back to single call.", e);
            ExecutionPlan::Single {
                prompt: prompt.to_string(),
                model: cli.model.clone(),
            }
        }
    };

    // Execute the plan.
    execute_plan(cli, &claude, &plan, start).await
}

/// Show the execution plan without running it.
pub async fn plan(cli: &Cli, prompt: &str) -> Result<()> {
    let claude = build_claude(cli)?;

    // Gather codebase context.
    let context = TaskContext::gather(cli.working_dir.as_deref()).await?;

    // Ask the decisioner for a plan.
    eprintln!("Analyzing task...");
    let decisioner = ClaudeDecisioner::new(&claude);
    let plan = decisioner
        .decide(prompt, &context, cli.strategy, cli.max_budget)
        .await
        .context("decisioner failed")?;

    // Display the plan.
    display_plan(&plan);
    Ok(())
}

/// Execute an execution plan.
async fn execute_plan(
    cli: &Cli,
    claude: &Claude,
    plan: &ExecutionPlan,
    start: Instant,
) -> Result<()> {
    match plan {
        ExecutionPlan::Single { prompt, model } => {
            // For model override from the plan, we set it in pool config.
            let mut config = PoolConfig::default();
            if let Some(m) = model {
                config.model = Some(m.clone());
            } else if let Some(ref m) = cli.model {
                config.model = Some(m.clone());
            }
            if let Some(budget) = cli.max_budget {
                config.budget_microdollars = Some((budget * 1_000_000.0) as u64);
            }

            let pool = Pool::builder(claude.clone())
                .slots(1)
                .config(config)
                .build()
                .await?;

            eprintln!("Running single task...");
            let result = pool.run(prompt).await?;
            print_result(&result, start.elapsed());
            pool.drain().await?;
        }
        ExecutionPlan::Parallel { tasks, slots } => {
            let slot_count = slots.unwrap_or(tasks.len().min(10));
            let pool = build_pool(cli, claude.clone(), slot_count).await?;

            eprintln!(
                "Running {} tasks in parallel ({} slots)...",
                tasks.len(),
                slot_count
            );

            let prompts: Vec<&str> = tasks.iter().map(|t| t.prompt.as_str()).collect();
            let results = pool.fan_out(&prompts).await?;

            let total_cost: u64 = results.iter().map(|r| r.cost_microdollars).sum();
            let succeeded = results.iter().filter(|r| r.success).count();

            for (i, result) in results.iter().enumerate() {
                eprintln!();
                eprintln!("=== Task {} ===", i + 1);
                println!("{}", result.output);
            }

            eprintln!();
            eprintln!("---");
            eprintln!(
                "Total: {}/{} succeeded  Cost: ${:.4}  Duration: {:.1}s",
                succeeded,
                results.len(),
                total_cost as f64 / 1_000_000.0,
                start.elapsed().as_secs_f64(),
            );

            pool.drain().await?;
        }
        ExecutionPlan::Chain { steps } => {
            let pool = build_pool(cli, claude.clone(), 1).await?;

            eprintln!("Running chain with {} steps...", steps.len());

            let chain_steps: Vec<ChainStep> = steps
                .iter()
                .map(|s| ChainStep {
                    name: s.name.clone(),
                    action: StepAction::Prompt {
                        prompt: s.prompt.clone(),
                    },
                    config: None,
                    failure_policy: StepFailurePolicy::default(),
                    output_vars: HashMap::new(),
                })
                .collect();

            let skills = claude_pool::skill::SkillRegistry::new();
            let result = claude_pool::chain::execute_chain(&pool, &skills, &chain_steps).await?;

            println!("{}", result.final_output);
            eprintln!();
            eprintln!("---");
            eprintln!(
                "Chain: {}/{} steps succeeded  Cost: ${:.4}  Duration: {:.1}s",
                result.steps.iter().filter(|s| s.success).count(),
                result.steps.len(),
                result.total_cost_microdollars as f64 / 1_000_000.0,
                start.elapsed().as_secs_f64(),
            );

            pool.drain().await?;
        }
    }

    Ok(())
}

/// Display an execution plan for review.
fn display_plan(plan: &ExecutionPlan) {
    match plan {
        ExecutionPlan::Single { prompt, model } => {
            eprintln!("Strategy: single");
            eprintln!("Model: {}", model.as_deref().unwrap_or("default"));
            eprintln!("Prompt: {}", prompt);
        }
        ExecutionPlan::Parallel { tasks, slots } => {
            eprintln!("Strategy: parallel ({} tasks)", tasks.len());
            if let Some(s) = slots {
                eprintln!("Slots: {}", s);
            }
            for (i, task) in tasks.iter().enumerate() {
                eprintln!();
                eprintln!(
                    "  Task {} [{}]:",
                    i + 1,
                    task.model.as_deref().unwrap_or("default")
                );
                eprintln!("    {}", task.prompt);
            }
        }
        ExecutionPlan::Chain { steps } => {
            eprintln!("Strategy: chain ({} steps)", steps.len());
            for (i, step) in steps.iter().enumerate() {
                eprintln!();
                eprintln!(
                    "  Step {} '{}' [{}]:",
                    i + 1,
                    step.name,
                    step.model.as_deref().unwrap_or("default")
                );
                eprintln!("    {}", step.prompt);
            }
        }
    }
}
