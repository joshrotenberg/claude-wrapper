//! Runtime surface gate tests. Verifies that the `[surfaces]`
//! ServerConfig block correctly disables individual surfaces
//! without recompiling, layered on top of the existing Cargo
//! feature gates.

#![cfg(feature = "full")]

use claude_server::config::SurfacesConfig;
use claude_server::{ServerConfig, registered_tools};

fn cfg_with_surfaces(surfaces: SurfacesConfig) -> ServerConfig {
    ServerConfig {
        surfaces,
        ..Default::default()
    }
}

fn tool_names(cfg: ServerConfig) -> Vec<String> {
    registered_tools(cfg)
        .expect("registered")
        .into_iter()
        .map(|t| t.name)
        .collect()
}

#[test]
fn default_surfaces_all_on() {
    let names = tool_names(ServerConfig::default());
    for expected in [
        "claude_query",
        "chat_open",
        "turn_get",
        "metrics_summary",
        "agent_list",
        "claude_job_list",
        "worktree_list",
        "claude_project_list",
    ] {
        assert!(
            names.contains(&expected.to_string()),
            "missing {expected} from default surface; got {names:?}"
        );
    }
}

#[test]
fn disable_artifacts_drops_agent_tools() {
    let surfaces = SurfacesConfig {
        enable_artifacts: false,
        ..Default::default()
    };
    let names = tool_names(cfg_with_surfaces(surfaces));
    for forbidden in ["agent_list", "agent_get"] {
        assert!(
            !names.contains(&forbidden.to_string()),
            "{forbidden} leaked through disabled artifacts gate; got {names:?}"
        );
    }
    // sibling surfaces stay
    assert!(names.contains(&"claude_query".to_string()));
    assert!(names.contains(&"chat_open".to_string()));
}

#[test]
fn disable_jobs_drops_job_tools() {
    let surfaces = SurfacesConfig {
        enable_jobs: false,
        ..Default::default()
    };
    let names = tool_names(cfg_with_surfaces(surfaces));
    for forbidden in ["claude_job_list", "claude_job_get"] {
        assert!(
            !names.contains(&forbidden.to_string()),
            "{forbidden} leaked"
        );
    }
}

#[test]
fn disable_history_drops_session_tools() {
    let surfaces = SurfacesConfig {
        enable_history: false,
        ..Default::default()
    };
    let names = tool_names(cfg_with_surfaces(surfaces));
    for forbidden in [
        "claude_project_list",
        "claude_session_list",
        "claude_session_get",
    ] {
        assert!(
            !names.contains(&forbidden.to_string()),
            "{forbidden} leaked"
        );
    }
}

#[test]
fn disable_worktrees_drops_worktree_tool() {
    let surfaces = SurfacesConfig {
        enable_worktrees: false,
        ..Default::default()
    };
    let names = tool_names(cfg_with_surfaces(surfaces));
    assert!(!names.contains(&"worktree_list".to_string()), "leaked");
}

#[test]
fn disable_chat_drops_chat_and_turn_tools() {
    let surfaces = SurfacesConfig {
        enable_chat: false,
        ..Default::default()
    };
    let names = tool_names(cfg_with_surfaces(surfaces));
    for forbidden in [
        "chat_open",
        "chat_send",
        "chat_list",
        "chat_close",
        "turn_get",
        "turn_wait",
        "turn_cancel",
        "turn_list",
    ] {
        assert!(
            !names.contains(&forbidden.to_string()),
            "{forbidden} leaked through disabled chat gate"
        );
    }
    // sibling surfaces stay
    assert!(names.contains(&"claude_query".to_string()));
}

#[test]
fn disable_core_drops_claude_passthrough_tools() {
    let surfaces = SurfacesConfig {
        enable_core: false,
        ..Default::default()
    };
    let names = tool_names(cfg_with_surfaces(surfaces));
    for forbidden in ["claude_query", "claude_cli_version", "claude_doctor"] {
        assert!(
            !names.contains(&forbidden.to_string()),
            "{forbidden} leaked"
        );
    }
}

#[test]
fn mutations_runtime_gate_requires_both_policy_and_surfaces() {
    use claude_server::ServerPolicy;

    // policy=true but surfaces.enable_mutations=false -> no muts
    let cfg = ServerConfig {
        policy: ServerPolicy {
            allow_mutations: true,
        },
        surfaces: SurfacesConfig {
            enable_mutations: false,
            ..Default::default()
        },
        ..Default::default()
    };
    let names = tool_names(cfg);
    assert!(
        !names.contains(&"claude_plugin_install".to_string()),
        "mutating tool leaked when surfaces.enable_mutations = false; got {names:?}"
    );

    // surfaces=true but policy=false -> no muts
    let cfg = ServerConfig {
        policy: ServerPolicy {
            allow_mutations: false,
        },
        surfaces: SurfacesConfig {
            enable_mutations: true,
            ..Default::default()
        },
        ..Default::default()
    };
    let names = tool_names(cfg);
    assert!(!names.contains(&"claude_plugin_install".to_string()));

    // both true -> muts appear
    let cfg = ServerConfig {
        policy: ServerPolicy {
            allow_mutations: true,
        },
        surfaces: SurfacesConfig {
            enable_mutations: true,
            ..Default::default()
        },
        ..Default::default()
    };
    let names = tool_names(cfg);
    assert!(
        names.contains(&"claude_plugin_install".to_string()),
        "mutating tools should register with both gates open; got {names:?}"
    );
}

#[test]
fn agent_mutating_tools_need_artifacts_and_mutations_and_policy() {
    use claude_server::ServerPolicy;

    // artifacts off -> no agent_write even if mutations on
    let cfg = ServerConfig {
        policy: ServerPolicy {
            allow_mutations: true,
        },
        surfaces: SurfacesConfig {
            enable_artifacts: false,
            enable_mutations: true,
            ..Default::default()
        },
        ..Default::default()
    };
    let names = tool_names(cfg);
    assert!(!names.contains(&"agent_write".to_string()));

    // all three on -> agent_write/delete appear
    let cfg = ServerConfig {
        policy: ServerPolicy {
            allow_mutations: true,
        },
        surfaces: SurfacesConfig::default(),
        ..Default::default()
    };
    let names = tool_names(cfg);
    assert!(
        names.contains(&"agent_write".to_string()),
        "agent_write missing when all gates open; got {names:?}"
    );
}
