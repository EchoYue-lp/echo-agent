//! Unified event bus for the versioned agent event transport contract.
//!
//! Allows Webhook/Trace/UI/Audit to subscribe to the same event stream,
//! replacing the current scattered per-frontend event mapping.

use crate::agent::EventEnvelope;
use std::sync::Arc;
use tokio::sync::broadcast;

/// Unified event bus. Subscribe with `subscribe()`, publish with `send()`.
/// Capacity 1024 to handle batch eval runs without dropping events.
pub struct EventBus {
    sender: broadcast::Sender<Arc<EventEnvelope>>,
}

impl EventBus {
    pub fn new(capacity: usize) -> Self {
        let (sender, _) = broadcast::channel(capacity);
        Self { sender }
    }

    pub fn subscribe(&self) -> broadcast::Receiver<Arc<EventEnvelope>> {
        self.sender.subscribe()
    }

    /// Publish an already sequenced event envelope.
    pub fn send(&self, event: EventEnvelope) {
        let _ = self.sender.send(Arc::new(event));
    }

    pub fn subscriber_count(&self) -> usize {
        self.sender.receiver_count()
    }
}

impl Default for EventBus {
    fn default() -> Self {
        Self::new(1024)
    }
}

/// Global event bus — capacity 1024 to handle batch eval without dropping events.
/// Subscribers that fall behind get `RecvError::Lagged` — consumers should handle this.
pub static GLOBAL_EVENT_BUS: std::sync::LazyLock<Arc<EventBus>> =
    std::sync::LazyLock::new(|| Arc::new(EventBus::new(1024)));
