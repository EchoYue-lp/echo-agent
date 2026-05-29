//! Unified event bus — single tokio::broadcast channel for AgentEvents.
//!
//! Allows Webhook/Trace/UI/Audit to subscribe to the same event stream,
//! replacing the current scattered per-frontend event mapping.

use crate::agent::AgentEvent;
use std::sync::Arc;
use tokio::sync::broadcast;

/// Wraps an AgentEvent with run context for multi-agent filtering.
#[derive(Debug, Clone)]
pub struct BusEvent {
    pub event: Arc<AgentEvent>,
    /// Run ID this event belongs to (None = global/system event).
    pub run_id: Option<String>,
    /// Agent name that produced this event.
    pub agent_id: Option<String>,
}

/// Unified event bus. Subscribe with `subscribe()`, publish with `send()`.
/// Capacity 1024 to handle batch eval runs without dropping events.
pub struct EventBus {
    sender: broadcast::Sender<Arc<BusEvent>>,
}

impl EventBus {
    pub fn new(capacity: usize) -> Self {
        let (sender, _) = broadcast::channel(capacity);
        Self { sender }
    }

    pub fn subscribe(&self) -> broadcast::Receiver<Arc<BusEvent>> {
        self.sender.subscribe()
    }

    /// Send an event with run context.
    pub fn send_with(&self, event: AgentEvent, run_id: Option<String>, agent_id: Option<String>) {
        let _ = self.sender.send(Arc::new(BusEvent {
            event: Arc::new(event),
            run_id,
            agent_id,
        }));
    }
    pub fn send(&self, event: AgentEvent) {
        self.send_with(event, None, None);
    }
    pub fn send_for_run(&self, event: AgentEvent, run_id: &str) {
        self.send_with(event, Some(run_id.to_string()), None);
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
