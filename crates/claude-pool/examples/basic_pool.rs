//! Basic pool usage: run a single task synchronously.
//!
//! Demonstrates pool construction, synchronous execution, status
//! checking, and graceful shutdown.
//!
//! ```sh
//! cargo run -p claude-pool --example basic_pool
//! ```

use claude_pool::{Pool, PoolConfig};
use claude_wrapper::Claude;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let claude = Claude::builder().build()?;

    // Build a pool with 2 slots and a $1 budget cap.
    let pool = Pool::builder(claude)
        .slots(2)
        .config(PoolConfig {
            model: Some("haiku".into()),
            budget_microdollars: Some(1_000_000), // $1.00
            max_turns: Some(3),
            ..Default::default()
        })
        .build()
        .await?;

    // Synchronous run: blocks until complete.
    let result = pool
        .run("What are the three primary colors? Answer in one sentence.")
        .await?;
    println!("Output: {}", result.output);
    println!(
        "Cost:   ${:.4}",
        result.cost_microdollars as f64 / 1_000_000.0
    );

    // Check pool status.
    let status = pool.status().await?;
    println!(
        "Pool:   {} slots, {} completed, ${:.4} spent",
        status.total_slots,
        status.completed_tasks,
        status.total_spend_microdollars as f64 / 1_000_000.0
    );

    // Graceful shutdown.
    let summary = pool.drain().await?;
    println!(
        "Drain:  {} tasks, ${:.4} total",
        summary.total_tasks_completed,
        summary.total_cost_microdollars as f64 / 1_000_000.0
    );

    Ok(())
}
