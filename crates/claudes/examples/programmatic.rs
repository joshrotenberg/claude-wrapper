//! Build and execute a manifest programmatically.
//!
//! Shows how to use claudes as a library: construct tasks directly,
//! build a manifest, and run it.
//!
//! ```sh
//! cargo run --example programmatic -p claudes
//! ```

use claudes::{Isolation, Manifest, RunOptions, Task, run};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Build tasks directly using the Task struct.
    let mut task = Task::new(
        "quick-question",
        "What is the speed of light? One sentence.",
    );
    task.model = Some("haiku".into());
    task.max_turns = Some(1);
    task.permission_mode = Some("plan".into());
    task.isolation = Some(Isolation::None);
    task.no_session_persistence = Some(true);

    let manifest = Manifest::new(vec![task]);

    // Validate.
    manifest.validate().map_err(|errs| errs.join("; "))?;

    // Execute.
    let options = RunOptions {
        project_dir: std::env::current_dir()?,
        force: false,
        binary: None,
        env: vec![],
    };

    let result = run(&manifest, &options).await?;

    for task in &result.tasks {
        if task.success {
            // Parse the JSON output to extract the result text.
            if let Ok(v) = serde_json::from_str::<serde_json::Value>(&task.stdout)
                && let Some(text) = v.get("result").and_then(|r| r.as_str())
            {
                println!("{text}");
            }
        } else {
            eprintln!("Task {} failed: {}", task.name, task.stderr);
        }
    }

    Ok(())
}
