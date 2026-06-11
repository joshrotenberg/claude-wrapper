//! Simulate a multi-agent worker pattern.
//!
//! Shows how to set up a worker agent with MCP hub connectivity,
//! restricted tools, and budget caps -- a common pattern for
//! orchestrating multiple Claude agents via a central hub.
//!
//! ```sh
//! cargo run --example agent_worker
//! ```

use claude_wrapper::{Claude, ClaudeCommand, McpConfigBuilder, PermissionMode, QueryCommand};

#[tokio::main]
async fn main() -> claude_wrapper::Result<()> {
    // 1. Generate MCP config for hub connectivity
    let dir = tempfile::tempdir().expect("failed to create temp dir");
    let config_path = McpConfigBuilder::new()
        .http_server("my-hub", "http://127.0.0.1:9090")
        .write_to(dir.path().join(".mcp.json"))?;

    // 2. Build the client
    //    CLAUDECODE env var is automatically removed by claude-wrapper
    let claude = Claude::builder()
        .env("HUB_URL", "http://127.0.0.1:9090")
        .timeout_secs(300)
        .build()?;

    // 3. Configure a worker agent query
    let worker = QueryCommand::new(
        "You are dev-1. Call hub/claim to get a task, implement it, \
         then call hub/complete when done.",
    )
    .mcp_config(config_path.to_string_lossy())
    .strict_mcp_config()
    .allowed_tools([
        "mcp__my-hub__*",
        "Bash",
        "Read",
        "Edit",
        "Write",
        "Glob",
        "Grep",
    ])
    .permission_mode(PermissionMode::AcceptEdits)
    .max_budget_usd(0.50)
    .max_turns(10)
    .no_session_persistence();

    // 4. Print what would be executed
    println!("Worker command args:");
    for arg in ClaudeCommand::args(&worker) {
        println!("  {arg}");
    }

    // Uncomment to actually run (requires a running MCP hub):
    // let output = worker.execute(&claude).await?;
    // println!("Exit code: {}", output.exit_code);

    let _ = claude; // suppress unused warning
    Ok(())
}
