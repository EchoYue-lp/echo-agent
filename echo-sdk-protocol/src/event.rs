//! Lossless event, replay and gap contracts for the SDK extension profile.

use chrono::{DateTime, Utc};
use echo_core::agent::{
    AgentEvent, ConversationId, EventEnvelope, EventId, ExecutionId, MessageId, RunId, StreamId,
    TurnId,
};
use serde::{Deserialize, Serialize};

use crate::scalar::{ScalarError, WireNonZeroU64, WireTimestamp, WireU64, WireValue};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct WireEventPayload {
    #[schemars(length(min = 1, max = 256))]
    pub event_type: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<WireValue>,
}

impl WireEventPayload {
    pub fn validate(&self) -> Result<(), EventWireError> {
        if self.event_type.trim().is_empty() {
            return Err(EventWireError::InvalidIdentity("event_type"));
        }
        if let Some(data) = &self.data {
            data.validate()?;
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct WireEventEnvelope {
    pub schema_version: u16,
    #[schemars(length(min = 1, max = 256))]
    pub event_id: String,
    #[schemars(regex(pattern = "^sha256:[0-9a-fA-F]{64}$"))]
    pub content_hash: String,
    pub sequence: WireNonZeroU64,
    #[schemars(length(min = 1, max = 256))]
    pub stream_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schemars(length(min = 1, max = 256))]
    pub conversation_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schemars(length(min = 1, max = 256))]
    pub run_id: Option<String>,
    #[schemars(length(min = 1, max = 256))]
    pub turn_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schemars(length(min = 1, max = 256))]
    pub message_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schemars(length(min = 1, max = 256))]
    pub execution_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schemars(length(min = 1, max = 256))]
    pub parent_event_id: Option<String>,
    pub timestamp: WireTimestamp,
    pub payload: WireEventPayload,
}

impl WireEventEnvelope {
    pub fn validate(&self) -> Result<(), EventWireError> {
        for (name, value) in [
            ("event_id", self.event_id.as_str()),
            ("stream_id", self.stream_id.as_str()),
            ("turn_id", self.turn_id.as_str()),
            ("content_hash", self.content_hash.as_str()),
        ] {
            if value.trim().is_empty() {
                return Err(EventWireError::InvalidIdentity(name));
            }
        }
        let hash_is_valid = self
            .content_hash
            .strip_prefix("sha256:")
            .is_some_and(|hex| {
                hex.chars().count() == 64
                    && hex.chars().all(|character| character.is_ascii_hexdigit())
            });
        if !hash_is_valid {
            return Err(EventWireError::InvalidIdentity("content_hash"));
        }
        if self.sequence.to_u64().is_none_or(|sequence| sequence == 0) {
            return Err(EventWireError::InvalidSequence);
        }
        self.timestamp.validate()?;
        self.payload.validate()
    }
}

#[derive(Debug)]
pub enum EventWireError {
    InvalidIdentity(&'static str),
    InvalidSequence,
    InvalidTimestamp,
    InvalidPayload(String),
    Scalar(ScalarError),
    Framework(String),
}

impl std::fmt::Display for EventWireError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidIdentity(name) => write!(formatter, "{name} must be non-empty"),
            Self::InvalidSequence => write!(formatter, "event sequence must start at one"),
            Self::InvalidTimestamp => write!(formatter, "event timestamp is out of range"),
            Self::InvalidPayload(message) => {
                write!(formatter, "invalid AgentEvent payload: {message}")
            }
            Self::Scalar(error) => write!(formatter, "invalid wire scalar: {error}"),
            Self::Framework(message) => write!(formatter, "invalid framework event: {message}"),
        }
    }
}

impl std::error::Error for EventWireError {}

impl From<ScalarError> for EventWireError {
    fn from(error: ScalarError) -> Self {
        Self::Scalar(error)
    }
}

impl TryFrom<EventEnvelope<AgentEvent>> for WireEventEnvelope {
    type Error = EventWireError;

    fn try_from(envelope: EventEnvelope<AgentEvent>) -> Result<Self, Self::Error> {
        let payload_value = serde_json::to_value(&envelope.payload)
            .map_err(|error| EventWireError::InvalidPayload(error.to_string()))?;
        let mut payload_object = payload_value
            .as_object()
            .cloned()
            .ok_or_else(|| EventWireError::InvalidPayload("expected tagged object".to_string()))?;
        let event_type = payload_object
            .remove("type")
            .and_then(|value| value.as_str().map(str::to_string))
            .ok_or_else(|| EventWireError::InvalidPayload("missing type tag".to_string()))?;
        let data = payload_object
            .remove("data")
            .map(WireValue::from_json)
            .transpose()?;
        if !payload_object.is_empty() {
            return Err(EventWireError::InvalidPayload(
                "unexpected fields outside type/data".to_string(),
            ));
        }
        let timestamp = WireTimestamp {
            unix_seconds: crate::scalar::WireI64::from_i64(envelope.timestamp.timestamp()),
            nanos: envelope.timestamp.timestamp_subsec_nanos(),
            rfc3339: Some(envelope.timestamp.to_rfc3339()),
        };
        let wire = Self {
            schema_version: envelope.schema_version,
            event_id: envelope.event_id.as_str().to_string(),
            content_hash: envelope.content_hash,
            sequence: WireNonZeroU64::try_from(envelope.sequence.to_string())?,
            stream_id: envelope.stream_id.as_str().to_string(),
            conversation_id: envelope
                .conversation_id
                .map(|value| value.as_str().to_string()),
            run_id: envelope.run_id.map(|value| value.as_str().to_string()),
            turn_id: envelope.turn_id.as_str().to_string(),
            message_id: envelope.message_id.map(|value| value.as_str().to_string()),
            execution_id: envelope
                .execution_id
                .map(|value| value.as_str().to_string()),
            parent_event_id: envelope
                .parent_event_id
                .map(|value| value.as_str().to_string()),
            timestamp,
            payload: WireEventPayload { event_type, data },
        };
        wire.validate()?;
        Ok(wire)
    }
}

impl TryFrom<WireEventEnvelope> for EventEnvelope<AgentEvent> {
    type Error = EventWireError;

    fn try_from(wire: WireEventEnvelope) -> Result<Self, Self::Error> {
        wire.validate()?;
        let mut payload = serde_json::Map::new();
        payload.insert(
            "type".to_string(),
            serde_json::Value::String(wire.payload.event_type),
        );
        if let Some(data) = wire.payload.data {
            payload.insert("data".to_string(), data.into_json()?);
        }
        let payload = serde_json::from_value(serde_json::Value::Object(payload))
            .map_err(|error| EventWireError::InvalidPayload(error.to_string()))?;
        let seconds = wire
            .timestamp
            .unix_seconds
            .to_i64()
            .ok_or(EventWireError::InvalidTimestamp)?;
        let timestamp = DateTime::<Utc>::from_timestamp(seconds, wire.timestamp.nanos)
            .ok_or(EventWireError::InvalidTimestamp)?;
        Ok(EventEnvelope {
            schema_version: wire.schema_version,
            event_id: EventId::new(wire.event_id)
                .map_err(|error| EventWireError::Framework(error.to_string()))?,
            content_hash: wire.content_hash,
            sequence: wire
                .sequence
                .to_u64()
                .ok_or(EventWireError::InvalidSequence)?,
            stream_id: StreamId::new(wire.stream_id)
                .map_err(|error| EventWireError::Framework(error.to_string()))?,
            conversation_id: wire
                .conversation_id
                .map(ConversationId::new)
                .transpose()
                .map_err(|error| EventWireError::Framework(error.to_string()))?,
            run_id: wire
                .run_id
                .map(RunId::new)
                .transpose()
                .map_err(|error| EventWireError::Framework(error.to_string()))?,
            turn_id: TurnId::new(wire.turn_id)
                .map_err(|error| EventWireError::Framework(error.to_string()))?,
            message_id: wire
                .message_id
                .map(MessageId::new)
                .transpose()
                .map_err(|error| EventWireError::Framework(error.to_string()))?,
            execution_id: wire
                .execution_id
                .map(ExecutionId::new)
                .transpose()
                .map_err(|error| EventWireError::Framework(error.to_string()))?,
            parent_event_id: wire
                .parent_event_id
                .map(EventId::new)
                .transpose()
                .map_err(|error| EventWireError::Framework(error.to_string()))?,
            timestamp,
            payload,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct EventNotification {
    pub envelope: WireEventEnvelope,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct EventCursor {
    #[schemars(length(min = 1, max = 256))]
    pub stream_id: String,
    pub last_processed_sequence: WireU64,
}

impl EventCursor {
    pub fn validate(&self) -> Result<(), EventWireError> {
        if self.stream_id.trim().is_empty() {
            Err(EventWireError::InvalidIdentity("stream_id"))
        } else {
            Ok(())
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct ReplayRequest {
    #[schemars(length(min = 1, max = 256))]
    pub stream_id: String,
    pub after_sequence: WireU64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_events: Option<WireNonZeroU64>,
}

impl ReplayRequest {
    pub fn validate(&self) -> Result<(), EventWireError> {
        if self.stream_id.trim().is_empty() {
            return Err(EventWireError::InvalidIdentity("stream_id"));
        }
        if self
            .max_events
            .as_ref()
            .is_some_and(|maximum| maximum.to_u64() == Some(0))
        {
            return Err(EventWireError::InvalidSequence);
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct ReplayResponse {
    /// Cursor supplied by the request. This makes an empty response and a
    /// retention gap independently verifiable.
    pub requested_after_sequence: WireU64,
    pub events: Vec<WireEventEnvelope>,
    pub next_cursor: EventCursor,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub gap: Option<EventGap>,
}

impl ReplayResponse {
    pub fn validate(&self) -> Result<(), EventWireError> {
        self.next_cursor.validate()?;
        let requested = self
            .requested_after_sequence
            .to_u64()
            .ok_or(EventWireError::InvalidSequence)?;
        let gap_watermark = match &self.gap {
            Some(gap) => Some(
                gap.snapshot_watermark
                    .to_u64()
                    .ok_or(EventWireError::InvalidSequence)?,
            ),
            None => None,
        };
        let mut previous: Option<u64> = None;
        for event in &self.events {
            event.validate()?;
            if event.stream_id != self.next_cursor.stream_id {
                return Err(EventWireError::InvalidIdentity("replay stream_id"));
            }
            let sequence = event
                .sequence
                .to_u64()
                .ok_or(EventWireError::InvalidSequence)?;
            if previous.is_none() {
                let base = gap_watermark.unwrap_or(requested);
                if base.checked_add(1) != Some(sequence) {
                    return Err(EventWireError::InvalidSequence);
                }
            }
            if let Some(previous) = previous
                && previous.checked_add(1) != Some(sequence)
            {
                return Err(EventWireError::InvalidSequence);
            }
            previous = Some(sequence);
        }
        let cursor = self
            .next_cursor
            .last_processed_sequence
            .to_u64()
            .ok_or(EventWireError::InvalidSequence)?;
        let expected_cursor = previous.or(gap_watermark).unwrap_or(requested);
        if cursor != expected_cursor {
            return Err(EventWireError::InvalidSequence);
        }
        if let Some(gap) = &self.gap {
            gap.validate()?;
            let from = gap
                .from_sequence
                .to_u64()
                .ok_or(EventWireError::InvalidSequence)?;
            if requested.checked_add(1) != Some(from) {
                return Err(EventWireError::InvalidSequence);
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct EventGap {
    pub from_sequence: WireNonZeroU64,
    pub to_sequence: WireNonZeroU64,
    #[schemars(length(min = 1, max = 1024))]
    pub reason: String,
    pub snapshot_watermark: WireNonZeroU64,
}

impl EventGap {
    pub fn validate(&self) -> Result<(), EventWireError> {
        let from = self
            .from_sequence
            .to_u64()
            .ok_or(EventWireError::InvalidSequence)?;
        let to = self
            .to_sequence
            .to_u64()
            .ok_or(EventWireError::InvalidSequence)?;
        let watermark = self
            .snapshot_watermark
            .to_u64()
            .ok_or(EventWireError::InvalidSequence)?;
        if from == 0 || to < from || watermark < to || self.reason.trim().is_empty() {
            Err(EventWireError::InvalidSequence)
        } else {
            Ok(())
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct GapNotification {
    #[schemars(length(min = 1, max = 256))]
    pub stream_id: String,
    pub gap: EventGap,
}

impl GapNotification {
    pub fn validate(&self) -> Result<(), EventWireError> {
        if self.stream_id.trim().is_empty() {
            return Err(EventWireError::InvalidIdentity("stream_id"));
        }
        self.gap.validate()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use echo_core::agent::EventIdentity;

    #[test]
    fn real_framework_event_round_trips() -> Result<(), Box<dyn std::error::Error>> {
        let identity = EventIdentity::new("stream-1", "turn-1")?;
        let framework =
            EventEnvelope::new(&identity, 1, None, AgentEvent::Token("你好".to_string()))?;
        let wire = WireEventEnvelope::try_from(framework.clone())?;
        assert_eq!(wire.payload.event_type, "token");
        let round_trip = EventEnvelope::<AgentEvent>::try_from(wire)?;
        assert_eq!(round_trip.event_id, framework.event_id);
        assert_eq!(round_trip.sequence, framework.sequence);
        assert_eq!(round_trip.content_hash, framework.content_hash);
        Ok(())
    }

    #[test]
    fn sequence_zero_is_invalid() {
        let parsed: Result<WireEventEnvelope, _> = serde_json::from_value(serde_json::json!({
            "schema_version": 4,
            "event_id": "event-1",
            "content_hash": format!("sha256:{}", "a".repeat(64)),
            "sequence": "0",
            "stream_id": "stream-1",
            "turn_id": "turn-1",
            "timestamp": {"unix_seconds": "0", "nanos": 0},
            "payload": {"event_type": "think_start"}
        }));
        assert!(parsed.is_err());
    }

    #[test]
    fn replay_rejects_non_contiguous_events() -> Result<(), Box<dyn std::error::Error>> {
        let identity = EventIdentity::new("stream-1", "turn-1")?;
        let first = WireEventEnvelope::try_from(EventEnvelope::new(
            &identity,
            1,
            None,
            AgentEvent::ThinkStart,
        )?)?;
        let third = WireEventEnvelope::try_from(EventEnvelope::new(
            &identity,
            3,
            None,
            AgentEvent::ThinkEnd {
                prompt_tokens: 1,
                completion_tokens: 1,
            },
        )?)?;
        let response = ReplayResponse {
            requested_after_sequence: WireU64::from_u64(0),
            events: vec![first, third],
            next_cursor: EventCursor {
                stream_id: "stream-1".to_string(),
                last_processed_sequence: WireU64::from_u64(3),
            },
            gap: None,
        };
        assert!(response.validate().is_err());
        Ok(())
    }

    #[test]
    fn replay_rejects_cursor_ahead_of_delivery() -> Result<(), Box<dyn std::error::Error>> {
        let identity = EventIdentity::new("stream-1", "turn-1")?;
        let event = WireEventEnvelope::try_from(EventEnvelope::new(
            &identity,
            1,
            None,
            AgentEvent::ThinkStart,
        )?)?;
        let response = ReplayResponse {
            requested_after_sequence: WireU64::from_u64(0),
            events: vec![event],
            next_cursor: EventCursor {
                stream_id: "stream-1".to_string(),
                last_processed_sequence: WireU64::from_u64(100),
            },
            gap: None,
        };
        assert!(response.validate().is_err());
        Ok(())
    }
}
