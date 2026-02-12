//! L0 Hot Buffer: ring buffer holding the most recent messages.
//!
//! The hot buffer is the primary working set — always contains the system prompt
//! plus the K most recent user/assistant/tool messages. When full, the oldest
//! message is evicted for promotion to L1.

use std::collections::VecDeque;

use cuervo_core::types::ChatMessage;

use crate::accountant::estimate_message_tokens;

/// L0: Ring buffer holding the most recent messages.
pub struct HotBuffer {
    messages: VecDeque<ChatMessage>,
    capacity: usize,
    token_count: u32,
}

impl HotBuffer {
    /// Create a new hot buffer with the given message capacity.
    pub fn new(capacity: usize) -> Self {
        Self {
            messages: VecDeque::with_capacity(capacity),
            capacity,
            token_count: 0,
        }
    }

    /// Append a message. Returns the evicted message if the buffer was full.
    pub fn push(&mut self, msg: ChatMessage) -> Option<ChatMessage> {
        let tokens = estimate_message_tokens(&msg);
        self.token_count += tokens;

        let evicted = if self.messages.len() >= self.capacity {
            let old = self.messages.pop_front();
            if let Some(ref m) = old {
                self.token_count = self.token_count.saturating_sub(estimate_message_tokens(m));
            }
            old
        } else {
            None
        };

        self.messages.push_back(msg);
        evicted
    }

    /// Borrow the message buffer for ModelRequest construction (no clone).
    pub fn messages(&self) -> &VecDeque<ChatMessage> {
        &self.messages
    }

    /// Clone all messages into a Vec for ModelRequest.
    pub fn to_message_vec(&self) -> Vec<ChatMessage> {
        self.messages.iter().cloned().collect()
    }

    /// Current estimated token count across all buffered messages.
    pub fn token_count(&self) -> u32 {
        self.token_count
    }

    /// Number of messages in the buffer.
    pub fn len(&self) -> usize {
        self.messages.len()
    }

    /// Whether the buffer is empty.
    pub fn is_empty(&self) -> bool {
        self.messages.is_empty()
    }

    /// Maximum capacity.
    pub fn capacity(&self) -> usize {
        self.capacity
    }

    /// Whether the buffer is at capacity.
    pub fn is_full(&self) -> bool {
        self.messages.len() >= self.capacity
    }

    /// Pop the oldest message (for forced eviction).
    pub fn pop_oldest(&mut self) -> Option<ChatMessage> {
        let msg = self.messages.pop_front()?;
        self.token_count = self.token_count.saturating_sub(estimate_message_tokens(&msg));
        Some(msg)
    }

    /// Drain all messages (for session rebuild/reset).
    pub fn drain(&mut self) -> Vec<ChatMessage> {
        self.token_count = 0;
        self.messages.drain(..).collect()
    }

    /// Clear the buffer.
    pub fn clear(&mut self) {
        self.messages.clear();
        self.token_count = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cuervo_core::types::{MessageContent, Role};

    fn text_msg(role: Role, text: &str) -> ChatMessage {
        ChatMessage {
            role,
            content: MessageContent::Text(text.to_string()),
        }
    }

    #[test]
    fn new_buffer_is_empty() {
        let buf = HotBuffer::new(8);
        assert!(buf.is_empty());
        assert_eq!(buf.len(), 0);
        assert_eq!(buf.token_count(), 0);
        assert_eq!(buf.capacity(), 8);
    }

    #[test]
    fn push_within_capacity() {
        let mut buf = HotBuffer::new(4);
        let evicted = buf.push(text_msg(Role::User, "hello"));
        assert!(evicted.is_none());
        assert_eq!(buf.len(), 1);
        assert!(buf.token_count() > 0);
    }

    #[test]
    fn push_at_capacity_evicts_oldest() {
        let mut buf = HotBuffer::new(2);
        buf.push(text_msg(Role::User, "first"));
        buf.push(text_msg(Role::Assistant, "second"));
        assert_eq!(buf.len(), 2);
        assert!(buf.is_full());

        let evicted = buf.push(text_msg(Role::User, "third"));
        assert!(evicted.is_some());
        let evicted = evicted.unwrap();
        assert_eq!(evicted.content.as_text().unwrap(), "first");
        assert_eq!(buf.len(), 2);
    }

    #[test]
    fn token_count_tracks_additions() {
        let mut buf = HotBuffer::new(10);
        buf.push(text_msg(Role::User, "hello")); // 5 chars → 2 tokens
        let t1 = buf.token_count();
        buf.push(text_msg(Role::User, "world")); // 5 chars → 2 tokens
        let t2 = buf.token_count();
        assert!(t2 > t1);
    }

    #[test]
    fn token_count_adjusts_on_eviction() {
        let mut buf = HotBuffer::new(2);
        buf.push(text_msg(Role::User, "short"));
        buf.push(text_msg(Role::User, "also short"));
        let before = buf.token_count();

        buf.push(text_msg(Role::User, "x")); // evicts "short"
        let after = buf.token_count();
        // After should be less since "short" was evicted and "x" is smaller
        assert!(after < before);
    }

    #[test]
    fn to_message_vec() {
        let mut buf = HotBuffer::new(4);
        buf.push(text_msg(Role::User, "msg1"));
        buf.push(text_msg(Role::Assistant, "msg2"));
        let vec = buf.to_message_vec();
        assert_eq!(vec.len(), 2);
    }

    #[test]
    fn pop_oldest() {
        let mut buf = HotBuffer::new(4);
        buf.push(text_msg(Role::User, "first"));
        buf.push(text_msg(Role::User, "second"));

        let oldest = buf.pop_oldest();
        assert!(oldest.is_some());
        assert_eq!(oldest.unwrap().content.as_text().unwrap(), "first");
        assert_eq!(buf.len(), 1);
    }

    #[test]
    fn pop_oldest_empty() {
        let mut buf = HotBuffer::new(4);
        assert!(buf.pop_oldest().is_none());
    }

    #[test]
    fn drain_empties_buffer() {
        let mut buf = HotBuffer::new(4);
        buf.push(text_msg(Role::User, "msg1"));
        buf.push(text_msg(Role::User, "msg2"));
        buf.push(text_msg(Role::User, "msg3"));

        let drained = buf.drain();
        assert_eq!(drained.len(), 3);
        assert!(buf.is_empty());
        assert_eq!(buf.token_count(), 0);
    }

    #[test]
    fn clear_resets_everything() {
        let mut buf = HotBuffer::new(4);
        buf.push(text_msg(Role::User, "msg1"));
        buf.push(text_msg(Role::User, "msg2"));
        buf.clear();
        assert!(buf.is_empty());
        assert_eq!(buf.token_count(), 0);
    }

    #[test]
    fn messages_borrow() {
        let mut buf = HotBuffer::new(4);
        buf.push(text_msg(Role::User, "msg1"));
        buf.push(text_msg(Role::Assistant, "msg2"));
        let msgs = buf.messages();
        assert_eq!(msgs.len(), 2);
    }

    #[test]
    fn capacity_one_always_latest() {
        let mut buf = HotBuffer::new(1);
        buf.push(text_msg(Role::User, "first"));
        buf.push(text_msg(Role::User, "second"));
        buf.push(text_msg(Role::User, "third"));
        assert_eq!(buf.len(), 1);
        assert_eq!(
            buf.messages().front().unwrap().content.as_text().unwrap(),
            "third"
        );
    }
}
