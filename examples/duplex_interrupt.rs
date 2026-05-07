//! Cancel a long-running turn with `DuplexSession::interrupt`.
//!
//! Spawns a duplex session, kicks off a turn that asks for a long
//! response, then sends an interrupt 500ms later. Both futures are
//! driven concurrently via `tokio::join!`: the `send` future
//! resolves with a truncated `TurnResult`, and the `interrupt`
//! future resolves with `Ok(())` once the CLI acknowledges.
//!
//! ```sh
//! cargo run --example duplex_interrupt
//! ```
//!
//! Interrupt is the long-lived host's clean alternative to
//! SIGKILL'ing the child: the conversation stays alive, you can
//! follow up with another `send`, and the truncated turn comes
//! back with diagnostic fields you can show to the user.

use std::time::Duration;

use claude_wrapper::Claude;
use claude_wrapper::duplex::{DuplexOptions, DuplexSession};

#[tokio::main]
async fn main() -> claude_wrapper::Result<()> {
    let claude = Claude::builder().build()?;
    let session = DuplexSession::spawn(&claude, DuplexOptions::default().model("haiku")).await?;

    let send_fut = session.send(
        "Write a long, detailed essay about the history of programming \
         languages, covering at least 20 distinct languages chronologically.",
    );
    let interrupt_fut = async {
        tokio::time::sleep(Duration::from_millis(500)).await;
        eprintln!("sending interrupt...");
        session.interrupt().await
    };

    let (turn_result, interrupt_result) = tokio::join!(send_fut, interrupt_fut);

    let turn = turn_result?;
    interrupt_result?;

    println!("interrupted turn closed");
    if let Some(reason) = turn.result.get("terminal_reason").and_then(|v| v.as_str()) {
        println!("  terminal_reason: {reason}");
    }
    if let Some(subtype) = turn.result.get("subtype").and_then(|v| v.as_str()) {
        println!("  subtype: {subtype}");
    }
    if let Some(is_err) = turn.result.get("is_error").and_then(|v| v.as_bool()) {
        println!("  is_error: {is_err}");
    }
    if let Some(cost) = turn.total_cost_usd() {
        println!("  cost: ${cost:.4}");
    }

    session.close().await?;
    Ok(())
}
