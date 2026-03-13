//! Fan-out: run multiple independent tasks in parallel.
//!
//! Demonstrates parallel execution across pool slots. Each prompt
//! runs on a separate slot concurrently; results are collected when
//! all tasks complete.
//!
//! Use case: review multiple files, audit multiple crates, search
//! across multiple codebases, or batch-process independent items.
//!
//! ```sh
//! cargo run -p claude-pool --example fan_out
//! ```

use claude_pool::{Pool, PoolConfig};
use claude_wrapper::Claude;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let claude = Claude::builder().build()?;

    // 4 slots for parallelism, budget-capped at $2.
    let pool = Pool::builder(claude)
        .slots(4)
        .config(PoolConfig {
            model: Some("haiku".into()),
            budget_microdollars: Some(2_000_000),
            max_turns: Some(3),
            ..Default::default()
        })
        .build()
        .await?;

    // Fan out: all prompts run concurrently across available slots.
    let prompts = vec![
        "What is the capital of France? One sentence.",
        "What is the capital of Japan? One sentence.",
        "What is the capital of Brazil? One sentence.",
        "What is the capital of Australia? One sentence.",
    ];

    println!("Fanning out {} tasks across {} slots...", prompts.len(), 4);

    let results = pool.fan_out(&prompts).await?;

    for (i, result) in results.iter().enumerate() {
        println!(
            "[task {}] {} (${:.4})",
            i,
            result.output.trim(),
            result.cost_microdollars as f64 / 1_000_000.0
        );
    }

    let total_cost: u64 = results.iter().map(|r| r.cost_microdollars).sum();
    println!(
        "\nTotal: {} results, ${:.4}",
        results.len(),
        total_cost as f64 / 1_000_000.0
    );

    pool.drain().await?;
    Ok(())
}
