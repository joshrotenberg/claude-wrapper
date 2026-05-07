//! Interactive multi-turn chat using `DuplexSession`.
//!
//! Spawns one `claude` subprocess and holds it open across turns.
//! A background task subscribes to the event stream and prints
//! event tags live so you can see the duplex protocol in action
//! while the model is working.
//!
//! Type a message and press Enter. Empty line ends the chat;
//! Ctrl+D / EOF also closes cleanly.
//!
//! ```sh
//! cargo run --example duplex_chat
//! ```

use std::io::Write;

use claude_wrapper::Claude;
use claude_wrapper::duplex::{DuplexOptions, DuplexSession, InboundEvent};
use tokio::io::{AsyncBufReadExt, BufReader};

#[tokio::main]
async fn main() -> claude_wrapper::Result<()> {
    let claude = Claude::builder().build()?;
    let session = DuplexSession::spawn(&claude, DuplexOptions::default().model("haiku")).await?;

    // Background task: print event tags as they arrive. The receiver
    // owns its end of the broadcast channel, so the task runs
    // independently of the main REPL loop. When the session closes,
    // events_tx is dropped, recv() returns Err, and the task exits.
    let mut rx = session.subscribe();
    let printer = tokio::spawn(async move {
        while let Ok(event) = rx.recv().await {
            let tag = match event {
                InboundEvent::SystemInit { ref session_id } => {
                    eprintln!("[init] session_id={session_id}");
                    continue;
                }
                InboundEvent::Assistant(_) => "[assistant]",
                InboundEvent::StreamEvent(_) => "[stream]",
                InboundEvent::User(_) => "[user]",
                InboundEvent::Other(_) => "[other]",
            };
            eprintln!("{tag}");
        }
    });

    let stdin = tokio::io::stdin();
    let mut lines = BufReader::new(stdin).lines();

    println!("DuplexSession chat (empty line to exit)");
    print!("> ");
    std::io::stdout().flush().ok();

    while let Some(line) = lines.next_line().await.ok().flatten() {
        if line.trim().is_empty() {
            break;
        }

        let turn = session.send(line).await?;

        if let Some(text) = turn.result_text() {
            println!("\n{text}\n");
        }
        if let Some(cost) = turn.total_cost_usd() {
            eprintln!("(turn cost: ${cost:.4})");
        }

        print!("> ");
        std::io::stdout().flush().ok();
    }

    println!("\nclosing session...");
    session.close().await?;
    let _ = printer.await;
    Ok(())
}
