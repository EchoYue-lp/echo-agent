use crate::agent::{AgentEvent, EventEnvelope};
use crate::error::{ReactError, Result};
use crate::runtime::{EventSink, SinkControl};
use agent_client_protocol::schema::v1::{
    ContentBlock, ContentChunk, SessionId, SessionNotification, SessionUpdate, TextContent,
    ToolCall, ToolCallStatus, ToolCallUpdate, ToolCallUpdateFields,
};
use agent_client_protocol::{Client, ConnectionTo};
use async_trait::async_trait;
use echo_core::agent::{AgentOutputProjection, AssistantPhase, ExecutionStatusKind};
use std::sync::Mutex;

#[derive(Default)]
struct ProjectionState {
    output: AgentOutputProjection,
    projection_failed: bool,
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
    /// A bounds violation or a failed send is surfaced as delivery failure;
    /// it never becomes a second Agent execution terminal.
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
        self.state
            .lock()
            .map_err(|_| ReactError::Other("ACP projector state lock poisoned".to_string()))?
            .updates(payload, self.max_update_chars)
    }

    fn reserve(&self, notification: &SessionNotification) -> Result<()> {
        self.state
            .lock()
            .map_err(|_| ReactError::Other("ACP projector state lock poisoned".to_string()))?
            .reserve_notification(
                notification,
                self.max_update_chars,
                self.max_updates_per_turn,
                self.max_total_update_chars,
            )
    }
}

impl ProjectionState {
    fn updates(
        &mut self,
        payload: &AgentEvent,
        max_update_chars: usize,
    ) -> Result<Vec<SessionUpdate>> {
        if self.projection_failed {
            return Err(ReactError::Other(
                "ACP projection already failed".to_string(),
            ));
        }
        let result = self.project_payload(payload, max_update_chars);
        // 无效原生事实不能被后续看似成功的状态抹去；这只是投递失败，不是执行终态。
        self.projection_failed |= result.is_err();
        result
    }

    fn project_payload(
        &mut self,
        payload: &AgentEvent,
        max_update_chars: usize,
    ) -> Result<Vec<SessionUpdate>> {
        let updates = match payload {
            AgentEvent::AssistantMessage { block } => match self.output.observe_block(block)? {
                Some(delta) if !delta.text.is_empty() => {
                    validate_update_size(&delta.text, max_update_chars)?;
                    let chunk = ContentChunk::new(ContentBlock::Text(TextContent::new(delta.text)));
                    if delta.block.phase == AssistantPhase::Reasoning {
                        vec![SessionUpdate::AgentThoughtChunk(chunk)]
                    } else {
                        vec![SessionUpdate::AgentMessageChunk(chunk)]
                    }
                }
                _ => Vec::new(),
            },
            AgentEvent::FinalAnswer(answer) => {
                // 只确认 runtime 经 Final Guard 批准的正文，不以“已见进度”屏蔽最终答复。
                self.output.confirm_answer(answer)?;
                Vec::new()
            }
            AgentEvent::ExecutionStatus(status) => {
                let changed = self.output.observe_status(status)?;
                if changed && status.status == ExecutionStatusKind::Settled {
                    // 原生 Settled 只允许补显示；原 driver 仍须读物理 EOF 后才能产生 receipt。
                    // 这里绝不调用 finish，也不创造执行身份、序号或结算 owner。
                    match self.output.take_final_presentation()? {
                        Some(missing) => {
                            validate_update_size(&missing, max_update_chars)?;
                            vec![SessionUpdate::AgentMessageChunk(ContentChunk::new(
                                ContentBlock::Text(TextContent::new(missing)),
                            ))]
                        }
                        None => Vec::new(),
                    }
                } else {
                    Vec::new()
                }
            }
            AgentEvent::ToolCall {
                call_id,
                invocation,
            } => {
                validate_update_size(&invocation.name, max_update_chars)?;
                let raw_input = encode_bounded(&invocation.args, max_update_chars)?;
                vec![SessionUpdate::ToolCall(
                    ToolCall::new(call_id.clone(), invocation.name.clone())
                        .status(ToolCallStatus::Pending)
                        .raw_input(raw_input),
                )]
            }
            AgentEvent::ToolStream { call_id, event, .. } => {
                let output = encode_bounded(event, max_update_chars)?;
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
                let output = encode_bounded(result, max_update_chars)?;
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

    fn reserve_notification(
        &mut self,
        notification: &SessionNotification,
        max_update_chars: usize,
        max_updates_per_turn: usize,
        max_total_update_chars: usize,
    ) -> Result<()> {
        let serialized = serde_json::to_string(notification).map_err(|error| {
            ReactError::Other(format!("failed to encode ACP session/update: {error}"))
        })?;
        let update_chars = serialized.chars().count();
        validate_update_size(&serialized, max_update_chars)?;
        let updates_sent = self
            .updates_sent
            .checked_add(1)
            .ok_or_else(|| ReactError::Other("ACP update count overflow".to_string()))?;
        let total_update_chars = self
            .total_update_chars
            .checked_add(update_chars)
            .ok_or_else(|| ReactError::Other("ACP update size overflow".to_string()))?;
        if updates_sent > max_updates_per_turn || total_update_chars > max_total_update_chars {
            return Err(ReactError::Other(
                "ACP Turn exceeded its cumulative update budget".to_string(),
            ));
        }
        self.updates_sent = updates_sent;
        self.total_update_chars = total_update_chars;
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

#[cfg(test)]
#[path = "projection/native_tests.rs"]
mod native_tests;
