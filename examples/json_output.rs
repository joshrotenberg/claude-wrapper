//! Query with structured JSON output.
//!
//! ```sh
//! cargo run --example json_output
//! ```

use claude_wrapper::{Claude, PermissionMode, QueryCommand};

#[tokio::main]
async fn main() -> claude_wrapper::Result<()> {
    let claude = Claude::builder().build()?;

    let result = QueryCommand::new("What is the capital of France? One word.")
        .model("haiku")
        .max_turns(1)
        .no_session_persistence()
        .permission_mode(PermissionMode::Plan)
        .execute_json(&claude)
        .await?;

    println!("Answer: {}", result.result);
    println!("Session: {}", result.session_id);
    if let Some(cost) = result.cost_usd {
        println!("Cost: ${cost:.4}");
    }
    if let Some(duration) = result.duration_ms {
        println!("Duration: {duration}ms");
    }

    Ok(())
}
