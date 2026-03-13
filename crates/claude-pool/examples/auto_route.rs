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
//! Also demonstrates structured hints that constrain routing decisions.
//!
//! ```sh
//! cargo run -p claude-pool --example auto_route
//! ```

use claude_pool::{AutoHint, Pool, PoolConfig, RoutePreference};
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

    // --- Basic routing (no hints) ---

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

    println!("=== Basic routing (no hints) ===\n");
    for prompt in &prompts {
        println!("---");
        println!("Prompt: {}", &prompt[..prompt.len().min(80)]);

        let route = pool.route(prompt).await?;
        println!("Route:  {:?}\n", route);
    }

    // --- Routing with hints ---

    println!("=== Routing with hints ===\n");

    // Hint: only 2 slots available, prefer parallel, with decomposition boundaries.
    let hints = AutoHint {
        max_parallel: Some(2),
        prefer: Some(RoutePreference::PreferParallel),
        domain: Some("Rust monorepo with independent crates".into()),
        decomposition_hints: Some(vec![
            "claude-wrapper crate".into(),
            "claude-pool crate".into(),
        ]),
        ..Default::default()
    };

    let prompt = "Audit the public API surface of each crate in this workspace. \
                  Check for missing doc comments and inconsistent naming.";

    println!("Prompt: {}", &prompt[..prompt.len().min(80)]);
    println!("Hints:  max_parallel=2, prefer=parallel, 2 decomposition boundaries");

    let route = pool.route_with_hints(prompt, &hints).await?;
    println!("Route:  {:?}\n", route);

    // Hint: cap chain depth.
    let hints = AutoHint {
        max_chain_steps: Some(2),
        ..Default::default()
    };

    let prompt = "First analyze the code for performance issues, then suggest fixes, \
                  then write tests for the fixes, then summarize.";

    println!("---");
    println!("Prompt: {}", &prompt[..prompt.len().min(80)]);
    println!("Hints:  max_chain_steps=2");

    let route = pool.route_with_hints(prompt, &hints).await?;
    println!("Route:  {:?}\n", route);

    // --- Execute one ---

    println!("=== Executing with auto-route ===\n");
    let prompt = "Explain what a mutex, an arc, and a channel are in Rust. \
                  Each explanation should be one sentence. These are independent.";

    let result = pool.auto(prompt).await?;
    println!("Chose:  {}", result.route_name());
    println!("Output:\n{}", result.output());
    dbg!(result);

    pool.drain().await?;
    Ok(())
}
