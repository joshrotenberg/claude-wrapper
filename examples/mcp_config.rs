//! Generate an MCP config file and use it with a query.
//!
//! ```sh
//! cargo run --example mcp_config
//! ```

use claude_wrapper::{ClaudeCommand, McpConfigBuilder, QueryCommand};

#[tokio::main]
async fn main() -> claude_wrapper::Result<()> {
    // Build an MCP config programmatically
    let dir = tempfile::tempdir().expect("failed to create temp dir");
    let config_path = McpConfigBuilder::new()
        .http_server("my-hub", "http://127.0.0.1:9090")
        .stdio_server("my-tool", "npx", ["my-mcp-server"])
        .write_to(dir.path().join(".mcp.json"))?;

    println!("Wrote MCP config to: {}", config_path.display());
    println!(
        "Contents:\n{}",
        std::fs::read_to_string(&config_path).unwrap()
    );

    // Use the config with a query (this will fail to connect, just showing the API)
    let cmd = QueryCommand::new("list available tools")
        .model("haiku")
        .mcp_config(config_path.to_string_lossy())
        .strict_mcp_config()
        .allowed_tools(["mcp__my-hub__*"])
        .max_turns(1)
        .no_session_persistence();

    println!(
        "\nWould run: claude {}",
        ClaudeCommand::args(&cmd).join(" ")
    );

    Ok(())
}
