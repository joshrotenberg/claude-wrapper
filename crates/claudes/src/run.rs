use std::collections::HashMap;
use std::time::Instant;

use anyhow::{Context, Result};
use claude_pool::chain::{ChainStep, StepAction, StepFailurePolicy};
use claude_pool::pool::Pool;
use claude_pool::store::InMemoryStore;
use claude_pool::types::{PoolConfig, TaskResult};
use claude_wrapper::Claude;

use crate::Cli;

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

/// Run a single task.
pub async fn single(cli: &Cli, prompt: &str) -> Result<()> {
    let claude = build_claude(cli)?;
    let pool = build_pool(cli, claude, 1).await?;
    let start = Instant::now();

    eprintln!("Running single task...");
    let result = pool.run(prompt).await?;
    print_result(&result, start.elapsed());

    pool.drain().await?;
    Ok(())
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

/// Show the execution plan without running it.
pub async fn plan(_cli: &Cli, prompt: &str) -> Result<()> {
    eprintln!("Plan mode is not yet implemented.");
    eprintln!();
    eprintln!("When implemented, this will:");
    eprintln!("  1. Analyze the task: \"{}\"", prompt);
    eprintln!("  2. Read codebase context (git status, file structure)");
    eprintln!("  3. Decide execution strategy (single / parallel / chain)");
    eprintln!("  4. Show the plan for review");
    eprintln!("  5. Execute on confirmation");
    eprintln!();
    eprintln!("For now, use:");
    eprintln!("  claudes \"{}\"                    # single task", prompt);
    eprintln!("  claudes --parallel \"a\" \"b\" \"c\"   # explicit parallel");
    eprintln!("  claudes --chain \"a\" \"b\" \"c\"      # explicit chain");
    Ok(())
}
