//! Inter-slot messaging for pool communication.
//!
//! Provides a message bus for slots to send and receive messages to each other.

use chrono::{DateTime, Utc};
use dashmap::DashMap;
use serde::{Deserialize, Serialize};

use crate::types::SlotId;

/// A message sent from one slot to another.
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Message {
    /// Unique message ID.
    pub id: String,
    /// Sender slot ID.
    pub from: SlotId,
    /// Recipient slot ID.
    pub to: SlotId,
    /// Message content.
    pub content: String,
    /// Timestamp when message was created.
    pub timestamp: DateTime<Utc>,
}

/// Message bus for inter-slot communication.
///
/// Implements a simple inbox-based messaging system where each slot
/// has a queue of messages addressed to it.
pub struct MessageBus {
    /// Inboxes keyed by slot ID, containing vectors of messages.
    inboxes: DashMap<String, Vec<Message>>,
}

impl Default for MessageBus {
    fn default() -> Self {
        Self::new()
    }
}

impl MessageBus {
    /// Create a new message bus.
    pub fn new() -> Self {
        Self {
            inboxes: DashMap::new(),
        }
    }

    /// Send a message from one slot to another, returning the message ID.
    pub fn send(&self, from: SlotId, to: SlotId, content: String) -> String {
        let message_id = generate_message_id();
        let to_key = to.0.clone();
        let message = Message {
            id: message_id.clone(),
            from,
            to,
            content,
            timestamp: Utc::now(),
        };

        self.inboxes.entry(to_key).or_default().push(message);

        message_id
    }

    /// Read and drain all messages for a slot, returning them in order.
    pub fn read(&self, slot_id: &SlotId) -> Vec<Message> {
        self.inboxes
            .remove(&slot_id.0)
            .map(|(_, messages)| messages)
            .unwrap_or_default()
    }

    /// Peek at all messages for a slot without removing them.
    pub fn peek(&self, slot_id: &SlotId) -> Vec<Message> {
        self.inboxes
            .get(&slot_id.0)
            .map(|entry| entry.clone())
            .unwrap_or_default()
    }

    /// Get the count of messages in a slot's inbox.
    pub fn count(&self, slot_id: &SlotId) -> usize {
        self.inboxes
            .get(&slot_id.0)
            .map(|entry| entry.len())
            .unwrap_or(0)
    }
}

fn generate_message_id() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    format!("msg-{nanos:x}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_send_and_read() {
        let bus = MessageBus::new();
        let slot1 = SlotId("slot-1".to_string());
        let slot2 = SlotId("slot-2".to_string());

        let msg_id = bus.send(slot1.clone(), slot2.clone(), "hello".to_string());
        assert!(!msg_id.is_empty());

        let messages = bus.read(&slot2);
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].id, msg_id);
        assert_eq!(messages[0].content, "hello");
        assert_eq!(messages[0].from, slot1);
        assert_eq!(messages[0].to, slot2);

        // Verify inbox is empty after read
        let messages_again = bus.read(&slot2);
        assert!(messages_again.is_empty());
    }

    #[test]
    fn test_send_and_peek() {
        let bus = MessageBus::new();
        let slot1 = SlotId("slot-1".to_string());
        let slot2 = SlotId("slot-2".to_string());

        bus.send(slot1.clone(), slot2.clone(), "hello".to_string());

        let messages = bus.peek(&slot2);
        assert_eq!(messages.len(), 1);

        // Verify inbox still has message after peek
        let messages_again = bus.peek(&slot2);
        assert_eq!(messages_again.len(), 1);
    }

    #[test]
    fn test_multiple_senders() {
        let bus = MessageBus::new();
        let slot1 = SlotId("slot-1".to_string());
        let slot2 = SlotId("slot-2".to_string());
        let slot3 = SlotId("slot-3".to_string());

        bus.send(slot1.clone(), slot3.clone(), "from slot1".to_string());
        bus.send(slot2.clone(), slot3.clone(), "from slot2".to_string());

        let messages = bus.read(&slot3);
        assert_eq!(messages.len(), 2);
        assert_eq!(messages[0].from, slot1);
        assert_eq!(messages[1].from, slot2);
    }

    #[test]
    fn test_empty_inbox() {
        let bus = MessageBus::new();
        let slot1 = SlotId("slot-1".to_string());

        let messages = bus.read(&slot1);
        assert!(messages.is_empty());

        let messages = bus.peek(&slot1);
        assert!(messages.is_empty());
    }

    #[test]
    fn test_count() {
        let bus = MessageBus::new();
        let slot1 = SlotId("slot-1".to_string());
        let slot2 = SlotId("slot-2".to_string());

        assert_eq!(bus.count(&slot1), 0);

        bus.send(slot1.clone(), slot2.clone(), "msg1".to_string());
        assert_eq!(bus.count(&slot2), 1);

        bus.send(slot1.clone(), slot2.clone(), "msg2".to_string());
        assert_eq!(bus.count(&slot2), 2);

        bus.read(&slot2);
        assert_eq!(bus.count(&slot2), 0);
    }
}
