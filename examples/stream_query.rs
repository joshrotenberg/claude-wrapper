//! Stream NDJSON events from a query in real time.
//!
//! ```sh
//! cargo run --example stream_query
//! ```

use claude_wrapper::streaming::{StreamEvent, stream_query};
use claude_wrapper::{Claude, OutputFormat, PermissionMode, QueryCommand};

#[tokio::main]
async fn main() -> claude_wrapper::Result<()> {
    let claude = Claude::builder().build()?;

    let cmd = QueryCommand::new("Explain quicksort in 3 sentences.")
        .model("haiku")
        .output_format(OutputFormat::StreamJson)
        .permission_mode(PermissionMode::Plan)
        .max_turns(1)
        .no_session_persistence();

    let output = stream_query(&claude, &cmd, |event: StreamEvent| {
        match event.event_type() {
            Some("result") => {
                println!("\n--- Result ---");
                if let Some(text) = event.result_text() {
                    println!("{text}");
                }
                if let Some(cost) = event.cost_usd() {
                    println!("Cost: ${cost:.4}");
                }
                if let Some(session) = event.session_id() {
                    println!("Session: {session}");
                }
            }
            Some(t) => {
                println!("[{t}]");
            }
            None => {}
        }
    })
    .await?;

    println!("\nExit code: {}", output.exit_code);

    Ok(())
}
