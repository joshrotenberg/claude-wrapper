//! Auto-routing: let the pool decide whether to run, fan_out, or chain.
//!
//! Sends a task to the pool's auto-router, which uses a single LLM call
//! to classify the work and then executes via the appropriate path.
//!
//! Try different prompts to see how the router classifies them:
//! - Simple task -> single
//! - Multiple independent items -> parallel
//! - Sequential dependencies -> chain
//!
//! ```sh
//! cargo run -p claude-pool --example auto_route
//! ```

use claude_pool::{Pool, PoolConfig};
use claude_wrapper::Claude;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let claude = Claude::builder().build()?;

    let pool = Pool::builder(claude)
        .slots(4)
        .config(PoolConfig {
            model: Some("haiku".into()),
            budget_microdollars: Some(2_000_000),
            max_turns: Some(5),
            ..Default::default()
        })
        .build()
        .await?;

    // Try a few different prompts.
    let prompts = [
        // Should route as single.
        "What is the capital of France? One sentence.",
        // Should route as parallel.
        "Review these three files for bugs: src/main.rs, src/lib.rs, src/utils.rs. \
         Each review is independent.",
        // Should route as chain.
        "First, list three common Rust mistakes. Then, for each mistake, suggest a fix. \
         Finally, summarize everything into a single paragraph.",
    ];

    for prompt in &prompts {
        println!("---");
        println!("Prompt: {}", &prompt[..prompt.len().min(80)]);

        // Route only (no execution) to see the decision.
        let route = pool.route(prompt).await?;
        println!("Route:  {:?}\n", route);
    }

    // Now actually execute one.
    println!("===\nExecuting with auto-route:\n");
    let prompt = "Explain what a mutex, an arc, and a channel are in Rust. \
                  Each explanation should be one sentence. These are independent.";

    let result = pool.auto(prompt).await?;
    println!("Chose:  {}", result.route_name());
    println!("Output:\n{}", result.output());

    pool.drain().await?;
    Ok(())
}
