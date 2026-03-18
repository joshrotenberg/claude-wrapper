//! Load and execute a manifest from a JSON file.
//!
//! ```sh
//! # First, create a manifest:
//! cargo run -p claudes -- plan -p "What is 2+2?" --model haiku --isolation none > /tmp/demo.json
//!
//! # Then run this example:
//! cargo run --example manifest_file -p claudes -- /tmp/demo.json
//! ```

use claudes::{Manifest, RunOptions, run};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let path = std::env::args()
        .nth(1)
        .expect("usage: manifest_file <path-to-manifest.json>");

    // Load the manifest from disk.
    let content = std::fs::read_to_string(&path)?;
    let manifest: Manifest = serde_json::from_str(&content)?;

    // Validate before running.
    if let Err(errors) = manifest.validate() {
        eprintln!("Invalid manifest:");
        for e in &errors {
            eprintln!("  - {e}");
        }
        std::process::exit(1);
    }

    println!("Executing {} task(s) from {path}", manifest.tasks.len());

    let options = RunOptions {
        project_dir: std::env::current_dir()?,
        force: false,
        binary: None,
        env: vec![],
    };

    let result = run(&manifest, &options).await?;

    println!(
        "\n{}/{} tasks succeeded",
        result.success_count(),
        result.tasks.len()
    );

    if !result.all_succeeded() {
        std::process::exit(1);
    }

    Ok(())
}
