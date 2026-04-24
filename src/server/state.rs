//! Server-side state shared across MCP handlers.
//!
//! [`ServerState`] is what handlers extract via tower-mcp's `State`
//! extractor. It bundles the shared [`Claude`] client, the optional
//! global [`BudgetTracker`], the chat registry for agent surface
//! sessions, and the [`ServerConfig`] so handlers can apply server
//! defaults.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};

use crate::Claude;
use crate::budget::BudgetTracker;
use crate::session::Session;

use super::config::ServerConfig;

/// Opaque server-issued identifier for an agent surface chat. Returned by
/// `agent.chat.open` and threaded back into `agent.chat.send` /
/// `agent.chat.close`.
pub type ChatId = String;

/// Holds the live multi-turn sessions for `agent.chat.*` tools.
///
/// Sessions accumulate `session_id`, history, cumulative cost, and
/// optionally a per-chat [`BudgetTracker`]. Sessions are dropped when
/// `agent.chat.close` runs or when the server shuts down.
#[derive(Default)]
pub(crate) struct ChatRegistry {
    chats: RwLock<HashMap<ChatId, Session>>,
}

impl ChatRegistry {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// Insert a fresh session and return its server-issued id.
    pub(crate) fn open(&self, session: Session) -> ChatId {
        let id = ulid::Ulid::new().to_string();
        let mut guard = self.chats.write().expect("chat registry poisoned");
        guard.insert(id.clone(), session);
        id
    }

    /// Close and drop a chat. Returns `true` if it existed.
    pub(crate) fn close(&self, id: &str) -> bool {
        self.chats
            .write()
            .expect("chat registry poisoned")
            .remove(id)
            .is_some()
    }

    /// Snapshot of currently open chats: id, cumulative cost, and
    /// turn count.
    pub(crate) fn list(&self) -> Vec<ChatSummary> {
        let guard = self.chats.read().expect("chat registry poisoned");
        guard
            .iter()
            .map(|(id, s)| ChatSummary {
                id: id.clone(),
                session_id: s.id().map(str::to_string),
                total_cost_usd: s.total_cost_usd(),
                total_turns: s.total_turns(),
            })
            .collect()
    }

    /// Apply `f` to the named chat's session; returns the result, or
    /// `None` if the chat is not registered.
    ///
    /// Holds the write lock for the duration of `f` because [`Session`]
    /// methods take `&mut self`. Don't do long-running work inside;
    /// they call out to the CLI which spans seconds. We accept this
    /// for v0 -- chat-level concurrency means one in-flight call per
    /// chat at a time, which matches user expectations for a chat.
    pub(crate) fn with_session<F, R>(&self, id: &str, f: F) -> Option<R>
    where
        F: FnOnce(&mut Session) -> R,
    {
        let mut guard = self.chats.write().expect("chat registry poisoned");
        guard.get_mut(id).map(f)
    }
}

#[derive(Debug, serde::Serialize, schemars::JsonSchema)]
pub(crate) struct ChatSummary {
    pub id: ChatId,
    pub session_id: Option<String>,
    pub total_cost_usd: f64,
    pub total_turns: u32,
}

/// State shared across every cli surface and agent surface handler.
#[derive(Clone)]
pub struct ServerState {
    pub(crate) claude: Arc<Claude>,
    pub(crate) budget: Option<BudgetTracker>,
    pub(crate) chats: Arc<ChatRegistry>,
    pub(crate) config: Arc<ServerConfig>,
    /// Per-cwd serialization for CLI invocations.
    ///
    /// Claude maintains per-project state under
    /// `~/.claude/projects/<cwd-hash>/` (settings cache, MCP probes,
    /// project-choice records). Concurrent CLI invocations against
    /// the same cwd race on that state. We serialize per-cwd so
    /// different cwds run in parallel but the same cwd does not.
    ///
    /// In v0 there is one cwd per server (set at startup), so this
    /// effectively single-threads CLI calls. The structure is
    /// per-cwd so per-call working_dir overrides land cleanly later.
    cli_locks: Arc<RwLock<HashMap<PathBuf, Arc<tokio::sync::Mutex<()>>>>>,
}

impl ServerState {
    pub(crate) fn new(claude: Arc<Claude>, config: Arc<ServerConfig>) -> Self {
        let budget = config.budget.as_ref().map(|b| {
            let mut builder = BudgetTracker::builder();
            if let Some(max) = b.max_usd {
                builder = builder.max_usd(max);
            }
            if let Some(warn) = b.warn_at_usd {
                builder = builder.warn_at_usd(warn);
            }
            builder.build()
        });
        Self {
            claude,
            budget,
            chats: Arc::new(ChatRegistry::new()),
            config,
            cli_locks: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Acquire the per-cwd CLI mutex, holding it until the returned
    /// guard is dropped. Call before invoking the wrapper's CLI
    /// path; release naturally when the guard goes out of scope.
    pub(crate) async fn lock_cwd(&self, cwd: &Path) -> tokio::sync::OwnedMutexGuard<()> {
        let key = cwd.to_path_buf();
        // Clone the Arc out of the read guard's scope before awaiting
        // so the guard does not span the await point. std's
        // RwLockReadGuard is !Send, so holding it across .await would
        // demote the surrounding future and tower-mcp rejects it.
        let existing = self
            .cli_locks
            .read()
            .expect("cli_locks poisoned")
            .get(&key)
            .cloned();
        if let Some(m) = existing {
            return m.lock_owned().await;
        }
        // First time for this cwd: take the write lock, insert, drop guard.
        let m = {
            let mut guard = self.cli_locks.write().expect("cli_locks poisoned");
            guard
                .entry(key)
                .or_insert_with(|| Arc::new(tokio::sync::Mutex::new(())))
                .clone()
        };
        m.lock_owned().await
    }

    /// Convenience: lock for the configured Claude client's cwd, or
    /// the process cwd if none is set.
    pub(crate) async fn lock_default_cwd(&self) -> tokio::sync::OwnedMutexGuard<()> {
        let cwd = self
            .claude
            .working_dir()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));
        self.lock_cwd(&cwd).await
    }
}
