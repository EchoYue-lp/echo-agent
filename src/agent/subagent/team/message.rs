//! TeamMessage — serializable multi-agent communication protocol.
//!
//! Messages between team members are structured and serializable for
//! replay, audit, and debugging.

use serde::{Deserialize, Serialize};

/// Kinds of messages exchanged between team members.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum MessageKind {
    /// Manager assigns a sub-task to a worker.
    Plan { sub_tasks: Vec<String> },
    /// Worker asks a question to the manager.
    Question { text: String },
    /// Worker reports a result.
    Result { output: String, success: bool },
    /// Reviewer critiques a result.
    Critique { score: f64, feedback: String },
    /// Status update from any member.
    Status { message: String },
}

/// A structured message between two team members.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TeamMessage {
    /// Sender agent name.
    pub from: String,
    /// Recipient agent name.
    pub to: String,
    /// Message kind + payload.
    pub kind: MessageKind,
    /// Sequence number within the team conversation.
    pub seq: u64,
    /// Parent message seq (for reply chains).
    pub in_reply_to: Option<u64>,
}

impl TeamMessage {
    pub fn new(from: &str, to: &str, kind: MessageKind, seq: u64) -> Self {
        Self {
            from: from.into(),
            to: to.into(),
            kind,
            seq,
            in_reply_to: None,
        }
    }

    pub fn reply_to(&self, kind: MessageKind, seq: u64) -> Self {
        Self {
            from: self.to.clone(),
            to: self.from.clone(),
            kind,
            seq,
            in_reply_to: Some(self.seq),
        }
    }

    /// Serialize for storage/replay.
    pub fn to_json(&self) -> String {
        serde_json::to_string(self).unwrap_or_default()
    }

    /// Deserialize from stored/replayed data.
    pub fn from_json(json: &str) -> Option<Self> {
        serde_json::from_str(json).ok()
    }
}
