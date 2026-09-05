//! Full event, replay and gap views for the extension profile.
//!
//! The framework `EventEnvelope` (echo-core `event_envelope.rs`) is the event
//! authority: schema version, stable identity, monotonic sequence starting
//! at 1, parent link, content hash, timestamp and `AgentEvent` payload. The
//! DTO here carries every one of those facts losslessly over
//! `_echo_agent/*` notifications (design §11.1) — sequence numbers are never
//! re-numbered, identities are never replaced by SDK-local ids, and the
//! payload is transported as the framework's own serialized event.
//!
//! The standard ACP `session/update` projection of the same event is
//! produced separately by the future adapter and is allowed to be a bounded
//! view; whatever it drops must remain observable here.

use serde::{Deserialize, Serialize};

use crate::scalar::{WireTimestamp, WireU64, WireUnknown};

/// Lossless wire view of one framework `EventEnvelope`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct WireEventEnvelope {
    /// Framework event schema version (currently 4).
    pub schema_version: u16,
    /// Stable event identity (non-empty).
    pub event_id: String,
    /// Integrity digest of the parent link and payload slot.
    pub content_hash: String,
    /// Monotonic sequence within the stream, starting at 1.
    pub sequence: WireU64,
    /// Identity facts preserved one-to-one from the framework envelope.
    pub stream_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub conversation_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub run_id: Option<String>,
    pub turn_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub execution_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent_event_id: Option<String>,
    /// Authoritative timestamp (nanoseconds since Unix epoch).
    pub timestamp: WireTimestamp,
    /// The framework `AgentEvent` serialized verbatim. Unknown additive
    /// variants survive as JSON for older SDKs (design §18).
    pub payload: serde_json::Value,
    /// Typed view of a payload the local contract revision does not know.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub unknown: Option<WireUnknown>,
}

impl WireEventEnvelope {
    /// Structural validation without panicking: non-empty identities and a
    /// sequence of at least 1.
    pub fn validate(&self) -> Result<(), &'static str> {
        if self.event_id.trim().is_empty() {
            return Err("event_id must be non-empty");
        }
        if self.stream_id.trim().is_empty() {
            return Err("stream_id must be non-empty");
        }
        if self.turn_id.trim().is_empty() {
            return Err("turn_id must be non-empty");
        }
        match self.sequence.to_u64() {
            Some(sequence) if sequence >= 1 => {}
            _ => return Err("sequence must be a canonical u64 >= 1"),
        }
        Ok(())
    }
}

/// `_echo_agent/event` notification body: one accepted framework event,
/// delivered to consumers that negotiated the extension profile.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct EventNotification {
    pub envelope: WireEventEnvelope,
}

/// Cursor semantics: consumers acknowledge the last sequence they processed;
/// replay resumes strictly after it (design §11.2).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct EventCursor {
    pub stream_id: String,
    /// Last sequence the consumer has fully processed.
    pub last_processed_sequence: WireU64,
}

/// `_echo_agent/run/replay` request.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct ReplayRequest {
    pub stream_id: String,
    /// Replay starts strictly after this sequence.
    pub after_sequence: WireU64,
    /// Upper bound on returned events; the Host enforces its own maximum.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_events: Option<WireU64>,
}

/// `_echo_agent/run/replay` response. `next_cursor` is the position to
/// resume from; `gap` is set when the requested window fell below the
/// retained watermark.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct ReplayResponse {
    pub events: Vec<WireEventEnvelope>,
    pub next_cursor: EventCursor,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub gap: Option<EventGap>,
}

/// A hole in the retained event history. Events are incremental facts
/// (design §11.2): after a gap the consumer must consult the snapshot at
/// `snapshot_watermark` instead of inferring state from partial events.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct EventGap {
    /// First missing sequence (inclusive).
    pub from_sequence: WireU64,
    /// Last missing sequence (inclusive).
    pub to_sequence: WireU64,
    /// Why the events are no longer available (retention floor, restart, ...).
    pub reason: String,
    /// Sequence at which a snapshot may be queried to rebuild state.
    pub snapshot_watermark: WireU64,
}

/// `_echo_agent/gap` notification body: live consumers crossing the
/// retention floor receive this instead of silently dropped events.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct GapNotification {
    pub gap: EventGap,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn envelope(sequence: u64) -> WireEventEnvelope {
        WireEventEnvelope {
            schema_version: 4,
            event_id: format!("event-{sequence}"),
            content_hash: "sha256:deadbeef".to_string(),
            sequence: WireU64::from_u64(sequence),
            stream_id: "stream-1".to_string(),
            conversation_id: None,
            run_id: Some("run-1".to_string()),
            turn_id: "turn-1".to_string(),
            message_id: None,
            execution_id: None,
            parent_event_id: None,
            timestamp: WireTimestamp::from_nanos(1_757_000_000_000_000_000),
            payload: serde_json::json!({"kind": "agent_message"}),
            unknown: None,
        }
    }

    #[test]
    fn envelope_round_trip_preserves_every_identity_fact() {
        let event = envelope(3);
        let json = serde_json::to_string(&event).unwrap_or_default();
        let back: WireEventEnvelope = serde_json::from_str(&json).unwrap_or(envelope(0));
        assert_eq!(back.event_id, "event-3");
        assert_eq!(back.sequence.to_u64(), Some(3));
        assert_eq!(back.run_id.as_deref(), Some("run-1"));
        assert_eq!(back.payload, serde_json::json!({"kind": "agent_message"}));
        assert!(back.validate().is_ok());
    }

    #[test]
    fn sequence_zero_is_invalid() {
        let mut event = envelope(1);
        event.sequence = WireU64::from_u64(0);
        assert!(event.validate().is_err());
    }

    #[test]
    fn gap_round_trip() {
        let gap = EventGap {
            from_sequence: WireU64::from_u64(5),
            to_sequence: WireU64::from_u64(9),
            reason: "retention floor".to_string(),
            snapshot_watermark: WireU64::from_u64(9),
        };
        let json = serde_json::to_string(&ReplayResponse {
            events: vec![],
            next_cursor: EventCursor {
                stream_id: "stream-1".to_string(),
                last_processed_sequence: WireU64::from_u64(4),
            },
            gap: Some(gap.clone()),
        })
        .unwrap_or_default();
        let back: ReplayResponse = serde_json::from_str(&json).unwrap_or(ReplayResponse {
            events: vec![],
            next_cursor: EventCursor {
                stream_id: String::new(),
                last_processed_sequence: WireU64::from_u64(0),
            },
            gap: None,
        });
        assert_eq!(back.gap, Some(gap));
    }
}
