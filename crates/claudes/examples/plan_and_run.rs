//! Plan a manifest and execute it.
//!
//! Demonstrates the core plan -> review -> run workflow.
//!
//! ```sh
//! cargo run --example plan_and_run -p claudes
//! ```

use claudes::{CleanupPolicy, PlanOptions, RunOptions, plan, run};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Plan: generate a manifest from options.
    let options = PlanOptions {
        prompts: vec![
            "What are the three primary colors? One sentence.".into(),
            "What are the three secondary colors? One sentence.".into(),
        ],
        model: Some("haiku".into()),
        max_turns: Some(1),
        permission_mode: Some("plan".into()),
        isolation: Some("none".into()),
        ..Default::default()
    };

    let manifest = plan(&options);

    // Review: inspect the manifest before running.
    println!("Generated manifest:");
    println!("{}", serde_json::to_string_pretty(&manifest)?);
    println!();

    // Run: execute the manifest.
    let run_opts = RunOptions {
        project_dir: std::env::current_dir()?,
        force: false,
        binary: None,
        env: vec![],
        cleanup: CleanupPolicy::None,
    };

    let result = run(&manifest, &run_opts).await?;

    println!("Results:");
    for task in &result.tasks {
        println!(
            "  {} - {} ({:.1}s)",
            task.name,
            if task.success { "ok" } else { "FAILED" },
            task.duration.as_secs_f64()
        );
    }

    Ok(())
}
