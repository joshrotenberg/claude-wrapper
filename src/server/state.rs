//! Server-side state shared across MCP handlers.
//!
//! [`ServerState`] is what handlers extract via tower-mcp's `State`
//! extractor. It bundles the shared [`Claude`] client, the optional
//! global [`BudgetTracker`], the chat registry for Surface B
//! sessions, and the [`ServerConfig`] so handlers can apply server
//! defaults.

use std::collections::HashMap;
use std::sync::{Arc, RwLock};

use crate::Claude;
use crate::budget::BudgetTracker;
use crate::session::Session;

use super::config::ServerConfig;

/// Opaque server-issued identifier for a Surface B chat. Returned by
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

/// State shared across every Surface A and Surface B handler.
#[derive(Clone)]
pub struct ServerState {
    pub(crate) claude: Arc<Claude>,
    pub(crate) budget: Option<BudgetTracker>,
    pub(crate) chats: Arc<ChatRegistry>,
    pub(crate) config: Arc<ServerConfig>,
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
        }
    }
}
