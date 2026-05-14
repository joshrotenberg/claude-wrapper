//! Mutating-tool gating tests. We deliberately do NOT live-test the
//! actual mutations here -- they would alter the user's claude
//! installation (MCP servers, plugins, marketplaces). The gating is
//! the contract: with `policy.allow_mutations = false` the model
//! literally cannot discover them.

use claude_server::{ServerConfig, ServerPolicy, registered_tools};

fn cfg_with_policy(allow: bool) -> ServerConfig {
    ServerConfig {
        policy: ServerPolicy {
            allow_mutations: allow,
        },
        ..Default::default()
    }
}

const MUTATING_TOOLS: &[&str] = &[
    "claude_mcp_add",
    "claude_mcp_add_json",
    "claude_mcp_remove",
    "claude_plugin_install",
    "claude_plugin_uninstall",
    "claude_plugin_enable",
    "claude_plugin_disable",
    "claude_plugin_update",
    "claude_marketplace_add",
    "claude_marketplace_remove",
    "claude_marketplace_update",
];

#[test]
fn mutations_off_omits_all_mutating_tools() {
    let tools = registered_tools(cfg_with_policy(false)).expect("config built");
    let names: Vec<&str> = tools.iter().map(|t| t.name.as_str()).collect();
    for forbidden in MUTATING_TOOLS {
        assert!(
            !names.contains(forbidden),
            "tool {forbidden} should NOT be registered with allow_mutations=false; got {names:?}"
        );
    }
}

#[test]
fn mutations_on_registers_all_mutating_tools() {
    let tools = registered_tools(cfg_with_policy(true)).expect("config built");
    let names: Vec<&str> = tools.iter().map(|t| t.name.as_str()).collect();
    for expected in MUTATING_TOOLS {
        assert!(
            names.contains(expected),
            "tool {expected} should be registered with allow_mutations=true; got {names:?}"
        );
    }
}

#[test]
fn mutations_default_is_off() {
    let tools = registered_tools(ServerConfig::default()).expect("config built");
    let names: Vec<&str> = tools.iter().map(|t| t.name.as_str()).collect();
    for forbidden in MUTATING_TOOLS {
        assert!(
            !names.contains(forbidden),
            "default policy should not register {forbidden}; got {names:?}"
        );
    }
}
