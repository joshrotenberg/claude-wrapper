//! MCP prompts: message templates clients can pull into context.

use std::collections::HashMap;

use tower_mcp::protocol::GetPromptResult;
use tower_mcp::{Prompt, PromptBuilder};

use crate::state::ServerState;

pub(crate) fn prompts(_state: &ServerState) -> Vec<Prompt> {
    vec![prompt_describe_server()]
}

fn prompt_describe_server() -> Prompt {
    PromptBuilder::new("describe_server")
        .description(
            "Ask the recipient LLM to describe this claude-server by \
             reading its own MCP resources. Zero args. Intended for \
             bootstrapping a new client / coordinator.",
        )
        .handler(|_args: HashMap<String, String>| async move {
            Ok(GetPromptResult::builder()
                .description("Summarize this claude-server via its resources.")
                .user(
                    "Read the MCP resources `claude://config` and `claude://tools` \
                     from this server, then summarize: what is this server, what \
                     tools does it expose, and what is the active configuration? \
                     Keep the summary under 200 words. Do not invoke any tools \
                     beyond reading these resources.",
                )
                .build())
        })
        .build()
}
