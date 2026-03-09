//! Simple oneshot query example.
//!
//! ```sh
//! cargo run --example oneshot
//! ```

use claude_wrapper::{Claude, ClaudeCommand, QueryCommand};

#[tokio::main]
async fn main() -> claude_wrapper::Result<()> {
    let claude = Claude::builder().build()?;

    let output = QueryCommand::new("What are the three primary colors? One sentence.")
        .model("haiku")
        .max_turns(1)
        .no_session_persistence()
        .execute(&claude)
        .await?;

    println!("{}", output.stdout);
    Ok(())
}
