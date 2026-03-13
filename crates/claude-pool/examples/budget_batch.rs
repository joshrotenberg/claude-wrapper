//! Budget-capped batch processing with async submission.
//!
//! Demonstrates submitting multiple tasks asynchronously, polling
//! for results, and working within a pool-level budget constraint.
//! Tasks that would exceed the budget are rejected.
//!
//! Use case: processing a batch of items (files, issues, PRs) with
//! a hard spending cap, where you submit all work upfront and
//! collect results as they complete.
//!
//! ```sh
//! cargo run -p claude-pool --example budget_batch
//! ```

use claude_pool::{Pool, PoolConfig, TaskOverrides};
use claude_wrapper::Claude;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let claude = Claude::builder().build()?;

    // Tight budget: $0.50 across all tasks.
    let pool = Pool::builder(claude)
        .slots(3)
        .config(PoolConfig {
            model: Some("haiku".into()),
            budget_microdollars: Some(500_000), // $0.50
            max_turns: Some(2),
            ..Default::default()
        })
        .build()
        .await?;

    // Submit a batch of tasks asynchronously.
    let items = vec![
        "Explain what a mutex is in one sentence.",
        "Explain what an arc is in one sentence.",
        "Explain what a channel is in one sentence.",
        "Explain what a future is in one sentence.",
        "Explain what a pinned future is in one sentence.",
    ];

    let mut task_ids = Vec::new();
    for item in &items {
        // Per-task budget cap of $0.10 using TaskOverrides.
        match pool
            .submit_with_config(
                item,
                Some(TaskOverrides {
                    max_budget_usd: Some(0.10),
                    ..Default::default()
                }),
                vec![],
            )
            .await
        {
            Ok(id) => {
                println!("Submitted: {} -> {}", item, id.0);
                task_ids.push(id);
            }
            Err(e) => {
                println!("Rejected (budget?): {} -> {}", item, e);
            }
        }
    }

    // Poll for all results.
    println!("\nWaiting for results...\n");

    let mut completed = vec![false; task_ids.len()];
    loop {
        let mut all_done = true;
        for (i, task_id) in task_ids.iter().enumerate() {
            if completed[i] {
                continue;
            }
            if let Some(result) = pool.result(task_id).await? {
                println!(
                    "[{}] {} (${:.4}){}",
                    task_id.0,
                    result.output.trim(),
                    result.cost_microdollars as f64 / 1_000_000.0,
                    if result.budget_exceeded {
                        " [BUDGET EXCEEDED]"
                    } else {
                        ""
                    },
                );
                completed[i] = true;
            } else {
                all_done = false;
            }
        }
        if all_done {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_secs(1)).await;
    }

    // Final budget report.
    let status = pool.status().await?;
    println!(
        "\nBudget: ${:.4} spent of ${:.4}",
        status.total_spend_microdollars as f64 / 1_000_000.0,
        status
            .budget_microdollars
            .map(|b| b as f64 / 1_000_000.0)
            .unwrap_or(0.0)
    );

    pool.drain().await?;
    Ok(())
}
