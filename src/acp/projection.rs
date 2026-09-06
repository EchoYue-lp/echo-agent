use crate::agent::{AgentEvent, EventEnvelope};
use crate::error::{ReactError, Result};
use crate::runtime::{EventSink, SinkControl};
use agent_client_protocol::schema::v1::{
    ContentBlock, ContentChunk, SessionId, SessionNotification, SessionUpdate, TextContent,
    ToolCall, ToolCallStatus, ToolCallUpdate, ToolCallUpdateFields,
};
use agent_client_protocol::{Client, ConnectionTo};
use async_trait::async_trait;
use std::sync::Mutex;

#[derive(Default)]
struct ProjectionState {
    thinking: bool,
    emitted_agent_text: bool,
    updates_sent: usize,
    total_update_chars: usize,
}

/// Standard ACP view of one run's committed events.
///
/// The projector never owns the events: the shared run ledger commits every
/// accepted [`EventEnvelope`] first, and this type only renders the bounded
/// `session/update` projection of the same facts (design §11.1 — the
/// projection may not introduce new sequences or terminals).
pub struct AcpEventProjector {
    session_id: SessionId,
    connection: ConnectionTo<Client>,
    max_update_chars: usize,
    max_updates_per_turn: usize,
    max_total_update_chars: usize,
    state: Mutex<ProjectionState>,
}

impl AcpEventProjector {
    pub fn new(
        session_id: SessionId,
        connection: ConnectionTo<Client>,
        max_update_chars: usize,
        max_updates_per_turn: usize,
        max_total_update_chars: usize,
    ) -> Self {
        Self {
            session_id,
            connection,
            max_update_chars,
            max_updates_per_turn,
            max_total_update_chars,
            state: Mutex::new(ProjectionState::default()),
        }
    }

    /// Render and send the standard updates for one committed envelope.
    /// A bounds violation or a failed send fails the whole run through the
    /// driver's exactly-one-terminal contract — the projection is part of
    /// the run's accepted output, not best-effort decoration.
    pub async fn emit(&self, envelope: &EventEnvelope) -> Result<()> {
        for notification in self.project(envelope)? {
            self.reserve(&notification)?;
            self.connection
                .send_notification(notification)
                .map_err(|error| {
                    ReactError::Other(format!("failed to send ACP session/update: {error}"))
                })?;
        }
        Ok(())
    }

    fn project(&self, envelope: &EventEnvelope) -> Result<Vec<SessionNotification>> {
        let updates = self.updates(&envelope.payload)?;
        Ok(updates
            .into_iter()
            .map(|update| SessionNotification::new(self.session_id.clone(), update))
            .collect())
    }

    fn updates(&self, payload: &AgentEvent) -> Result<Vec<SessionUpdate>> {
        let mut state = self
            .state
            .lock()
            .map_err(|_| ReactError::Other("ACP projector state lock poisoned".to_string()))?;
        let updates = match payload {
            AgentEvent::ThinkStart => {
                state.thinking = true;
                Vec::new()
            }
            AgentEvent::ThinkEnd { .. } => {
                state.thinking = false;
                Vec::new()
            }
            AgentEvent::Token(text) => {
                validate_update_size(text, self.max_update_chars)?;
                let chunk = ContentChunk::new(ContentBlock::Text(TextContent::new(text.clone())));
                if state.thinking {
                    vec![SessionUpdate::AgentThoughtChunk(chunk)]
                } else {
                    state.emitted_agent_text = true;
                    vec![SessionUpdate::AgentMessageChunk(chunk)]
                }
            }
            AgentEvent::FinalAnswer(answer) if !state.emitted_agent_text => {
                validate_update_size(answer, self.max_update_chars)?;
                state.emitted_agent_text = true;
                vec![SessionUpdate::AgentMessageChunk(ContentChunk::new(
                    ContentBlock::Text(TextContent::new(answer.clone())),
                ))]
            }
            AgentEvent::ToolCall {
                call_id,
                invocation,
            } => {
                validate_update_size(&invocation.name, self.max_update_chars)?;
                let raw_input = encode_bounded(&invocation.args, self.max_update_chars)?;
                vec![SessionUpdate::ToolCall(
                    ToolCall::new(call_id.clone(), invocation.name.clone())
                        .status(ToolCallStatus::Pending)
                        .raw_input(raw_input),
                )]
            }
            AgentEvent::ToolStream { call_id, event, .. } => {
                let output = encode_bounded(event, self.max_update_chars)?;
                vec![SessionUpdate::ToolCallUpdate(ToolCallUpdate::new(
                    call_id.clone(),
                    ToolCallUpdateFields::new()
                        .status(ToolCallStatus::InProgress)
                        .raw_output(output),
                ))]
            }
            AgentEvent::ToolResult {
                call_id, result, ..
            } => {
                let failed = !result.success;
                let output = encode_bounded(result, self.max_update_chars)?;
                vec![SessionUpdate::ToolCallUpdate(ToolCallUpdate::new(
                    call_id.clone(),
                    ToolCallUpdateFields::new()
                        .status(if failed {
                            ToolCallStatus::Failed
                        } else {
                            ToolCallStatus::Completed
                        })
                        .raw_output(output),
                ))]
            }
            _ => Vec::new(),
        };
        Ok(updates)
    }

    fn reserve(&self, notification: &SessionNotification) -> Result<()> {
        let serialized = serde_json::to_string(notification).map_err(|error| {
            ReactError::Other(format!("failed to encode ACP session/update: {error}"))
        })?;
        let update_chars = serialized.chars().count();
        if update_chars > self.max_update_chars {
            return Err(ReactError::Other(format!(
                "ACP update exceeds the configured {} character limit",
                self.max_update_chars
            )));
        }
        let mut state = self
            .state
            .lock()
            .map_err(|_| ReactError::Other("ACP projector state lock poisoned".to_string()))?;
        let updates_sent = state
            .updates_sent
            .checked_add(1)
            .ok_or_else(|| ReactError::Other("ACP update count overflow".to_string()))?;
        let total_update_chars = state
            .total_update_chars
            .checked_add(update_chars)
            .ok_or_else(|| ReactError::Other("ACP update size overflow".to_string()))?;
        if updates_sent > self.max_updates_per_turn
            || total_update_chars > self.max_total_update_chars
        {
            return Err(ReactError::Other(
                "ACP Turn exceeded its cumulative update budget".to_string(),
            ));
        }
        state.updates_sent = updates_sent;
        state.total_update_chars = total_update_chars;
        Ok(())
    }
}

#[async_trait]
impl EventSink for AcpEventProjector {
    async fn on_event(&self, envelope: EventEnvelope) -> Result<SinkControl> {
        self.emit(&envelope).await?;
        Ok(SinkControl::Continue)
    }
}

fn validate_update_size(text: &str, max_chars: usize) -> Result<()> {
    if text.chars().count() > max_chars {
        return Err(ReactError::Other(format!(
            "ACP update exceeds the configured {max_chars} character limit"
        )));
    }
    Ok(())
}

fn encode_bounded<T: serde::Serialize>(value: &T, max_chars: usize) -> Result<serde_json::Value> {
    let encoded = serde_json::to_string(value)
        .map_err(|error| ReactError::Other(format!("failed to encode ACP tool update: {error}")))?;
    validate_update_size(&encoded, max_chars)?;
    serde_json::from_str(&encoded).map_err(|error| {
        ReactError::Other(format!("failed to materialize ACP tool update: {error}"))
    })
}
