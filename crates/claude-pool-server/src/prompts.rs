//! MCP prompt definitions for skills.
//!
//! Skills are exposed as MCP prompts so the coordinator can discover
//! them via standard MCP prompt listing.

use claude_pool::skill::SkillRegistry;
use tower_mcp::prompt::{Prompt, PromptBuilder};
use tower_mcp::protocol::{Content, GetPromptResult, PromptMessage, PromptRole};

/// Build MCP prompts from all registered skills.
pub fn skill_prompts(registry: &SkillRegistry) -> Vec<Prompt> {
    registry
        .list()
        .into_iter()
        .map(|skill| {
            let mut builder = PromptBuilder::new(&skill.name).description(&skill.description);

            for arg in &skill.arguments {
                if arg.required {
                    builder = builder.required_arg(&arg.name, &arg.description);
                } else {
                    builder = builder.optional_arg(&arg.name, &arg.description);
                }
            }

            let prompt_template = skill.prompt.clone();
            let arguments: Vec<_> = skill.arguments.iter().map(|a| a.name.clone()).collect();

            builder
                .handler(move |args| {
                    let prompt_template = prompt_template.clone();
                    let arguments = arguments.clone();
                    async move {
                        let mut rendered = prompt_template;
                        for arg_name in &arguments {
                            if let Some(value) = args.get(arg_name) {
                                rendered = rendered.replace(&format!("{{{arg_name}}}"), value);
                            }
                        }

                        Ok(GetPromptResult {
                            description: None,
                            messages: vec![PromptMessage {
                                role: PromptRole::User,
                                content: Content::Text {
                                    text: rendered,
                                    annotations: None,
                                    meta: None,
                                },
                                meta: None,
                            }],
                            meta: None,
                        })
                    }
                })
                .build()
        })
        .collect()
}
