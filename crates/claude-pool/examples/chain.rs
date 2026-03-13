//! Chain execution: sequential pipeline where each step feeds the next.
//!
//! Demonstrates a multi-step workflow: analyze a problem, propose a
//! solution, then critique the solution. Each step sees the output
//! of the previous step via `{previous_output}` substitution.
//!
//! Use case: analyze -> plan -> implement -> test pipelines,
//! draft -> review -> revise loops, or any sequential dependency.
//!
//! ```sh
//! cargo run -p claude-pool --example chain
//! ```

use claude_pool::{
    ChainOptions, ChainStep, Pool, PoolConfig, SkillRegistry, StepAction, StepFailurePolicy,
    TaskOverrides,
};
use claude_wrapper::Claude;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let claude = Claude::builder().build()?;

    let pool = Pool::builder(claude)
        .slots(1) // Chains run on a single slot sequentially.
        .config(PoolConfig {
            model: Some("haiku".into()),
            budget_microdollars: Some(2_000_000),
            max_turns: Some(5),
            ..Default::default()
        })
        .build()
        .await?;

    // Define a 3-step chain.
    let steps = vec![
        ChainStep {
            name: "analyze".into(),
            action: StepAction::Prompt {
                prompt: "List three common mistakes people make when writing async Rust code. \
                         Be concise -- one sentence per mistake."
                    .into(),
            },
            config: None,
            failure_policy: StepFailurePolicy::default(),
            output_vars: Default::default(),
        },
        ChainStep {
            name: "solutions".into(),
            action: StepAction::Prompt {
                prompt: "For each mistake identified below, suggest a one-sentence fix:\n\n\
                         {previous_output}"
                    .into(),
            },
            config: None,
            failure_policy: StepFailurePolicy {
                retries: 1,
                recovery_prompt: Some("Try again with simpler language.".into()),
            },
            output_vars: Default::default(),
        },
        ChainStep {
            name: "summary".into(),
            action: StepAction::Prompt {
                prompt: "Summarize the following mistakes and solutions into a single \
                         paragraph suitable for a README:\n\n{previous_output}"
                    .into(),
            },
            // Use a different model for the summary step.
            config: Some(TaskOverrides {
                model: Some("sonnet".into()),
                ..Default::default()
            }),
            failure_policy: StepFailurePolicy::default(),
            output_vars: Default::default(),
        },
    ];

    println!("Running 3-step chain: analyze -> solutions -> summary\n");

    let skills = SkillRegistry::new();
    let task_id = pool
        .submit_chain(steps, &skills, ChainOptions::default())
        .await?;

    // Poll for the result.
    loop {
        if let Some(result) = pool.result(&task_id).await? {
            if result.success {
                println!("Chain output:\n{}", result.output);
            } else {
                println!("Chain failed: {}", result.output);
            }
            println!(
                "\nCost: ${:.4}",
                result.cost_microdollars as f64 / 1_000_000.0
            );
            break;
        }
        tokio::time::sleep(std::time::Duration::from_secs(2)).await;
    }

    pool.drain().await?;
    Ok(())
}
