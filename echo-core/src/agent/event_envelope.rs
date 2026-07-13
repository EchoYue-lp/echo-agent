use super::AgentEvent;
use crate::error::Result;
use chrono::{DateTime, Utc};
use futures::stream::{BoxStream, StreamExt};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::HashMap;

/// Current schema version for the framework event transport contract.
pub const AGENT_EVENT_SCHEMA_VERSION: u16 = 1;

/// Stable invocation identity copied onto every emitted event.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct EventIdentity {
    pub conversation_id: Option<String>,
    /// Formal task run identity. Ordinary chat turns keep this as `None`.
    pub run_id: Option<String>,
    pub turn_id: String,
    pub execution_id: Option<String>,
    /// Parent event for a child invocation, such as a delegated subagent.
    pub parent_event_id: Option<String>,
}

/// Versioned transport contract around an [`AgentEvent`] payload.
#[derive(Debug, Serialize, Deserialize)]
pub struct EventEnvelope {
    pub schema_version: u16,
    pub event_id: String,
    /// Monotonic within one wrapped invocation/execution stream, starting at 1.
    pub sequence: u64,
    pub conversation_id: Option<String>,
    pub run_id: Option<String>,
    pub turn_id: String,
    pub execution_id: Option<String>,
    pub parent_event_id: Option<String>,
    pub timestamp: DateTime<Utc>,
    pub payload: AgentEvent,
}

impl EventEnvelope {
    /// Construct one event when a caller must report failure before a stream exists.
    pub fn new(
        identity: &EventIdentity,
        sequence: u64,
        parent_event_id: Option<String>,
        payload: AgentEvent,
    ) -> Self {
        let event_id = stable_event_id(identity, sequence);
        Self {
            schema_version: AGENT_EVENT_SCHEMA_VERSION,
            event_id,
            sequence,
            conversation_id: identity.conversation_id.clone(),
            run_id: identity.run_id.clone(),
            turn_id: identity.turn_id.clone(),
            execution_id: identity.execution_id.clone(),
            parent_event_id,
            timestamp: Utc::now(),
            payload,
        }
    }
}

fn stable_event_id(identity: &EventIdentity, sequence: u64) -> String {
    let mut hasher = Sha256::new();
    hasher.update(AGENT_EVENT_SCHEMA_VERSION.to_be_bytes());
    for part in [
        identity.conversation_id.as_deref(),
        identity.run_id.as_deref(),
        Some(identity.turn_id.as_str()),
        identity.execution_id.as_deref(),
    ] {
        match part {
            Some(value) => {
                hasher.update([1]);
                hasher.update(u64::try_from(value.len()).unwrap_or(u64::MAX).to_be_bytes());
                hasher.update(value.as_bytes());
            }
            None => hasher.update([0]),
        }
    }
    hasher.update(sequence.to_be_bytes());
    format!("evt_{:x}", hasher.finalize())
}

impl EventIdentity {
    /// Derive transport identity from one value-scoped agent invocation.
    pub fn from_invocation(invocation: &super::AgentInvocationContext) -> Self {
        let runtime = invocation.runtime.as_ref();
        let run_id = runtime.and_then(|value| value.run_id.clone());
        let execution_id = runtime.and_then(|value| value.execution_id.clone());
        let turn_id = runtime
            .and_then(|value| value.turn_id.clone())
            .or_else(|| execution_id.clone())
            .or_else(|| run_id.clone())
            .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
        Self {
            conversation_id: runtime.and_then(|value| value.conversation_id.clone()),
            run_id,
            turn_id,
            execution_id,
            parent_event_id: None,
        }
    }
}

/// Wrap a raw event stream with stable identity, ordering, correlation, and terminal semantics.
///
/// Tool result/progress events point to their corresponding `ToolCall` event.
/// Raw stream errors and streams that end without a terminal payload are
/// normalized into exactly one terminal `AgentEvent::Error` envelope.
pub fn envelope_event_stream<'a>(
    stream: BoxStream<'a, Result<AgentEvent>>,
    identity: EventIdentity,
) -> BoxStream<'a, Result<EventEnvelope>> {
    envelope_event_stream_after(stream, identity, 0)
}

/// Resume envelope sequencing after the last durably persisted sequence.
///
/// Combined with deterministic `event_id` generation, this lets persistence
/// adapters reject duplicate side-effect completion events idempotently.
pub fn envelope_event_stream_after<'a>(
    mut stream: BoxStream<'a, Result<AgentEvent>>,
    identity: EventIdentity,
    last_persisted_sequence: u64,
) -> BoxStream<'a, Result<EventEnvelope>> {
    let wrapped = async_stream::stream! {
        let mut sequence = last_persisted_sequence;
        let mut terminal_emitted = false;
        let mut tool_calls = HashMap::<String, String>::new();

        while let Some(item) = stream.next().await {
            let payload = match item {
                Ok(event) => event,
                Err(error) => AgentEvent::Error {
                    source: "agent_stream".to_string(),
                    message: error.to_string(),
                },
            };

            sequence = sequence.saturating_add(1);
            let tool_call_id = match &payload {
                AgentEvent::ToolCall { call_id, .. } => Some(call_id.clone()),
                _ => None,
            };
            let parent_event_id = match &payload {
                AgentEvent::ToolResult { call_id, .. }
                | AgentEvent::ToolError { call_id, .. }
                | AgentEvent::ToolStream { call_id, .. } => tool_calls.get(call_id).cloned(),
                _ => identity.parent_event_id.clone(),
            };
            let is_tool_terminal = matches!(
                &payload,
                AgentEvent::ToolResult { .. } | AgentEvent::ToolError { .. }
            );
            let is_terminal = payload.is_terminal();
            let envelope = EventEnvelope::new(&identity, sequence, parent_event_id, payload);

            if let Some(call_id) = tool_call_id {
                tool_calls.insert(call_id, envelope.event_id.clone());
            }
            if is_tool_terminal {
                match &envelope.payload {
                    AgentEvent::ToolResult { call_id, .. }
                    | AgentEvent::ToolError { call_id, .. } => {
                        tool_calls.remove(call_id);
                    }
                    _ => {}
                }
            }

            yield Ok(envelope);
            if is_terminal {
                terminal_emitted = true;
                break;
            }
        }

        if !terminal_emitted {
            sequence = sequence.saturating_add(1);
            yield Ok(EventEnvelope::new(
                &identity,
                sequence,
                identity.parent_event_id.clone(),
                AgentEvent::Error {
                    source: "agent_stream".to_string(),
                    message: "agent stream ended without a terminal event".to_string(),
                },
            ));
        }
    };
    Box::pin(wrapped)
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures::stream;

    fn identity() -> EventIdentity {
        EventIdentity {
            conversation_id: Some("conversation-1".to_string()),
            run_id: None,
            turn_id: "turn-1".to_string(),
            execution_id: Some("execution-1".to_string()),
            parent_event_id: None,
        }
    }

    #[tokio::test]
    async fn sequences_events_and_correlates_tool_lifecycle() -> Result<()> {
        let raw = stream::iter(vec![
            Ok(AgentEvent::ToolCall {
                call_id: "call-1".to_string(),
                name: "shell".to_string(),
                args: serde_json::json!({}),
            }),
            Ok(AgentEvent::ToolResult {
                call_id: "call-1".to_string(),
                name: "shell".to_string(),
                output: "done".to_string(),
            }),
            Ok(AgentEvent::FinalAnswer("ok".to_string())),
        ]);
        let events = envelope_event_stream(Box::pin(raw), identity())
            .collect::<Vec<_>>()
            .await;
        let events = events.into_iter().collect::<Result<Vec<_>>>()?;

        assert_eq!(events.len(), 3);
        assert_eq!(events.get(0).map(|event| event.sequence), Some(1));
        assert_eq!(events.get(1).map(|event| event.sequence), Some(2));
        assert_eq!(events.get(2).map(|event| event.sequence), Some(3));
        assert_eq!(
            events
                .get(1)
                .and_then(|event| event.parent_event_id.as_ref()),
            events.first().map(|event| &event.event_id)
        );
        assert!(
            events
                .last()
                .is_some_and(|event| event.payload.is_terminal())
        );
        assert!(events.iter().all(|event| event.run_id.is_none()));
        Ok(())
    }

    #[tokio::test]
    async fn normalizes_missing_and_duplicate_terminals() -> Result<()> {
        let missing = stream::iter(vec![Ok(AgentEvent::Token("partial".to_string()))]);
        let missing_events = envelope_event_stream(Box::pin(missing), identity())
            .collect::<Vec<_>>()
            .await
            .into_iter()
            .collect::<Result<Vec<_>>>()?;
        assert_eq!(missing_events.len(), 2);
        assert!(
            missing_events
                .last()
                .is_some_and(|event| matches!(event.payload, AgentEvent::Error { .. }))
        );

        let duplicate = stream::iter(vec![
            Ok(AgentEvent::FinalAnswer("first".to_string())),
            Ok(AgentEvent::Cancelled),
        ]);
        let duplicate_events = envelope_event_stream(Box::pin(duplicate), identity())
            .collect::<Vec<_>>()
            .await
            .into_iter()
            .collect::<Result<Vec<_>>>()?;
        assert_eq!(duplicate_events.len(), 1);
        assert!(matches!(
            duplicate_events.first().map(|event| &event.payload),
            Some(AgentEvent::FinalAnswer(answer)) if answer == "first"
        ));

        let failed = stream::iter(vec![Err(crate::error::ReactError::Other(
            "provider disconnected".to_string(),
        ))]);
        let failed_events = envelope_event_stream(Box::pin(failed), identity())
            .collect::<Vec<_>>()
            .await
            .into_iter()
            .collect::<Result<Vec<_>>>()?;
        assert_eq!(failed_events.len(), 1);
        assert!(matches!(
            failed_events.first().map(|event| &event.payload),
            Some(AgentEvent::Error { source, message })
                if source == "agent_stream" && message.contains("provider disconnected")
        ));
        Ok(())
    }

    #[tokio::test]
    async fn resumes_after_persisted_sequence_with_stable_ids() -> Result<()> {
        let first = EventEnvelope::new(
            &identity(),
            8,
            None,
            AgentEvent::FinalAnswer("done".to_string()),
        );
        let repeated = EventEnvelope::new(
            &identity(),
            8,
            None,
            AgentEvent::FinalAnswer("done".to_string()),
        );
        assert_eq!(first.event_id, repeated.event_id);

        let resumed = envelope_event_stream_after(
            Box::pin(stream::iter(vec![Ok(AgentEvent::FinalAnswer(
                "resumed".to_string(),
            ))])),
            identity(),
            8,
        )
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .collect::<Result<Vec<_>>>()?;
        assert_eq!(resumed.first().map(|event| event.sequence), Some(9));
        Ok(())
    }

    #[test]
    fn serializes_versioned_contract() -> Result<()> {
        let envelope = EventEnvelope::new(
            &identity(),
            1,
            None,
            AgentEvent::FinalAnswer("done".to_string()),
        );
        let value = serde_json::to_value(&envelope)
            .map_err(|error| crate::error::ReactError::Other(error.to_string()))?;
        assert_eq!(
            value.get("schema_version").and_then(|value| value.as_u64()),
            Some(u64::from(AGENT_EVENT_SCHEMA_VERSION))
        );
        assert_eq!(
            value
                .get("payload")
                .and_then(|payload| payload.get("type"))
                .and_then(|value| value.as_str()),
            Some("final_answer")
        );
        Ok(())
    }
}
