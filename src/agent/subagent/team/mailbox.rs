//! Team mailbox — async message-passing for inter-agent communication

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use tokio::sync::{Mutex, mpsc};

use crate::error::{ReactError, Result};

// ── Message Types ─────────────────────────────────────────────────────────────

/// Kinds of messages exchanged between team members.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MessageKind {
    /// Leader assigns a task to a teammate.
    TaskAssigned {
        /// Task description.
        task: String,
        /// Context data for the task.
        context: HashMap<String, String>,
    },
    /// Teammate reports a task result.
    TaskResult {
        /// Task description.
        task: String,
        /// Result output.
        result: String,
        /// Whether the task succeeded.
        success: bool,
    },
    /// Teammate requests clarification.
    Query {
        /// Question text.
        question: String,
    },
    /// Leader responds to a query.
    QueryResponse {
        /// Answer text.
        answer: String,
    },
    /// Status update from teammate.
    Status {
        /// Status message.
        message: String,
    },
    /// Cancellation notice.
    Cancelled,
}

/// A message in the mailbox system.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MailboxMessage {
    /// Sender agent name.
    pub from: String,
    /// Recipient agent name.
    pub to: String,
    /// Message content.
    pub kind: MessageKind,
    /// Timestamp (seconds since epoch).
    pub ts: u64,
}

impl MailboxMessage {
    /// Create a new mailbox message with current timestamp.
    ///
    /// # 参数
    /// * `from` - Sender agent name.
    /// * `to` - Recipient agent name.
    /// * `kind` - Message kind.
    pub fn new(from: impl Into<String>, to: impl Into<String>, kind: MessageKind) -> Self {
        Self {
            from: from.into(),
            to: to.into(),
            kind,
            ts: crate::utils::time::now_secs(),
        }
    }
}

// ── Mailbox ───────────────────────────────────────────────────────────────────

const MAILBOX_CAPACITY: usize = 256;

/// Async mailbox for a single agent.
///
/// Uses `tokio::sync::mpsc` for point-to-point messaging.
/// Each team member gets their own mailbox.
pub struct Mailbox {
    tx: mpsc::Sender<MailboxMessage>,
    rx: Mutex<mpsc::Receiver<MailboxMessage>>,
}

impl Mailbox {
    /// Create a new mailbox.
    pub fn new() -> Self {
        let (tx, rx) = mpsc::channel(MAILBOX_CAPACITY);
        Self {
            tx,
            rx: Mutex::new(rx),
        }
    }

    /// Create with custom capacity.
    pub fn with_capacity(capacity: usize) -> Self {
        let (tx, rx) = mpsc::channel(capacity);
        Self {
            tx,
            rx: Mutex::new(rx),
        }
    }

    /// Send a message (non-blocking, returns error if full).
    pub async fn send(&self, msg: MailboxMessage) -> Result<()> {
        self.tx
            .send(msg)
            .await
            .map_err(|e| ReactError::Other(format!("Mailbox send error: {}", e)))
    }

    /// Receive the next message (blocking await).
    pub async fn recv(&self) -> Option<MailboxMessage> {
        let mut rx = self.rx.lock().await;
        rx.recv().await
    }

    /// Try to receive a message without blocking.
    pub async fn try_recv(&self) -> Option<MailboxMessage> {
        let mut rx = self.rx.lock().await;
        rx.try_recv().ok()
    }

    /// Get a sender handle (for creating mailboxes in other agents).
    pub fn sender(&self) -> MailboxSender {
        MailboxSender {
            tx: self.tx.clone(),
        }
    }

    /// Check if the mailbox is closed (all senders dropped).
    pub fn is_closed(&self) -> bool {
        self.tx.is_closed()
    }
}

/// Sender half of a mailbox (can be cloned and shared).
#[derive(Clone)]
pub struct MailboxSender {
    tx: mpsc::Sender<MailboxMessage>,
}

impl MailboxSender {
    /// Send a message through the mailbox.
    ///
    /// # 参数
    /// * `msg` - Message to send.
    pub async fn send(&self, msg: MailboxMessage) -> Result<()> {
        self.tx
            .send(msg)
            .await
            .map_err(|e| ReactError::Other(format!("MailboxSender send error: {}", e)))
    }
}

impl Default for Mailbox {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_mailbox_send_recv() {
        let mailbox = Mailbox::new();

        let msg = MailboxMessage::new(
            "leader",
            "worker",
            MessageKind::TaskAssigned {
                task: "do stuff".into(),
                context: HashMap::new(),
            },
        );

        mailbox.send(msg).await.unwrap();
        let received = mailbox.recv().await.unwrap();

        assert_eq!(received.from, "leader");
        assert_eq!(received.to, "worker");
        match received.kind {
            MessageKind::TaskAssigned { task, .. } => assert_eq!(task, "do stuff"),
            _ => panic!("Wrong message kind"),
        }
    }

    #[tokio::test]
    async fn test_mailbox_sender() {
        let mailbox = Mailbox::new();
        let sender = mailbox.sender();

        sender
            .send(MailboxMessage::new(
                "a",
                "b",
                MessageKind::Status {
                    message: "ok".into(),
                },
            ))
            .await
            .unwrap();

        let received = mailbox.recv().await.unwrap();
        assert_eq!(received.from, "a");
    }

    #[tokio::test]
    async fn test_mailbox_try_recv_empty() {
        let mailbox = Mailbox::new();
        let result = mailbox.try_recv().await;
        assert!(result.is_none());
    }
}
