//! Shared server state.
//!
//! Lives inside tool handler closures so they can reach the
//! [`Claude`] client, the active [`ServerConfig`], and the registry
//! of open chats.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use tokio::sync::{Mutex, RwLock};

use claude_wrapper::Claude;
use claude_wrapper::conversation::Conversation;

use crate::config::ServerConfig;

/// Opaque identifier for a server-held chat.
pub type ChatId = String;

/// State shared by every tool handler.
#[derive(Clone)]
pub struct ServerState {
    pub claude: Arc<Claude>,
    pub config: Arc<ServerConfig>,
    pub chats: Arc<RwLock<HashMap<ChatId, Arc<Mutex<Conversation>>>>>,
}

impl ServerState {
    pub fn new(claude: Arc<Claude>, config: Arc<ServerConfig>) -> Self {
        Self {
            claude,
            config,
            chats: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Insert a chat and return its id.
    pub async fn insert_chat(&self, conv: Conversation) -> ChatId {
        let id = new_chat_id();
        self.chats
            .write()
            .await
            .insert(id.clone(), Arc::new(Mutex::new(conv)));
        id
    }

    /// Look up a chat by id without removing it. Returns the handle
    /// so the caller can lock it for the duration of a turn.
    pub async fn get_chat(&self, id: &str) -> Option<Arc<Mutex<Conversation>>> {
        self.chats.read().await.get(id).cloned()
    }

    /// Remove a chat from the registry. Returns the handle if it
    /// existed, leaving close-and-drop to the caller.
    pub async fn remove_chat(&self, id: &str) -> Option<Arc<Mutex<Conversation>>> {
        self.chats.write().await.remove(id)
    }
}

/// Generate a fresh chat id. Combines microseconds-since-epoch with a
/// monotonic counter to dodge same-microsecond collisions; not a
/// security boundary, just a unique handle.
pub fn new_chat_id() -> ChatId {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let t = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_micros())
        .unwrap_or(0);
    format!("chat_{t:x}_{n:x}")
}

#[cfg(test)]
mod tests {
    use super::new_chat_id;

    #[test]
    fn ids_are_unique_across_calls() {
        let a = new_chat_id();
        let b = new_chat_id();
        let c = new_chat_id();
        assert_ne!(a, b);
        assert_ne!(b, c);
        assert_ne!(a, c);
    }

    #[test]
    fn ids_have_chat_prefix() {
        assert!(new_chat_id().starts_with("chat_"));
    }
}
