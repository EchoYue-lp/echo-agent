//! Runtime checkpoints for resumable Agent runs.
//!
//! [`RuntimeStateStore`] persists the ReAct checkpoint only. Task dependency and
//! lifecycle state is owned by `echo_orchestration::tasks::TaskRevisionService`
//! and `RuntimeTaskService`, so this module deliberately has no task graph API.
//!
//! # Quick Start
//!
//! ```rust,ignore
//! use echo_agent::state::{AgentCheckpoint, RuntimeStateStore};
//!
//! # async fn example(store: &dyn RuntimeStateStore) -> echo_agent::error::Result<()> {
//! let checkpoint = AgentCheckpoint::new("conv-123");
//! store.save_checkpoint(&checkpoint).await?;
//! # Ok(())
//! # }
//! ```

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Sequenced event journal and checkpoint-reducer primitives.
///
/// Unknown outcomes retain the original [`PreparedJournalBatch`](crate::state::journal::PreparedJournalBatch). After
/// reopening a file-backed authority, first perform a read-only lookup, then
/// pass the same prepared value to `apply_batch`: an existing identity returns
/// `AlreadyCommitted`, and the reducer folds only a not-yet-applied suffix.
///
/// ```
/// use echo_agent::state::journal::{
///     ApplyBatchReceipt, CheckpointStore, CheckpointedReducer, EventJournal, EventReducer,
///     JournalBatchLookup, MemoryCheckpointStore, MemoryEventJournal, PreparedJournalBatch,
/// };
/// use std::sync::Arc;
///
/// #[derive(Default, serde::Serialize, serde::Deserialize)]
/// struct Count(u64);
/// impl EventReducer for Count {
///     type Event = String;
///     fn apply(&mut self, _event: &String) {
///         self.0 = self.0.saturating_add(1);
///     }
/// }
///
/// fn resume_prepared<J, R>(
///     journal: &J,
///     reducer: &CheckpointedReducer<J, R>,
///     prepared: PreparedJournalBatch<R::Event>,
/// ) -> Result<ApplyBatchReceipt, String>
/// where
///     J: EventJournal<R::Event>,
///     R: EventReducer,
/// {
///     if let JournalBatchLookup::Conflict { error } = journal
///         .lookup_batch(&prepared)
///         .map_err(|error| error.to_string())?
///     {
///         return Err(error);
///     }
///     reducer.apply_batch(prepared).map_err(|error| error.to_string())
/// }
///
/// # fn main() -> Result<(), String> {
/// let journal = Arc::new(MemoryEventJournal::<String>::new());
/// let checkpoints: Arc<dyn CheckpointStore<Count>> = Arc::new(MemoryCheckpointStore::new());
/// let reducer = CheckpointedReducer::new(Arc::clone(&journal), checkpoints, 8);
/// let prepared = PreparedJournalBatch::new(vec!["one".to_string(), "two".to_string()])
///     .map_err(|error| error.to_string())?;
/// let receipt = resume_prepared(journal.as_ref(), &reducer, prepared)?;
/// assert_eq!(receipt.record_count, 2);
/// # Ok(())
/// # }
/// ```
pub mod journal {
    pub use echo_state::journal::*;
}

/// Typed durable delivery lifecycle primitives.
///
/// This is the stable framework facade for ordered message delivery. The
/// route and payload remain caller-owned types; the framework owns only
/// lifecycle identity, attempts, retention, and recovery.
pub mod delivery {
    pub use echo_state::delivery::*;
}

// ── AgentCheckpoint ────────────────────────────────────────────────────

/// A full checkpoint of agent runtime state, suitable for serialization
/// and later restoration (hydration).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentCheckpoint {
    /// Conversation / session identifier.
    pub conversation_id: String,
    /// Serialized message history.
    pub messages_json: String,
    /// Current plan text (optional).
    pub current_plan: Option<String>,
    /// Names of currently active skills.
    pub active_skills: Vec<String>,
    /// If the agent was blocked, the reason.
    pub blocked_reason: Option<String>,
    /// Session-bound working directory (worktree path). Restored on hydration
    /// so a worktree-bound session resumes in the same isolated checkout.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub working_dir: Option<std::path::PathBuf>,
    /// Timestamp when the checkpoint was captured.
    #[serde(with = "crate::utils::time::local_rfc3339")]
    pub timestamp: DateTime<Utc>,
}

/// Stable message identity persisted for one transcript projection generation.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TranscriptProjectionMessage {
    pub ordinal: u64,
    /// SHA-256 of the normalized user-visible message projection.
    pub digest: String,
}

/// Durable cursor for append-only projection of one model-context generation.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TranscriptProjectionCheckpoint {
    pub generation_id: String,
    pub next_ordinal: u64,
    pub projected: Vec<TranscriptProjectionMessage>,
}

/// Validated runtime payload restored from one `AgentCheckpoint` parse.
pub struct RestoredAgentCheckpoint {
    pub messages: Vec<crate::llm::types::Message>,
    pub transcript_projection: Option<TranscriptProjectionCheckpoint>,
}

#[derive(Serialize, Deserialize)]
struct AgentCheckpointPayload {
    messages: Vec<crate::llm::types::Message>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    transcript_projection: Option<TranscriptProjectionCheckpoint>,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum AgentCheckpointPayloadCompat {
    Current(AgentCheckpointPayload),
    Legacy(Vec<crate::llm::types::Message>),
}

impl AgentCheckpoint {
    /// Create a new checkpoint.
    pub fn new(conversation_id: impl Into<String>) -> Self {
        Self {
            conversation_id: conversation_id.into(),
            messages_json: String::new(),
            current_plan: None,
            active_skills: Vec::new(),
            blocked_reason: None,
            working_dir: None,
            timestamp: Utc::now(),
        }
    }

    /// Deserialize and validate the checkpoint message history.
    ///
    /// A checkpoint is resumable only when every assistant tool call has one
    /// matching tool result in order. Rejecting malformed history here avoids
    /// sending provider-invalid context or replaying an already completed
    /// side effect after restart.
    pub fn restore_messages(&self) -> crate::error::Result<Vec<crate::llm::types::Message>> {
        self.restore_runtime_payload()
            .map(|payload| payload.messages)
    }

    /// Restore the durable transcript generation cursor, when present.
    pub fn restore_transcript_projection(
        &self,
    ) -> crate::error::Result<Option<TranscriptProjectionCheckpoint>> {
        self.restore_runtime_payload()
            .map(|payload| payload.transcript_projection)
    }

    /// Parse and validate messages plus transcript cursor exactly once.
    pub fn restore_runtime_payload(&self) -> crate::error::Result<RestoredAgentCheckpoint> {
        let (messages, projection) = self.restore_payload()?;
        validate_tool_message_pairing(&messages)?;
        self.validate_transcript_projection(projection.as_ref())?;
        Ok(RestoredAgentCheckpoint {
            messages,
            transcript_projection: projection,
        })
    }

    fn validate_transcript_projection(
        &self,
        projection: Option<&TranscriptProjectionCheckpoint>,
    ) -> crate::error::Result<()> {
        if let Some(projection) = projection.as_ref() {
            if projection.generation_id != self.conversation_id {
                return Err(invalid_checkpoint(
                    "transcript projection generation does not match checkpoint identity"
                        .to_string(),
                ));
            }
            let mut previous = None;
            for message in &projection.projected {
                if message.ordinal >= projection.next_ordinal
                    || previous.is_some_and(|previous| message.ordinal <= previous)
                    || message.digest.len() != 64
                    || !message.digest.bytes().all(|byte| byte.is_ascii_hexdigit())
                {
                    return Err(invalid_checkpoint(
                        "transcript projection cursor is corrupt".to_string(),
                    ));
                }
                previous = Some(message.ordinal);
            }
        }
        Ok(())
    }

    /// Serialize messages and their exact transcript projection cursor into the
    /// existing checkpoint payload column/file.
    pub fn serialize_payload(
        messages: Vec<crate::llm::types::Message>,
        transcript_projection: Option<TranscriptProjectionCheckpoint>,
    ) -> crate::error::Result<String> {
        serde_json::to_string(&AgentCheckpointPayload {
            messages,
            transcript_projection,
        })
        .map_err(|error| {
            crate::error::ReactError::RuntimeState(Box::new(
                echo_core::error::RuntimeStateError::SerializationError(format!(
                    "Failed to serialize checkpoint payload: {error}"
                )),
            ))
        })
    }

    fn restore_payload(
        &self,
    ) -> crate::error::Result<(
        Vec<crate::llm::types::Message>,
        Option<TranscriptProjectionCheckpoint>,
    )> {
        let payload: AgentCheckpointPayloadCompat = serde_json::from_str(&self.messages_json)
            .map_err(|error| {
                crate::error::ReactError::RuntimeState(Box::new(
                    echo_core::error::RuntimeStateError::SerializationError(format!(
                        "Failed to deserialize checkpoint messages: {error}"
                    )),
                ))
            })?;
        Ok(match payload {
            AgentCheckpointPayloadCompat::Current(payload) => {
                (payload.messages, payload.transcript_projection)
            }
            AgentCheckpointPayloadCompat::Legacy(messages) => (messages, None),
        })
    }

    /// Completed tool call IDs present in this checkpoint, in message order.
    pub fn completed_tool_call_ids(&self) -> crate::error::Result<Vec<String>> {
        let messages = self.restore_messages()?;
        Ok(messages
            .into_iter()
            .filter_map(|message| {
                if message.role == crate::llm::types::Role::Tool {
                    message.tool_call_id
                } else {
                    None
                }
            })
            .collect())
    }
}

fn validate_tool_message_pairing(
    messages: &[crate::llm::types::Message],
) -> crate::error::Result<()> {
    let mut pending = std::collections::HashMap::<String, String>::new();
    for message in messages {
        if message.role == crate::llm::types::Role::Assistant
            && let Some(tool_calls) = message.tool_calls.as_ref()
        {
            for call in tool_calls {
                if pending
                    .insert(call.id.clone(), call.function.name.clone())
                    .is_some()
                {
                    return Err(invalid_checkpoint(format!(
                        "duplicate in-flight tool call id {}",
                        call.id
                    )));
                }
            }
        }
        if message.role == crate::llm::types::Role::Tool {
            let call_id = message.tool_call_id.as_deref().ok_or_else(|| {
                invalid_checkpoint("tool result is missing tool_call_id".to_string())
            })?;
            let expected_name = pending.remove(call_id).ok_or_else(|| {
                invalid_checkpoint(format!("orphan or duplicate tool result for {call_id}"))
            })?;
            if message.name.as_deref() != Some(expected_name.as_str()) {
                return Err(invalid_checkpoint(format!(
                    "tool result name mismatch for {call_id}: expected {expected_name}, got {}",
                    message.name.as_deref().unwrap_or("<missing>")
                )));
            }
        }
    }
    if pending.is_empty() {
        Ok(())
    } else {
        let mut ids = pending.into_keys().collect::<Vec<_>>();
        ids.sort();
        Err(invalid_checkpoint(format!(
            "checkpoint has tool calls without results: {}",
            ids.join(", ")
        )))
    }
}

fn invalid_checkpoint(message: String) -> crate::error::ReactError {
    crate::error::ReactError::RuntimeState(Box::new(
        echo_core::error::RuntimeStateError::SerializationError(message),
    ))
}

// ── RuntimeStateStore trait ────────────────────────────────────────────

/// Result of deleting one exact runtime-state incarnation from a stable scope.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeStateClearReceipt {
    pub scope_id: String,
    pub runtime_state_id: String,
    pub checkpoint_removed: bool,
}

/// Result of deleting every indexed runtime-state incarnation in one scope.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeStateScopeClearReceipt {
    pub scope_id: String,
    pub runtime_state_ids: Vec<String>,
}

/// Result of deleting stable transcript data and its runtime-state lineage.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PersistedConversationDeleteReceipt {
    pub conversation_id: String,
    pub runtime_state_ids: Vec<String>,
}

/// Trait for persistent runtime state storage.
///
/// Implementations may use SQLite, JSON files, or another durable backend.
pub trait RuntimeStateStore: Send + Sync {
    /// Get the most recent checkpoint for a conversation, if any.
    fn get_checkpoint<'a>(
        &'a self,
        conversation_id: &'a str,
    ) -> futures::future::BoxFuture<'a, crate::error::Result<Option<AgentCheckpoint>>>;

    /// Save a checkpoint.
    fn save_checkpoint<'a>(
        &'a self,
        checkpoint: &'a AgentCheckpoint,
    ) -> futures::future::BoxFuture<'a, crate::error::Result<()>>;

    /// Save a checkpoint and durably bind its globally unique runtime identity
    /// to one stable product/session scope.
    ///
    /// Implementations must commit the checkpoint and globally unique scope
    /// ownership as one recoverable authority. A secondary index may lag after
    /// a crash only when it can be rebuilt from that authority without guessing.
    fn save_checkpoint_for_scope<'a>(
        &'a self,
        scope_id: &'a str,
        checkpoint: &'a AgentCheckpoint,
    ) -> futures::future::BoxFuture<'a, crate::error::Result<()>>;

    /// List sorted runtime-state identities durably bound to `scope_id`.
    fn runtime_state_ids<'a>(
        &'a self,
        scope_id: &'a str,
    ) -> futures::future::BoxFuture<'a, crate::error::Result<Vec<String>>>;

    /// Delete one exact runtime-state incarnation and remove its scope binding.
    fn clear_runtime_state<'a>(
        &'a self,
        scope_id: &'a str,
        runtime_state_id: &'a str,
    ) -> futures::future::BoxFuture<'a, crate::error::Result<RuntimeStateClearReceipt>>;

    /// Delete every runtime-state incarnation indexed by `scope_id`.
    fn clear_runtime_state_scope<'a>(
        &'a self,
        scope_id: &'a str,
    ) -> futures::future::BoxFuture<'a, crate::error::Result<RuntimeStateScopeClearReceipt>>;

    /// Delete all state for a conversation.
    fn clear_conversation<'a>(
        &'a self,
        conversation_id: &'a str,
    ) -> futures::future::BoxFuture<'a, crate::error::Result<()>>;
}

/// Delete one retired runtime incarnation without deleting its stable product
/// transcript.
///
/// An incarnation-keyed transcript, if one was written by a non-invocation
/// operation, is deleted first. The exact runtime checkpoint and scope binding
/// are then cleared. Callers retain both IDs and can safely retry either step.
pub async fn clear_persisted_runtime_incarnation(
    conversation_store: &dyn crate::memory::ConversationStore,
    runtime_state_store: &dyn RuntimeStateStore,
    scope_id: &str,
    runtime_state_id: &str,
) -> crate::error::Result<RuntimeStateClearReceipt> {
    if runtime_state_id != scope_id {
        let indexed = runtime_state_store.runtime_state_ids(scope_id).await?;
        if !indexed
            .iter()
            .any(|indexed_id| indexed_id == runtime_state_id)
        {
            return Err(echo_core::error::RuntimeStateError::NotFound(format!(
                "runtime state {runtime_state_id} is not owned by scope {scope_id}"
            ))
            .into());
        }
        conversation_store
            .delete_conversation(runtime_state_id)
            .await?;
    }
    runtime_state_store
        .clear_runtime_state(scope_id, runtime_state_id)
        .await
}

/// Delete a stable user-visible transcript and every runtime checkpoint bound
/// to the same scope.
///
/// Incarnation-keyed transcripts are removed while the durable lineage still
/// exists, then runtime state is cleared, and the stable transcript is deleted
/// last. Each step is idempotent, so a crash can resume enumeration from the
/// retained scope index.
pub async fn delete_persisted_conversation(
    conversation_store: &dyn crate::memory::ConversationStore,
    runtime_state_store: &dyn RuntimeStateStore,
    conversation_id: &str,
) -> crate::error::Result<PersistedConversationDeleteReceipt> {
    let runtime_state_ids = runtime_state_store
        .runtime_state_ids(conversation_id)
        .await?;
    for runtime_state_id in &runtime_state_ids {
        if runtime_state_id != conversation_id {
            conversation_store
                .delete_conversation(runtime_state_id)
                .await?;
        }
    }
    let runtime = runtime_state_store
        .clear_runtime_state_scope(conversation_id)
        .await?;
    conversation_store
        .delete_conversation(conversation_id)
        .await?;
    Ok(PersistedConversationDeleteReceipt {
        conversation_id: conversation_id.to_string(),
        runtime_state_ids: runtime.runtime_state_ids,
    })
}

// ── Re-export implementations ─────────────────────────────────────────

/// File-backed runtime state store (default, no SQLite dependency).
pub mod file;

/// SQLite-backed runtime state store (`sqlite` feature).
#[cfg(feature = "sqlite")]
pub mod sqlite;

pub use file::FileRuntimeStateStore;
#[cfg(feature = "sqlite")]
pub use sqlite::SqliteRuntimeStateStore;

#[cfg(test)]
mod checkpoint_tests {
    use super::*;
    use crate::llm::types::{FunctionCall, Message, ToolCall};

    fn tool_call(id: &str, name: &str) -> ToolCall {
        ToolCall {
            id: id.to_string(),
            call_type: "function".to_string(),
            function: FunctionCall {
                name: name.to_string(),
                arguments: "{}".to_string(),
            },
        }
    }

    fn checkpoint(messages: Vec<Message>) -> crate::error::Result<AgentCheckpoint> {
        let mut checkpoint = AgentCheckpoint::new("conversation-1");
        checkpoint.messages_json = serde_json::to_string(&messages)
            .map_err(|error| crate::error::ReactError::Other(error.to_string()))?;
        Ok(checkpoint)
    }

    #[test]
    fn checkpoint_restores_paired_tool_history() -> crate::error::Result<()> {
        let checkpoint = checkpoint(vec![
            Message::assistant_with_tools(vec![tool_call("call-1", "write_file")]),
            Message::tool_result(
                "call-1".to_string(),
                "write_file".to_string(),
                "written".to_string(),
            ),
        ])?;
        assert_eq!(checkpoint.restore_messages()?.len(), 2);
        assert_eq!(checkpoint.completed_tool_call_ids()?, vec!["call-1"]);
        Ok(())
    }

    #[test]
    fn checkpoint_rejects_unpaired_or_duplicate_tool_results() -> crate::error::Result<()> {
        let unpaired = checkpoint(vec![Message::assistant_with_tools(vec![tool_call(
            "call-1",
            "write_file",
        )])])?;
        assert!(unpaired.restore_messages().is_err());

        let duplicate = checkpoint(vec![
            Message::assistant_with_tools(vec![tool_call("call-1", "write_file")]),
            Message::tool_result(
                "call-1".to_string(),
                "write_file".to_string(),
                "written".to_string(),
            ),
            Message::tool_result(
                "call-1".to_string(),
                "write_file".to_string(),
                "written twice".to_string(),
            ),
        ])?;
        assert!(duplicate.restore_messages().is_err());
        Ok(())
    }

    #[test]
    fn transcript_projection_cursor_round_trips_and_rejects_corruption() -> crate::error::Result<()>
    {
        let projection = TranscriptProjectionCheckpoint {
            generation_id: "conversation-1".to_string(),
            next_ordinal: 2,
            projected: vec![
                TranscriptProjectionMessage {
                    ordinal: 0,
                    digest: "a".repeat(64),
                },
                TranscriptProjectionMessage {
                    ordinal: 1,
                    digest: "b".repeat(64),
                },
            ],
        };
        let mut checkpoint = AgentCheckpoint::new("conversation-1");
        checkpoint.messages_json = AgentCheckpoint::serialize_payload(
            vec![Message::user("hello".to_string())],
            Some(projection.clone()),
        )?;
        assert_eq!(
            checkpoint.restore_transcript_projection()?,
            Some(projection)
        );

        let wrong_generation = TranscriptProjectionCheckpoint {
            generation_id: "other".to_string(),
            next_ordinal: 1,
            projected: vec![TranscriptProjectionMessage {
                ordinal: 0,
                digest: "c".repeat(64),
            }],
        };
        checkpoint.messages_json =
            AgentCheckpoint::serialize_payload(Vec::new(), Some(wrong_generation))?;
        assert!(checkpoint.restore_transcript_projection().is_err());

        let duplicate_ordinal = TranscriptProjectionCheckpoint {
            generation_id: "conversation-1".to_string(),
            next_ordinal: 2,
            projected: vec![
                TranscriptProjectionMessage {
                    ordinal: 1,
                    digest: "d".repeat(64),
                },
                TranscriptProjectionMessage {
                    ordinal: 1,
                    digest: "e".repeat(64),
                },
            ],
        };
        checkpoint.messages_json =
            AgentCheckpoint::serialize_payload(Vec::new(), Some(duplicate_ordinal))?;
        assert!(checkpoint.restore_transcript_projection().is_err());
        Ok(())
    }

    #[test]
    fn transcript_cursor_checkpoint_has_bounded_digest_overhead() -> crate::error::Result<()> {
        let projection = TranscriptProjectionCheckpoint {
            generation_id: "conversation-1".to_string(),
            next_ordinal: 100_000,
            projected: (0_u64..100_000)
                .map(|ordinal| TranscriptProjectionMessage {
                    ordinal,
                    digest: "f".repeat(64),
                })
                .collect(),
        };
        let started = std::time::Instant::now();
        let payload = AgentCheckpoint::serialize_payload(
            vec![Message::user("x".repeat(1_000_000))],
            Some(projection),
        )?;
        let serialize_elapsed = started.elapsed();
        let mut checkpoint = AgentCheckpoint::new("conversation-1");
        checkpoint.messages_json = payload;
        let restore_started = std::time::Instant::now();
        let restored = checkpoint.restore_runtime_payload()?;
        let restore_elapsed = restore_started.elapsed();
        if checkpoint.messages_json.len() > 13_000_000
            || serialize_elapsed > std::time::Duration::from_secs(5)
            || restore_elapsed > std::time::Duration::from_secs(5)
            || restored
                .transcript_projection
                .as_ref()
                .map(|projection| projection.projected.len())
                != Some(100_000)
        {
            return Err(crate::error::ReactError::Other(format!(
                "checkpoint cursor exceeded budget: bytes={}, serialize={serialize_elapsed:?}, restore={restore_elapsed:?}",
                checkpoint.messages_json.len(),
            )));
        }
        Ok(())
    }
}
