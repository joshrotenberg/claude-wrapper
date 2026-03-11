//! Inter-slot messaging system.
//!
//! Provides a pull-based message bus for slots to communicate with each other.

use chrono::{DateTime, Utc};
use dashmap::DashMap;
use serde::{Deserialize, Serialize};

use crate::types::SlotId;

/// A message sent from one slot to another.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Message {
    /// Unique message identifier.
    pub id: String,
    /// Slot that sent the message.
    pub from: SlotId,
    /// Slot that receives the message.
    pub to: SlotId,
    /// Message content.
    pub content: String,
    /// When the message was sent.
    pub timestamp: DateTime<Utc>,
}

/// Message bus for inter-slot communication.
///
/// Implements pull-based messaging where slots can send, read, and peek
/// messages without blocking senders.
pub struct MessageBus {
    /// Per-slot inbox: slot ID -> messages.
    inboxes: DashMap<String, Vec<Message>>,
}

impl MessageBus {
    /// Create a new message bus.
    pub fn new() -> Self {
        Self {
            inboxes: DashMap::new(),
        }
    }

    /// Send a message from one slot to another.
    ///
    /// Pushes the message to the recipient's inbox for later retrieval.
    pub fn send(&self, from: &SlotId, to: &SlotId, content: String) -> String {
        use std::time::{SystemTime, UNIX_EPOCH};
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let message_id = format!("msg-{:x}", nanos);

        let message = Message {
            id: message_id.clone(),
            from: from.clone(),
            to: to.clone(),
            content,
            timestamp: Utc::now(),
        };

        self.inboxes.entry(to.0.clone()).or_default().push(message);

        message_id
    }

    /// Read and drain all messages for a slot.
    ///
    /// Removes all messages from the slot's inbox and returns them.
    pub fn read(&self, slot_id: &SlotId) -> Vec<Message> {
        self.inboxes
            .remove(&slot_id.0)
            .map(|(_, messages)| messages)
            .unwrap_or_default()
    }

    /// Peek at messages without draining.
    ///
    /// Returns all messages in the slot's inbox without removing them.
    pub fn peek(&self, slot_id: &SlotId) -> Vec<Message> {
        self.inboxes
            .get(&slot_id.0)
            .map(|inbox| inbox.clone())
            .unwrap_or_default()
    }

    /// Get the number of messages in a slot's inbox.
    pub fn count(&self, slot_id: &SlotId) -> usize {
        self.inboxes
            .get(&slot_id.0)
            .map(|inbox| inbox.len())
            .unwrap_or(0)
    }
}

impl Default for MessageBus {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_send_and_read() {
        let bus = MessageBus::new();
        let from = SlotId("slot-0".to_string());
        let to = SlotId("slot-1".to_string());

        let msg_id = bus.send(&from, &to, "hello".to_string());
        assert!(!msg_id.is_empty());

        let messages = bus.read(&to);
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].from, from);
        assert_eq!(messages[0].to, to);
        assert_eq!(messages[0].content, "hello");

        // Should be drained
        let messages = bus.read(&to);
        assert_eq!(messages.len(), 0);
    }

    #[test]
    fn test_send_and_peek() {
        let bus = MessageBus::new();
        let from = SlotId("slot-0".to_string());
        let to = SlotId("slot-1".to_string());

        bus.send(&from, &to, "message 1".to_string());
        bus.send(&from, &to, "message 2".to_string());

        // Peek should not drain
        let messages = bus.peek(&to);
        assert_eq!(messages.len(), 2);

        let messages = bus.peek(&to);
        assert_eq!(messages.len(), 2);

        // Read should drain
        let messages = bus.read(&to);
        assert_eq!(messages.len(), 2);

        let messages = bus.peek(&to);
        assert_eq!(messages.len(), 0);
    }

    #[test]
    fn test_multiple_senders() {
        let bus = MessageBus::new();
        let slot_0 = SlotId("slot-0".to_string());
        let slot_1 = SlotId("slot-1".to_string());
        let slot_2 = SlotId("slot-2".to_string());

        bus.send(&slot_0, &slot_2, "from 0".to_string());
        bus.send(&slot_1, &slot_2, "from 1".to_string());

        let messages = bus.read(&slot_2);
        assert_eq!(messages.len(), 2);
        assert_eq!(messages[0].from, slot_0);
        assert_eq!(messages[1].from, slot_1);
    }

    #[test]
    fn test_empty_inbox() {
        let bus = MessageBus::new();
        let slot = SlotId("slot-0".to_string());

        assert_eq!(bus.count(&slot), 0);
        assert_eq!(bus.peek(&slot).len(), 0);
        assert_eq!(bus.read(&slot).len(), 0);
    }

    #[test]
    fn test_count() {
        let bus = MessageBus::new();
        let from = SlotId("slot-0".to_string());
        let to = SlotId("slot-1".to_string());

        assert_eq!(bus.count(&to), 0);

        bus.send(&from, &to, "msg 1".to_string());
        assert_eq!(bus.count(&to), 1);

        bus.send(&from, &to, "msg 2".to_string());
        assert_eq!(bus.count(&to), 2);

        bus.read(&to);
        assert_eq!(bus.count(&to), 0);
    }
}
