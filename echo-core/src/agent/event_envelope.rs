use super::AgentEvent;
#[cfg(test)]
use super::ToolInvocation;
use crate::error::Result;
use chrono::{DateTime, Utc};
use futures::stream::{BoxStream, StreamExt};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::fmt;

/// Current schema version for the framework event transport contract.
pub const AGENT_EVENT_SCHEMA_VERSION: u16 = 5;

macro_rules! identity_id {
    ($name:ident, $label:literal) => {
        #[doc = concat!("Validated, wire-transparent `", stringify!($name), "` value.")]
        ///
        /// Identity types are deliberately not interchangeable:
        ///
        /// ```compile_fail
        /// use echo_agent::agent::{RunId, TurnId};
        /// let run_id = RunId::new("run-1")?;
        /// let _turn_id: TurnId = run_id;
        /// # Ok::<(), echo_agent::Error>(())
        /// ```
        #[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
        #[serde(transparent)]
        pub struct $name(String);

        impl $name {
            pub fn new(value: impl Into<String>) -> Result<Self> {
                let value = value.into();
                if value.trim().is_empty() {
                    return Err(crate::error::ReactError::Other(format!(
                        "{} must not be empty",
                        $label
                    )));
                }
                Ok(Self(value))
            }

            pub fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str(&self.0)
            }
        }

        impl AsRef<str> for $name {
            fn as_ref(&self) -> &str {
                self.as_str()
            }
        }
    };
}

identity_id!(StreamId, "event stream_id");
identity_id!(ConversationId, "conversation_id");
identity_id!(RunId, "run_id");
identity_id!(TurnId, "turn_id");
identity_id!(MessageId, "message_id");
identity_id!(ExecutionId, "execution_id");
identity_id!(EventId, "event_id");

/// Stable invocation identity copied onto every emitted event.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EventIdentity {
    /// Unique identity of one concrete event stream invocation.
    pub stream_id: StreamId,
    pub conversation_id: Option<ConversationId>,
    /// Formal task run identity. Ordinary chat turns keep this as `None`.
    pub run_id: Option<RunId>,
    pub turn_id: TurnId,
    /// Message that triggered this invocation when message and turn identities differ.
    pub message_id: Option<MessageId>,
    pub execution_id: Option<ExecutionId>,
    /// Parent event for a child invocation, such as a delegated subagent.
    pub parent_event_id: Option<EventId>,
}

/// Versioned transport contract around an event payload.
///
/// `AgentEvent` remains the default payload so existing users can continue to
/// refer to `EventEnvelope` without a type argument. Other framework event
/// families reuse the same identity, ordering, hashing, and timestamp rules by
/// supplying their own serializable payload.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EventEnvelope<T = AgentEvent> {
    pub schema_version: u16,
    pub event_id: EventId,
    /// Integrity digest for the parent link and payload occupying this logical slot.
    pub content_hash: String,
    /// Monotonic within one wrapped invocation/execution stream, starting at 1.
    pub sequence: u64,
    pub stream_id: StreamId,
    pub conversation_id: Option<ConversationId>,
    pub run_id: Option<RunId>,
    pub turn_id: TurnId,
    pub message_id: Option<MessageId>,
    pub execution_id: Option<ExecutionId>,
    pub parent_event_id: Option<EventId>,
    pub timestamp: DateTime<Utc>,
    pub payload: T,
}

impl<T> EventEnvelope<T>
where
    T: Serialize,
{
    /// Construct one event when a caller must report failure before a stream exists.
    pub fn new(
        identity: &EventIdentity,
        sequence: u64,
        parent_event_id: Option<EventId>,
        payload: T,
    ) -> Result<Self> {
        identity.validate()?;
        if sequence == 0 {
            return Err(crate::error::ReactError::Other(
                "event sequence must start at one".to_string(),
            ));
        }
        let event_id = stable_event_id(identity, sequence);
        let content_hash = event_content_hash(parent_event_id.as_ref(), &payload)?;
        Ok(Self {
            schema_version: AGENT_EVENT_SCHEMA_VERSION,
            event_id,
            content_hash,
            sequence,
            stream_id: identity.stream_id.clone(),
            conversation_id: identity.conversation_id.clone(),
            run_id: identity.run_id.clone(),
            turn_id: identity.turn_id.clone(),
            message_id: identity.message_id.clone(),
            execution_id: identity.execution_id.clone(),
            parent_event_id,
            timestamp: Utc::now(),
            payload,
        })
    }
}

fn event_content_hash<T>(parent_event_id: Option<&EventId>, payload: &T) -> Result<String>
where
    T: Serialize,
{
    let mut hasher = Sha256::new();
    match parent_event_id {
        Some(value) => {
            hasher.update([1]);
            hasher.update(value.as_str().as_bytes());
        }
        None => hasher.update([0]),
    }
    let encoded = crate::utils::canonical_json::canonical_json_bytes(payload).map_err(|error| {
        crate::error::ReactError::Other(format!("failed to encode event payload: {error}"))
    })?;
    hasher.update(encoded);
    Ok(format!("sha256:{:x}", hasher.finalize()))
}

fn stable_event_id(identity: &EventIdentity, sequence: u64) -> EventId {
    let mut hasher = Sha256::new();
    hasher.update(AGENT_EVENT_SCHEMA_VERSION.to_be_bytes());
    hasher.update(identity.stream_id.as_str().as_bytes());
    for part in [
        identity
            .conversation_id
            .as_ref()
            .map(ConversationId::as_str),
        identity.run_id.as_ref().map(RunId::as_str),
        Some(identity.turn_id.as_str()),
        identity.message_id.as_ref().map(MessageId::as_str),
        identity.execution_id.as_ref().map(ExecutionId::as_str),
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
    EventId(format!("evt_{:x}", hasher.finalize()))
}

impl EventIdentity {
    pub fn new(stream_id: impl Into<String>, turn_id: impl Into<String>) -> Result<Self> {
        let identity = Self {
            stream_id: StreamId::new(stream_id)?,
            conversation_id: None,
            run_id: None,
            turn_id: TurnId::new(turn_id)?,
            message_id: None,
            execution_id: None,
            parent_event_id: None,
        };
        identity.validate()?;
        Ok(identity)
    }

    pub fn validate(&self) -> Result<()> {
        StreamId::new(self.stream_id.as_str())?;
        TurnId::new(self.turn_id.as_str())?;
        Ok(())
    }

    /// Construct identity for a formal run whose first turn and execution use
    /// the same externally assigned value.
    pub fn for_run(run_id: impl Into<String>) -> Result<Self> {
        let run_id = run_id.into();
        Ok(Self {
            stream_id: StreamId(uuid::Uuid::new_v4().to_string()),
            conversation_id: None,
            run_id: Some(RunId::new(run_id.clone())?),
            turn_id: TurnId::new(run_id.clone())?,
            message_id: None,
            execution_id: Some(ExecutionId::new(run_id)?),
            parent_event_id: None,
        })
    }

    /// Construct identity for an interactive chat turn while preserving the
    /// optional formal run and triggering message identities separately.
    pub fn for_chat(
        conversation_id: Option<String>,
        turn_id: impl Into<String>,
        message_id: impl Into<String>,
        run_id: Option<String>,
    ) -> Result<Self> {
        Ok(Self {
            stream_id: StreamId(uuid::Uuid::new_v4().to_string()),
            conversation_id: conversation_id.map(ConversationId::new).transpose()?,
            run_id: run_id.map(RunId::new).transpose()?,
            turn_id: TurnId::new(turn_id)?,
            message_id: Some(MessageId::new(message_id)?),
            execution_id: None,
            parent_event_id: None,
        })
    }

    pub fn with_execution_id(mut self, execution_id: impl Into<String>) -> Result<Self> {
        self.execution_id = Some(ExecutionId::new(execution_id)?);
        Ok(self)
    }

    pub fn with_conversation_id(mut self, conversation_id: impl Into<String>) -> Result<Self> {
        self.conversation_id = Some(ConversationId::new(conversation_id)?);
        Ok(self)
    }

    pub fn with_run_id(mut self, run_id: impl Into<String>) -> Result<Self> {
        self.run_id = Some(RunId::new(run_id)?);
        Ok(self)
    }

    pub fn with_message_id(mut self, message_id: impl Into<String>) -> Result<Self> {
        self.message_id = Some(MessageId::new(message_id)?);
        Ok(self)
    }

    pub fn with_parent_event_id(mut self, parent_event_id: impl Into<String>) -> Result<Self> {
        self.parent_event_id = Some(EventId::new(parent_event_id)?);
        Ok(self)
    }

    /// Derive transport identity from one value-scoped agent invocation.
    pub fn from_invocation(invocation: &super::AgentInvocationContext) -> Result<Self> {
        Self::from_runtime_context(invocation.runtime.as_ref())
    }

    /// Derive transport identity directly from an external runtime context.
    ///
    /// Dispatch coordinators use this before the child Agent invocation exists
    /// so start, inner-stream, and terminal events share one stream identity.
    pub fn from_runtime_context(
        runtime: Option<&crate::tools::ExternalRunContext>,
    ) -> Result<Self> {
        let run_id = runtime.and_then(|value| value.run_id.clone());
        let execution_id = runtime.and_then(|value| value.execution_id.clone());
        let turn_id = runtime
            .and_then(|value| value.turn_id.clone())
            .or_else(|| execution_id.clone())
            .or_else(|| run_id.clone())
            .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
        Ok(Self {
            stream_id: StreamId(uuid::Uuid::new_v4().to_string()),
            conversation_id: runtime
                .and_then(|value| value.conversation_id.clone())
                .map(ConversationId::new)
                .transpose()?,
            run_id: run_id.map(RunId::new).transpose()?,
            turn_id: TurnId::new(turn_id)?,
            message_id: runtime
                .and_then(|value| value.message_id.clone())
                .map(MessageId::new)
                .transpose()?,
            execution_id: execution_id.map(ExecutionId::new).transpose()?,
            parent_event_id: runtime
                .and_then(|value| value.subagent_lineage.as_ref())
                .and_then(|value| value.parent_event_id.clone())
                .map(EventId::new)
                .transpose()?,
        })
    }
}

/// 给原生 source 附加稳定身份、次序与工具父事件关联。
///
/// 原 runtime 状态和终态后的尾部始终运输到同一 source 的原 EOF；工具结果与进度
/// 关联其 ToolCall。原错误保留既有 typed Error 投影，缺少 native terminal 时发布
/// transport refusal Error。该拒绝不是原生执行终态，不能合成执行成功或物理结算。
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
        if let Err(error) = identity.validate() {
            yield Err(error);
            return;
        }
        let mut sequence = last_persisted_sequence;
        let mut terminal_emitted = false;
        let mut observation_only = false;
        // 当前source的真实状态是唯一执行终态；正文通知不选择另一种运输模式。
        let mut tool_calls = HashMap::<String, EventId>::new();

        while let Some(item) = stream.next().await {
            let payload = match item {
                Ok(event) => event,
                Err(error) => match AgentEvent::from_error("agent_stream", &error) {
                    Ok(terminal) => terminal,
                    Err(observation) => {
                        observation_only = true;
                        yield Err(observation);
                        continue;
                    }
                },
            };

            let Some(next_sequence) = sequence.checked_add(1) else {
                yield Err(crate::error::ReactError::Other(
                    "event sequence exhausted".to_string(),
                ));
                return;
            };
            sequence = next_sequence;
            let tool_call_id = match &payload {
                AgentEvent::ToolCall { call_id, .. } => Some(call_id.clone()),
                _ => None,
            };
            let parent_event_id = match &payload {
                AgentEvent::ToolResult { call_id, .. }
                | AgentEvent::ToolStream { call_id, .. } => tool_calls.get(call_id).cloned(),
                _ => identity.parent_event_id.clone(),
            };
            let is_terminal = payload.is_execution_terminal();
            let envelope = match EventEnvelope::new(&identity, sequence, parent_event_id, payload) {
                Ok(envelope) => envelope,
                Err(error) => {
                    yield Err(error);
                    return;
                }
            };

            if let Some(call_id) = tool_call_id {
                tool_calls.insert(call_id, envelope.event_id.clone());
            }
            if let AgentEvent::ToolResult { call_id, .. } = &envelope.payload {
                tool_calls.remove(call_id);
            }

            yield Ok(envelope);
            if is_terminal { terminal_emitted = true; }
            // 不提前drop：同一producer仍可能有真正尾错/资源结算。
        }

        if !terminal_emitted && !observation_only {
            let Some(next_sequence) = sequence.checked_add(1) else {
                yield Err(crate::error::ReactError::Other(
                    "event sequence exhausted before terminal event".to_string(),
                ));
                return;
            };
            sequence = next_sequence;
            yield EventEnvelope::new(
                &identity,
                sequence,
                identity.parent_event_id.clone(),
                AgentEvent::Error {
                    source: "agent_stream".to_string(),
                    message: "agent stream ended without a terminal event".to_string(),
                    failure: crate::error::AgentFailure::message(
                        "agent_stream",
                        "agent stream ended without a terminal event",
                    ),
                },
            );
        }
    };
    Box::pin(wrapped)
}

/// Validate payload-neutral envelope identity, ordering, hash, and parent-link
/// invariants without interpreting event-family-specific terminal semantics.
pub fn validate_envelope_trajectory<T>(events: &[EventEnvelope<T>]) -> Vec<String>
where
    T: Serialize,
{
    let mut violations = Vec::new();
    let Some(first) = events.first() else {
        return vec!["trajectory is empty".to_string()];
    };
    let mut seen_ids = std::collections::HashSet::<EventId>::new();
    let mut seen_event_ids = std::collections::HashSet::<EventId>::new();

    for (index, event) in events.iter().enumerate() {
        let expected_sequence = u64::try_from(index)
            .ok()
            .and_then(|offset| first.sequence.checked_add(offset));
        if event.schema_version != AGENT_EVENT_SCHEMA_VERSION {
            violations.push(format!(
                "unsupported schema version at sequence {}: {}",
                event.sequence, event.schema_version
            ));
        }
        if expected_sequence != Some(event.sequence) {
            violations.push(format!(
                "non-contiguous sequence: expected {}, got {}",
                expected_sequence
                    .map(|value| value.to_string())
                    .unwrap_or_else(|| "sequence overflow".to_string()),
                event.sequence
            ));
        }
        if event.stream_id != first.stream_id
            || event.conversation_id != first.conversation_id
            || event.run_id != first.run_id
            || event.turn_id != first.turn_id
            || event.message_id != first.message_id
            || event.execution_id != first.execution_id
        {
            violations.push(format!("identity changed at sequence {}", event.sequence));
        }
        if !seen_event_ids.insert(event.event_id.clone()) {
            violations.push(format!("duplicate event id: {}", event.event_id));
        }
        let identity = EventIdentity {
            stream_id: event.stream_id.clone(),
            conversation_id: event.conversation_id.clone(),
            run_id: event.run_id.clone(),
            turn_id: event.turn_id.clone(),
            message_id: event.message_id.clone(),
            execution_id: event.execution_id.clone(),
            parent_event_id: first.parent_event_id.clone(),
        };
        if event.event_id != stable_event_id(&identity, event.sequence) {
            violations.push(format!("invalid event id at sequence {}", event.sequence));
        }
        match event_content_hash(event.parent_event_id.as_ref(), &event.payload) {
            Ok(expected) if event.content_hash != expected => violations.push(format!(
                "invalid content hash at sequence {}",
                event.sequence
            )),
            Err(error) => violations.push(format!(
                "could not validate content hash at sequence {}: {error}",
                event.sequence
            )),
            Ok(_) => {}
        }
        if let Some(parent_id) = event.parent_event_id.as_ref()
            && first.parent_event_id.as_ref() != Some(parent_id)
            && !seen_ids.contains(parent_id)
        {
            violations.push(format!(
                "parent event {} was not emitted before sequence {}",
                parent_id, event.sequence
            ));
        }
        seen_ids.insert(event.event_id.clone());
    }

    violations
}

/// Validate a captured Agent event trajectory without re-running a model.
pub fn validate_event_trajectory(events: &[EventEnvelope]) -> Vec<String> {
    let mut violations = validate_envelope_trajectory(events);
    if events.is_empty() {
        return violations;
    }
    let mut tool_calls = HashMap::<String, EventId>::new();
    let mut terminal_count = 0_usize;

    for (index, event) in events.iter().enumerate() {
        match &event.payload {
            AgentEvent::ToolCall { call_id, .. }
                if tool_calls
                    .insert(call_id.clone(), event.event_id.clone())
                    .is_some() =>
            {
                violations.push(format!("duplicate in-flight tool call: {call_id}"));
            }
            AgentEvent::ToolCall { .. } => {}
            AgentEvent::ToolResult { call_id, .. } => match tool_calls.remove(call_id) {
                Some(parent_id) if event.parent_event_id.as_ref() == Some(&parent_id) => {}
                Some(parent_id) => violations.push(format!(
                    "tool completion {call_id} has wrong parent; expected {parent_id}"
                )),
                None => violations.push(format!("orphan tool completion: {call_id}")),
            },
            AgentEvent::ToolStream { call_id, .. } => {
                if let Some(parent_id) = tool_calls.get(call_id) {
                    if event.parent_event_id.as_ref() != Some(parent_id) {
                        violations.push(format!("tool stream {call_id} has wrong parent"));
                    }
                } else {
                    violations.push(format!("orphan tool stream: {call_id}"));
                }
            }
            _ => {}
        }
        if event.payload.is_execution_terminal() {
            terminal_count = terminal_count.saturating_add(1);
            if index.saturating_add(1) != events.len() {
                violations.push(format!(
                    "terminal event at sequence {} is not last",
                    event.sequence
                ));
            }
        }
    }

    if terminal_count != 1 {
        violations.push(format!(
            "trajectory must contain exactly one terminal event, got {terminal_count}"
        ));
    }
    let mut unfinished = tool_calls.into_keys().collect::<Vec<_>>();
    unfinished.sort();
    for call_id in unfinished {
        violations.push(format!("tool call without completion: {call_id}"));
    }
    violations
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures::stream;

    fn assert_same_payload(
        actual: Option<&AgentEvent>,
        expected: Option<&AgentEvent>,
    ) -> Result<()> {
        // AgentEvent 不要求整体 PartialEq；传输测试逐值比较完整 wire payload，
        // 再比较含 serde 跳过字段的完整状态，避免漏掉 adapter_generated。
        let actual_value = serde_json::to_value(actual)
            .map_err(|error| crate::error::ReactError::Other(error.to_string()))?;
        let expected_value = serde_json::to_value(expected)
            .map_err(|error| crate::error::ReactError::Other(error.to_string()))?;
        assert_eq!(actual_value, expected_value);
        if let (
            Some(AgentEvent::ExecutionStatus(actual)),
            Some(AgentEvent::ExecutionStatus(expected)),
        ) = (actual, expected)
        {
            assert_eq!(actual, expected);
        }
        Ok(())
    }

    #[derive(Debug, Serialize, Deserialize, PartialEq, Eq)]
    struct TestPayload {
        label: String,
    }

    fn identity() -> EventIdentity {
        EventIdentity {
            stream_id: StreamId("stream-1".to_string()),
            conversation_id: Some(ConversationId("conversation-1".to_string())),
            run_id: None,
            turn_id: TurnId("turn-1".to_string()),
            message_id: Some(MessageId("message-1".to_string())),
            execution_id: Some(ExecutionId("execution-1".to_string())),
            parent_event_id: None,
        }
    }

    fn native_status(status: super::super::ExecutionStatusKind, sequence: u64) -> AgentEvent {
        AgentEvent::ExecutionStatus(super::super::ExecutionStatus {
            adapter_generated: false,
            execution_id: "execution-1".to_string(),
            request_id: Some("response-1".to_string()),
            sequence,
            phase: super::super::AgentPhase::Terminal,
            status,
            reason: None,
        })
    }

    #[tokio::test]
    async fn native_message_envelope_delivers_runtime_terminal_after_final_answer() -> Result<()> {
        let raw = stream::iter(vec![
            Ok(native_status(super::super::ExecutionStatusKind::Started, 0)),
            Ok(AgentEvent::FinalAnswer("已结算的回答".to_string())),
            Ok(native_status(super::super::ExecutionStatusKind::Settled, 1)),
        ]);
        let events = envelope_event_stream(Box::pin(raw), identity())
            .collect::<Vec<_>>()
            .await
            .into_iter()
            .collect::<Result<Vec<_>>>()?;
        assert_eq!(events.len(), 3, "FinalAnswer 不能截掉 SDK/EKO 最后结算事实");
        assert!(matches!(events.last().map(|event| &event.payload),
            Some(AgentEvent::ExecutionStatus(status))
                if status.status == super::super::ExecutionStatusKind::Settled));
        assert!(validate_event_trajectory(&events).is_empty());
        Ok(())
    }

    #[tokio::test]
    async fn sequences_events_and_correlates_tool_lifecycle() -> Result<()> {
        let raw = stream::iter(vec![
            Ok(native_status(super::super::ExecutionStatusKind::Started, 0)),
            Ok(AgentEvent::ToolCall {
                call_id: "call-1".to_string(),
                invocation: ToolInvocation {
                    requested_name: "shell".to_string(),
                    requested_args: serde_json::json!({}),
                    name: "shell".to_string(),
                    args: serde_json::json!({}),
                    rewrites: Vec::new(),
                },
            }),
            Ok(AgentEvent::ToolResult {
                call_id: "call-1".to_string(),
                name: "shell".to_string(),
                result: crate::tools::ToolResult::success("done"),
            }),
            Ok(AgentEvent::FinalAnswer("ok".to_string())),
            Ok(native_status(super::super::ExecutionStatusKind::Settled, 1)),
        ]);
        let events = envelope_event_stream(Box::pin(raw), identity())
            .collect::<Vec<_>>()
            .await;
        let events = events.into_iter().collect::<Result<Vec<_>>>()?;

        assert_eq!(events.len(), 5);
        assert_eq!(
            events
                .iter()
                .map(|event| event.sequence)
                .collect::<Vec<_>>(),
            vec![1, 2, 3, 4, 5]
        );
        let kinds: Vec<_> = events
            .iter()
            .map(|event| match &event.payload {
                AgentEvent::ExecutionStatus(status)
                    if status.status == super::super::ExecutionStatusKind::Started =>
                {
                    "started"
                }
                AgentEvent::ToolCall { .. } => "tool_call",
                AgentEvent::ToolResult { .. } => "tool_result",
                AgentEvent::FinalAnswer(_) => "final_answer",
                AgentEvent::ExecutionStatus(status)
                    if status.status == super::super::ExecutionStatusKind::Settled =>
                {
                    "settled"
                }
                _ => "unexpected",
            })
            .collect();
        assert_eq!(
            kinds,
            vec![
                "started",
                "tool_call",
                "tool_result",
                "final_answer",
                "settled"
            ]
        );
        assert_eq!(
            events
                .get(2)
                .and_then(|event| event.parent_event_id.as_ref()),
            events.get(1).map(|event| &event.event_id)
        );
        assert!(
            events
                .last()
                .is_some_and(|event| event.payload.is_terminal())
        );
        assert!(events.iter().all(|event| event.run_id.is_none()));
        assert!(validate_event_trajectory(&events).is_empty());
        for event in &events {
            let rebuilt = EventEnvelope::new(
                &identity(),
                event.sequence,
                event.parent_event_id.clone(),
                event.payload.clone(),
            )?;
            assert_eq!(event.event_id, rebuilt.event_id);
            assert_eq!(event.content_hash, rebuilt.content_hash);
        }
        Ok(())
    }

    #[test]
    fn content_hash_is_independent_of_tool_metadata_insertion_order() -> Result<()> {
        let mut forward = crate::tools::ToolResult::success("done");
        forward
            .metadata
            .insert("alpha".to_string(), "1".to_string());
        forward
            .metadata
            .insert("omega".to_string(), "2".to_string());

        let mut reverse = crate::tools::ToolResult::success("done");
        reverse
            .metadata
            .insert("omega".to_string(), "2".to_string());
        reverse
            .metadata
            .insert("alpha".to_string(), "1".to_string());

        let forward = AgentEvent::ToolResult {
            call_id: "call-1".to_string(),
            name: "shell".to_string(),
            result: forward,
        };
        let reverse = AgentEvent::ToolResult {
            call_id: "call-1".to_string(),
            name: "shell".to_string(),
            result: reverse,
        };

        assert_eq!(
            event_content_hash(None, &forward)?,
            event_content_hash(None, &reverse)?
        );
        Ok(())
    }

    #[test]
    fn serialized_trajectory_recomputes_canonical_nested_metadata_hashes() -> Result<()> {
        let started = EventEnvelope::new(
            &identity(),
            1,
            None,
            native_status(super::super::ExecutionStatusKind::Started, 0),
        )?;
        let tool_call = EventEnvelope::new(
            &identity(),
            2,
            None,
            AgentEvent::ToolCall {
                call_id: "call-1".to_string(),
                invocation: ToolInvocation {
                    requested_name: "shell".to_string(),
                    requested_args: serde_json::json!({"command": "true"}),
                    name: "shell".to_string(),
                    args: serde_json::json!({"command": "true"}),
                    rewrites: Vec::new(),
                },
            },
        )?;
        let mut result = crate::tools::ToolResult::success("done");
        result.metadata.insert("omega".to_string(), "2".to_string());
        result.metadata.insert("alpha".to_string(), "1".to_string());
        let tool_result = EventEnvelope::new(
            &identity(),
            3,
            Some(tool_call.event_id.clone()),
            AgentEvent::ToolResult {
                call_id: "call-1".to_string(),
                name: "shell".to_string(),
                result,
            },
        )?;
        let expected_result_hash = tool_result.content_hash.clone();
        let confirmation = EventEnvelope::new(
            &identity(),
            4,
            None,
            AgentEvent::FinalAnswer("done".to_string()),
        )?;
        let terminal = EventEnvelope::new(
            &identity(),
            5,
            None,
            native_status(super::super::ExecutionStatusKind::Settled, 1),
        )?;
        let encoded = serde_json::to_vec(&vec![
            started,
            tool_call,
            tool_result,
            confirmation,
            terminal,
        ])
        .map_err(|error| crate::error::ReactError::Other(error.to_string()))?;
        let decoded: Vec<EventEnvelope> = serde_json::from_slice(&encoded)
            .map_err(|error| crate::error::ReactError::Other(error.to_string()))?;

        // 正文确认不授执行终态；同一序列化轨迹必须保留原 native Settled。
        let confirmation_only = decoded.iter().take(4).cloned().collect::<Vec<_>>();
        assert!(
            validate_event_trajectory(&confirmation_only)
                .iter()
                .any(|violation| violation.contains("exactly one terminal event, got 0"))
        );
        assert!(validate_event_trajectory(&decoded).is_empty());
        assert_eq!(
            decoded.get(2).map(|event| event.content_hash.as_str()),
            Some(expected_result_hash.as_str())
        );
        assert!(matches!(decoded.last().map(|event| &event.payload),
            Some(AgentEvent::ExecutionStatus(status))
                if !status.adapter_generated
                    && status.execution_id == "execution-1"
                    && status.status == super::super::ExecutionStatusKind::Settled));
        Ok(())
    }

    #[tokio::test]
    async fn native_transport_keeps_missing_refusal_and_original_cancellation_and_failure()
    -> Result<()> {
        use super::super::{AgentOutputProjection, ExecutionStatusKind};
        let missing = stream::iter(vec![
            Ok(native_status(ExecutionStatusKind::Started, 0)),
            Ok(AgentEvent::AssistantMessage {
                block: super::super::AssistantMessageBlock {
                    provider_source: None,
                    block_id: "partial".into(),
                    request_id: "response-1".into(),
                    sequence: 0,
                    phase: super::super::AssistantPhase::Commentary,
                    delta: "partial".into(),
                    complete: false,
                    provider: None,
                    reasoning: None,
                },
            }),
        ]);
        let missing_events = envelope_event_stream(Box::pin(missing), identity())
            .collect::<Vec<_>>()
            .await
            .into_iter()
            .collect::<Result<Vec<_>>>()?;
        assert_eq!(missing_events.len(), 3);
        assert!(validate_envelope_trajectory(&missing_events).is_empty());
        assert!(missing_events.last().is_some_and(|event| matches!(&event.payload,
            AgentEvent::Error { source, message, .. } if source == "agent_stream" && message == "agent stream ended without a terminal event")));
        assert!(
            missing_events
                .iter()
                .all(|event| !event.payload.is_execution_terminal())
        );
        let mut incomplete = AgentOutputProjection::default();
        for event in &missing_events {
            if let AgentEvent::ExecutionStatus(status) = &event.payload {
                incomplete.observe_status(status)?;
            }
            if let AgentEvent::AssistantMessage { block } = &event.payload {
                incomplete.observe_block(block)?;
            }
        }
        assert!(incomplete.finish().is_err());

        // 旧 FinalAnswer 吞掉后续取消的义务已退役；真实 native Cancelled 原样可见。
        let cancelled = stream::iter(vec![
            Ok(native_status(ExecutionStatusKind::Started, 0)),
            Ok(AgentEvent::FinalAnswer("first".to_string())),
            Ok(native_status(ExecutionStatusKind::Cancelled, 1)),
        ]);
        let cancelled_events = envelope_event_stream(Box::pin(cancelled), identity())
            .collect::<Vec<_>>()
            .await
            .into_iter()
            .collect::<Result<Vec<_>>>()?;
        assert_eq!(cancelled_events.len(), 3);
        assert!(validate_event_trajectory(&cancelled_events).is_empty());
        assert!(
            matches!(cancelled_events.get(1).map(|event| &event.payload), Some(AgentEvent::FinalAnswer(answer)) if answer == "first")
        );
        assert!(
            matches!(cancelled_events.last().map(|event| &event.payload),
            Some(AgentEvent::ExecutionStatus(status)) if !status.adapter_generated && status.execution_id == "execution-1" && status.status == ExecutionStatusKind::Cancelled)
        );

        let original = crate::error::ReactError::Other("provider disconnected".into());
        let expected_error = AgentEvent::from_error("agent_stream", &original)?;
        let mut failed = native_status(ExecutionStatusKind::Failed, 1);
        if let AgentEvent::ExecutionStatus(status) = &mut failed {
            status.reason = Some("provider disconnected".into());
        }
        let failed_events = envelope_event_stream(
            Box::pin(stream::iter(vec![
                Ok(native_status(ExecutionStatusKind::Started, 0)),
                Err(original),
                Ok(failed.clone()),
            ])),
            identity(),
        )
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .collect::<Result<Vec<_>>>()?;
        assert_eq!(failed_events.len(), 3);
        assert_same_payload(
            failed_events.get(1).map(|event| &event.payload),
            Some(&expected_error),
        )?;
        assert_same_payload(
            failed_events.last().map(|event| &event.payload),
            Some(&failed),
        )?;
        assert!(validate_event_trajectory(&failed_events).is_empty());
        Ok(())
    }

    #[tokio::test]
    async fn native_transport_preserves_duplicate_foreign_and_conflicting_status_for_one_projection()
    -> Result<()> {
        use super::super::{AgentOutputProjection, ExecutionStatusKind};
        for foreign in [true, false] {
            let started = native_status(ExecutionStatusKind::Started, 0);
            let settled = native_status(ExecutionStatusKind::Settled, 1);
            let mut rejected = settled.clone();
            if let AgentEvent::ExecutionStatus(status) = &mut rejected {
                if foreign {
                    status.execution_id = "foreign-execution".into();
                } else {
                    status.reason = Some("conflicting replay".into());
                }
            }
            let payloads = [
                started,
                AgentEvent::FinalAnswer("confirmed body".into()),
                settled.clone(),
                settled,
                rejected,
            ];
            let events = envelope_event_stream_after(
                Box::pin(stream::iter(payloads.iter().cloned().map(Ok))),
                identity(),
                41,
            )
            .collect::<Vec<_>>()
            .await
            .into_iter()
            .collect::<Result<Vec<_>>>()?;
            assert_eq!(events.len(), payloads.len());
            assert!(validate_envelope_trajectory(&events).is_empty());
            assert!(!validate_event_trajectory(&events).is_empty());
            let mut projection = AgentOutputProjection::default();
            for (index, event) in events.iter().enumerate() {
                assert_same_payload(Some(&event.payload), payloads.get(index))?;
                let sequence = u64::try_from(index)
                    .ok()
                    .and_then(|offset| offset.checked_add(42));
                assert_eq!(Some(event.sequence), sequence);
                let rebuilt =
                    EventEnvelope::new(&identity(), event.sequence, None, event.payload.clone())?;
                assert_eq!(event.event_id, rebuilt.event_id);
                assert_eq!(event.content_hash, rebuilt.content_hash);
                match &event.payload {
                    AgentEvent::ExecutionStatus(status) => {
                        let checked = projection.observe_status(status);
                        if index == 4 {
                            assert!(checked.is_err());
                        } else if index == 3 {
                            assert!(!checked?);
                        } else {
                            assert!(checked?);
                        }
                    }
                    AgentEvent::FinalAnswer(answer) => projection.confirm_answer(answer)?,
                    _ => {}
                }
            }
        }
        Ok(())
    }

    #[tokio::test]
    async fn resumes_after_persisted_sequence_with_stable_ids() -> Result<()> {
        let first = EventEnvelope::new(
            &identity(),
            8,
            None,
            AgentEvent::FinalAnswer("done".to_string()),
        )?;
        let repeated = EventEnvelope::new(
            &identity(),
            8,
            None,
            AgentEvent::FinalAnswer("done".to_string()),
        )?;
        assert_eq!(first.event_id, repeated.event_id);

        let resumed = envelope_event_stream_after(
            Box::pin(stream::iter(vec![
                Ok(native_status(super::super::ExecutionStatusKind::Started, 0)),
                Ok(AgentEvent::FinalAnswer("resumed".to_string())),
                Ok(native_status(super::super::ExecutionStatusKind::Settled, 1)),
            ])),
            identity(),
            8,
        )
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .collect::<Result<Vec<_>>>()?;
        assert_eq!(
            resumed
                .iter()
                .map(|event| event.sequence)
                .collect::<Vec<_>>(),
            vec![9, 10, 11]
        );
        assert!(validate_event_trajectory(&resumed).is_empty());
        let rebuilt = EventEnvelope::new(
            &identity(),
            9,
            None,
            native_status(super::super::ExecutionStatusKind::Started, 0),
        )?;
        assert_eq!(
            resumed.first().map(|event| &event.event_id),
            Some(&rebuilt.event_id)
        );
        assert_eq!(
            resumed.first().map(|event| &event.content_hash),
            Some(&rebuilt.content_hash)
        );
        Ok(())
    }

    #[test]
    fn independent_streams_with_same_business_identity_do_not_collide() -> Result<()> {
        let first_identity = identity();
        let mut second_identity = first_identity.clone();
        second_identity.stream_id = StreamId("stream-2".to_string());
        let first = EventEnvelope::new(
            &first_identity,
            1,
            None,
            AgentEvent::FinalAnswer("done".to_string()),
        )?;
        let second = EventEnvelope::new(
            &second_identity,
            1,
            None,
            AgentEvent::FinalAnswer("done".to_string()),
        )?;
        assert_ne!(first.event_id, second.event_id);
        Ok(())
    }

    #[test]
    fn serializes_versioned_contract() -> Result<()> {
        let envelope = EventEnvelope::new(
            &identity(),
            1,
            None,
            AgentEvent::FinalAnswer("done".to_string()),
        )?;
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

    #[tokio::test]
    async fn validates_canonical_success_cancel_error_and_hitl_budget_prefixes() -> Result<()> {
        use super::super::{AgentOutputProjection, ExecutionStatusKind};
        for (prefix, kind) in [
            (
                vec![
                    AgentEvent::BudgetDecision {
                        decision: super::super::BudgetDecision::FinalOnly,
                        reason: "token_budget".into(),
                        iteration: 2,
                        reported_model_tokens: 100,
                        usage_complete: true,
                    },
                    AgentEvent::FinalAnswer("完成 🧪".into()),
                ],
                ExecutionStatusKind::Settled,
            ),
            (
                vec![
                    AgentEvent::SafetyNotice {
                        action: "write file".into(),
                        reason: "requested".into(),
                        risk: "local mutation".into(),
                        permission: "confirm".into(),
                    },
                    AgentEvent::Cancelled,
                ],
                ExecutionStatusKind::Cancelled,
            ),
            (
                vec![AgentEvent::error_message("model", "stream interrupted")],
                ExecutionStatusKind::Failed,
            ),
        ] {
            let mut payloads = vec![native_status(ExecutionStatusKind::Started, 0)];
            payloads.extend(prefix);
            let mut terminal = native_status(kind.clone(), 1);
            if let AgentEvent::ExecutionStatus(status) = &mut terminal
                && kind == ExecutionStatusKind::Failed
            {
                status.reason = Some("stream interrupted".into());
            }
            payloads.push(terminal);
            let events = envelope_event_stream(
                Box::pin(stream::iter(payloads.iter().cloned().map(Ok))),
                identity(),
            )
            .collect::<Vec<_>>()
            .await
            .into_iter()
            .collect::<Result<Vec<_>>>()?;
            assert_eq!(events.len(), payloads.len());
            assert!(validate_event_trajectory(&events).is_empty());
            let mut projection = AgentOutputProjection::default();
            for (index, event) in events.iter().enumerate() {
                assert_same_payload(Some(&event.payload), payloads.get(index))?;
                assert_eq!(
                    Some(event.sequence),
                    u64::try_from(index)
                        .ok()
                        .and_then(|offset| offset.checked_add(1))
                );
                let rebuilt =
                    EventEnvelope::new(&identity(), event.sequence, None, event.payload.clone())?;
                assert_eq!(event.event_id, rebuilt.event_id);
                assert_eq!(event.content_hash, rebuilt.content_hash);
                match &event.payload {
                    AgentEvent::ExecutionStatus(status) => {
                        projection.observe_status(status)?;
                    }
                    AgentEvent::FinalAnswer(answer) => projection.confirm_answer(answer)?,
                    _ => {}
                }
            }
            match kind {
                ExecutionStatusKind::Settled => assert_eq!(projection.finish()?, "完成 🧪"),
                ExecutionStatusKind::Cancelled => assert!(
                    matches!(projection.finish(), Err(crate::error::ReactError::Agent(error)) if matches!(error.as_ref(), crate::error::AgentError::Cancelled(_)))
                ),
                _ => assert!(
                    matches!(projection.finish(), Err(crate::error::ReactError::Other(message)) if message == "stream interrupted")
                ),
            }
        }
        Ok(())
    }

    #[test]
    fn trajectory_validator_reports_sequence_identity_parent_and_terminal_drift() -> Result<()> {
        let mut first = EventEnvelope::new(
            &identity(),
            1,
            Some(EventId("missing-parent".to_string())),
            native_status(super::super::ExecutionStatusKind::Settled, 1),
        )?;
        first.schema_version = AGENT_EVENT_SCHEMA_VERSION.saturating_add(1);
        let mut second = EventEnvelope::new(
            &identity(),
            3,
            None,
            native_status(super::super::ExecutionStatusKind::Cancelled, 2),
        )?;
        second.turn_id = TurnId("different-turn".to_string());
        second.event_id = first.event_id.clone();
        let violations = validate_event_trajectory(&[first, second]);
        for expected in [
            "schema version",
            "not last",
            "non-contiguous",
            "identity changed",
            "duplicate event id",
            "exactly one terminal",
        ] {
            assert!(violations.iter().any(|value| value.contains(expected)));
        }
        Ok(())
    }

    #[test]
    fn rejects_blank_identity_and_zero_sequence() {
        let mut blank = identity();
        blank.turn_id = TurnId(String::new());
        assert!(EventEnvelope::new(&blank, 1, None, AgentEvent::Cancelled).is_err());
        assert!(EventEnvelope::new(&identity(), 0, None, AgentEvent::Cancelled).is_err());
    }

    #[test]
    fn generic_envelope_reuses_identity_hash_and_sequence_validation() -> Result<()> {
        let first = EventEnvelope::new(
            &identity(),
            4,
            None,
            TestPayload {
                label: "开始".to_string(),
            },
        )?;
        let second = EventEnvelope::new(
            &identity(),
            5,
            Some(first.event_id.clone()),
            TestPayload {
                label: "完成".to_string(),
            },
        )?;
        let encoded = serde_json::to_vec(&vec![first, second])
            .map_err(|error| crate::error::ReactError::Other(error.to_string()))?;
        let decoded: Vec<EventEnvelope<TestPayload>> = serde_json::from_slice(&encoded)
            .map_err(|error| crate::error::ReactError::Other(error.to_string()))?;

        assert!(validate_envelope_trajectory(&decoded).is_empty());
        assert_eq!(decoded.get(1).map(|event| event.sequence), Some(5));
        Ok(())
    }

    #[test]
    fn runtime_identity_preserves_nested_parent_event() -> Result<()> {
        let runtime = crate::tools::ExternalRunContext {
            turn_id: Some("child-turn".to_string()),
            execution_id: Some("child-execution".to_string()),
            subagent_lineage: Some(crate::tools::SubagentLineage {
                parent_event_id: Some("evt_parent_tool".to_string()),
                ..crate::tools::SubagentLineage::default()
            }),
            ..crate::tools::ExternalRunContext::default()
        };
        let identity = EventIdentity::from_runtime_context(Some(&runtime))?;
        assert_eq!(
            identity.parent_event_id.as_ref().map(EventId::as_str),
            Some("evt_parent_tool")
        );
        Ok(())
    }

    #[test]
    fn trajectory_rejects_tampered_slot_and_content() -> Result<()> {
        let mut slot = EventEnvelope::new(
            &identity(),
            1,
            None,
            AgentEvent::FinalAnswer("original".to_string()),
        )?;
        slot.event_id = EventId("evt_tampered".to_string());
        slot.content_hash = "sha256:tampered".to_string();
        let violations = validate_event_trajectory(&[slot]);
        assert!(
            violations
                .iter()
                .any(|value| value.contains("invalid event id"))
        );
        assert!(
            violations
                .iter()
                .any(|value| value.contains("invalid content hash"))
        );
        Ok(())
    }

    #[tokio::test]
    async fn rejects_exhausted_resume_sequence() {
        let events = envelope_event_stream_after(
            Box::pin(stream::iter(vec![Ok(native_status(
                super::super::ExecutionStatusKind::Started,
                0,
            ))])),
            identity(),
            u64::MAX,
        )
        .collect::<Vec<_>>()
        .await;
        assert_eq!(events.len(), 1);
        assert!(events.first().is_some_and(Result::is_err));
    }
}

#[cfg(test)]
mod canonical_transport_contract_tests {
    use super::*;
    use crate::agent::{AgentPhase, ExecutionStatus, ExecutionStatusKind};
    fn native_status(kind: ExecutionStatusKind, sequence: u64) -> AgentEvent {
        AgentEvent::ExecutionStatus(ExecutionStatus {
            adapter_generated: false,
            execution_id: "original-execution".into(),
            request_id: Some("request".into()),
            sequence,
            phase: AgentPhase::Terminal,
            status: kind,
            reason: None,
        })
    }
    #[tokio::test]
    async fn canonical_transport_does_not_end_at_confirmed_body_or_rewrite_original_status()
    -> Result<()> {
        let identity = EventIdentity::for_run("run")?.with_execution_id("original-execution")?;
        let input = futures::stream::iter(vec![
            Ok(native_status(ExecutionStatusKind::Started, 0)),
            Ok(AgentEvent::FinalAnswer("中文🙂".into())),
            Ok(native_status(ExecutionStatusKind::Settled, 8)),
        ])
        .boxed();
        let events = envelope_event_stream(input, identity)
            .collect::<Vec<_>>()
            .await
            .into_iter()
            .collect::<Result<Vec<_>>>()?;
        assert_eq!(events.len(), 3);
        assert!(
            matches!(events.last().map(|event|&event.payload),Some(AgentEvent::ExecutionStatus(status)) if status.sequence==8&&status.execution_id=="original-execution"&&!status.adapter_generated)
        );
        Ok(())
    }
    #[tokio::test]
    async fn canonical_transport_final_answer_without_source_terminal_is_not_a_successful_trajectory()
    -> Result<()> {
        let identity = EventIdentity::for_run("run")?;
        let input =
            futures::stream::iter(vec![Ok(AgentEvent::FinalAnswer("body only".into()))]).boxed();
        let events = envelope_event_stream(input, identity)
            .collect::<Vec<_>>()
            .await
            .into_iter()
            .collect::<Result<Vec<_>>>()?;
        assert!(events.iter().all(|event|!matches!(&event.payload,AgentEvent::ExecutionStatus(status) if status.status==ExecutionStatusKind::Settled)));
        assert!(
            events
                .iter()
                .any(|event| matches!(event.payload, AgentEvent::Error { .. })),
            "plain body/EOF requires a transport refusal; never Completed"
        );
        Ok(())
    }
}
