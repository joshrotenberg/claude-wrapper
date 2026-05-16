//! Read-only access to git worktrees, exposed as MCP tools and
//! resources. Backed by [`claude_wrapper::worktrees::WorktreeRoot`].
//!
//! Useful for hosts that orchestrate worktree-isolated chats via
//! `chat_open(worktree=true, worktree_name=...)` -- the host can
//! later list the worktrees its chats produced and decide what to
//! do with them.
//!
//! Tools:
//! - `worktree_list { repo_path? }` -- enumerate worktrees for a
//!   given repo. `repo_path` defaults to the server's resolved
//!   default repo (see [`crate::config::ServerConfig::worktrees_root`]).
//!
//! Resources:
//! - `claude://worktrees` -- same shape as `worktree_list` against
//!   the server's default repo.
//!
//! Mutations (worktree_remove, prune) are not yet wired -- they
//! belong behind the `mutations` feature double-gate.

use std::path::PathBuf;

use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::json;
use tower_mcp::protocol::ReadResourceResult;
use tower_mcp::resource::ResourceTemplate;
use tower_mcp::{CallToolResult, Resource, ResourceBuilder, Tool, ToolBuilder};

use claude_wrapper::worktrees::{Worktree, WorktreeRoot};

use crate::state::ServerState;

/// Build the worktrees-feature tool list.
pub(crate) fn tools(state: &ServerState) -> Vec<Tool> {
    vec![tool_worktree_list(state)]
}

/// Build the worktrees-feature resource list.
pub(crate) fn resources(state: &ServerState) -> Vec<Resource> {
    vec![resource_worktrees(state)]
}

/// Build the worktrees-feature resource templates.
/// Empty for the first cut; per-repo templates may come later.
pub(crate) fn templates(_state: &ServerState) -> Vec<ResourceTemplate> {
    Vec::new()
}

// -- tool_worktree_list --------------------------------------------

#[derive(Debug, Default, Deserialize, JsonSchema)]
struct WorktreeListInput {
    /// Optional path inside the target repo. Defaults to the
    /// server's resolved default repo (config.worktrees_root,
    /// then config.claude.working_dir, then process cwd).
    #[serde(default)]
    repo_path: Option<PathBuf>,
}

fn tool_worktree_list(state: &ServerState) -> Tool {
    let state = state.clone();
    ToolBuilder::new("worktree_list")
        .description(
            "Enumerate git worktrees for a repository. Optional \
             `repo_path` selects which repo (defaults to the \
             server's resolved default). Each entry: `path`, `head`, \
             `branch`, `is_main`, `is_detached`, `is_bare`, \
             `is_locked` (with optional `lock_reason`), `is_prunable` \
             (with optional `prune_reason`). The first entry in the \
             list is always the main worktree. Read-only.",
        )
        .read_only()
        .handler(move |input: WorktreeListInput| {
            let state = state.clone();
            async move {
                let path = resolve_repo_path(&state, input.repo_path);
                let root = WorktreeRoot::for_repo(path);
                let wts = root.list().map_err(crate::errors::from_wrapper)?;
                Ok(CallToolResult::json(json!({
                    "worktrees": wts.iter().map(worktree_to_json).collect::<Vec<_>>(),
                })))
            }
        })
        .build()
}

// -- resource: claude://worktrees -----------------------------------

fn resource_worktrees(state: &ServerState) -> Resource {
    let state = state.clone();
    ResourceBuilder::new("claude://worktrees")
        .name("Git worktrees")
        .description(
            "Live view of git worktrees for the server's default \
             repo. Same shape as the worktree_list tool.",
        )
        .mime_type("application/json")
        .handler(move || {
            let state = state.clone();
            async move {
                let path = resolve_repo_path(&state, None);
                let root = WorktreeRoot::for_repo(path);
                let wts = root.list().map_err(crate::errors::from_wrapper)?;
                let body = json!({
                    "worktrees": wts.iter().map(worktree_to_json).collect::<Vec<_>>(),
                });
                let text = serde_json::to_string_pretty(&body).unwrap_or_default();
                Ok(ReadResourceResult::text("claude://worktrees", text))
            }
        })
        .build()
}

// -- helpers --------------------------------------------------------

/// Resolution order for the target repo path:
/// 1. caller-provided `repo_path` argument
/// 2. `config.worktrees_root` (per-server default)
/// 3. `config.claude.working_dir` (server's spawn cwd)
/// 4. process cwd (last resort)
fn resolve_repo_path(state: &ServerState, explicit: Option<PathBuf>) -> PathBuf {
    if let Some(p) = explicit {
        return p;
    }
    if let Some(p) = state.config.worktrees_root.as_ref() {
        return p.clone();
    }
    if let Some(p) = state.config.claude.working_dir.as_ref() {
        return p.clone();
    }
    std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."))
}

fn worktree_to_json(w: &Worktree) -> serde_json::Value {
    json!({
        "path": w.path,
        "head": w.head,
        "branch": w.branch,
        "is_main": w.is_main,
        "is_detached": w.is_detached,
        "is_bare": w.is_bare,
        "is_locked": w.is_locked,
        "lock_reason": w.lock_reason,
        "is_prunable": w.is_prunable,
        "prune_reason": w.prune_reason,
    })
}
