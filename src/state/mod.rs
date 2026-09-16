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
/// Checkpoints include a [`JournalIdentity`](crate::state::journal::JournalIdentity);
/// recovery accepts state only from the exact Journal generation that produced it.
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

/// Last known retry classification for one durable transcript effect.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TranscriptProjectionAttemptClass {
    OutcomeUnknown,
    TransientNoCommit,
    RevisionConflict,
}

/// Durable intent written before a transcript backend can observe the effect.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PendingTranscriptProjection {
    pub batch: crate::memory::TranscriptProjectionBatch,
    pub cursor_before: TranscriptProjectionCheckpoint,
    pub cursor_after: TranscriptProjectionCheckpoint,
    pub base_runtime_revision: u64,
    #[serde(with = "crate::utils::time::local_rfc3339")]
    pub prepared_at: DateTime<Utc>,
    pub attempt: u32,
    pub last_attempt_class: Option<TranscriptProjectionAttemptClass>,
    pub last_error: Option<String>,
}

/// Parsed checkpoint payload including any unsettled transcript intent.
pub struct ManagedAgentCheckpointPayload {
    pub messages: Vec<crate::llm::types::Message>,
    pub transcript_projection: Option<TranscriptProjectionCheckpoint>,
    pub pending_transcript_projection: Option<PendingTranscriptProjection>,
}

/// Validated runtime payload restored from one `AgentCheckpoint` parse.
pub struct RestoredAgentCheckpoint {
    pub messages: Vec<crate::llm::types::Message>,
    pub transcript_projection: Option<TranscriptProjectionCheckpoint>,
}

#[derive(Serialize, Deserialize)]
struct AgentCheckpointPayload {
    #[serde(default = "default_agent_checkpoint_payload_version")]
    schema_version: u16,
    messages: Vec<crate::llm::types::Message>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    transcript_projection: Option<TranscriptProjectionCheckpoint>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pending_transcript_projection: Option<PendingTranscriptProjection>,
}

fn default_agent_checkpoint_payload_version() -> u16 {
    1
}

#[derive(Deserialize)]
#[serde(untagged)]
enum AgentCheckpointPayloadCompat {
    Current(Box<AgentCheckpointPayload>),
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
        let payload = self.restore_managed_runtime_payload()?;
        if payload.pending_transcript_projection.is_some() {
            return Err(invalid_checkpoint(
                "checkpoint has unsettled transcript projection; managed recovery is required"
                    .to_string(),
            ));
        }
        Ok(RestoredAgentCheckpoint {
            messages: payload.messages,
            transcript_projection: payload.transcript_projection,
        })
    }

    /// Parse and validate runtime messages, cursor, and durable pending effect.
    pub fn restore_managed_runtime_payload(
        &self,
    ) -> crate::error::Result<ManagedAgentCheckpointPayload> {
        let payload = self.restore_payload()?;
        validate_tool_message_pairing(&payload.messages)?;
        self.validate_transcript_projection(payload.transcript_projection.as_ref())?;
        self.validate_pending_transcript_projection(
            payload.pending_transcript_projection.as_ref(),
        )?;
        Ok(payload)
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

    fn validate_pending_transcript_projection(
        &self,
        pending: Option<&PendingTranscriptProjection>,
    ) -> crate::error::Result<()> {
        let Some(pending) = pending else {
            return Ok(());
        };
        pending.batch.validate()?;
        if pending.batch.generation_id != self.conversation_id
            || pending.cursor_before.generation_id != self.conversation_id
            || pending.cursor_after.generation_id != self.conversation_id
            || pending.base_runtime_revision == 0
            || pending.attempt == 0
            || pending.cursor_before.next_ordinal
                != pending
                    .batch
                    .items
                    .first()
                    .map(|item| item.ordinal)
                    .unwrap_or(pending.cursor_before.next_ordinal)
            || pending.cursor_after.next_ordinal
                != pending
                    .batch
                    .items
                    .last()
                    .and_then(|item| item.ordinal.checked_add(1))
                    .unwrap_or(pending.cursor_after.next_ordinal)
        {
            return Err(invalid_checkpoint(
                "pending transcript projection is not aligned with checkpoint identity and cursor"
                    .to_string(),
            ));
        }
        self.validate_transcript_projection(Some(&pending.cursor_before))?;
        self.validate_transcript_projection(Some(&pending.cursor_after))?;
        Ok(())
    }

    /// Serialize messages and their exact transcript projection cursor into the
    /// existing checkpoint payload column/file.
    pub fn serialize_payload(
        messages: Vec<crate::llm::types::Message>,
        transcript_projection: Option<TranscriptProjectionCheckpoint>,
    ) -> crate::error::Result<String> {
        Self::serialize_managed_payload(messages, transcript_projection, None)
    }

    /// Serialize messages, cursor, and one durable pending transcript effect.
    pub fn serialize_managed_payload(
        messages: Vec<crate::llm::types::Message>,
        transcript_projection: Option<TranscriptProjectionCheckpoint>,
        pending_transcript_projection: Option<PendingTranscriptProjection>,
    ) -> crate::error::Result<String> {
        serde_json::to_string(&AgentCheckpointPayload {
            schema_version: 2,
            messages,
            transcript_projection,
            pending_transcript_projection,
        })
        .map_err(|error| {
            crate::error::ReactError::RuntimeState(Box::new(
                echo_core::error::RuntimeStateError::SerializationError(format!(
                    "Failed to serialize checkpoint payload: {error}"
                )),
            ))
        })
    }

    fn restore_payload(&self) -> crate::error::Result<ManagedAgentCheckpointPayload> {
        let payload: AgentCheckpointPayloadCompat = serde_json::from_str(&self.messages_json)
            .map_err(|error| {
                crate::error::ReactError::RuntimeState(Box::new(
                    echo_core::error::RuntimeStateError::SerializationError(format!(
                        "Failed to deserialize checkpoint messages: {error}"
                    )),
                ))
            })?;
        match payload {
            AgentCheckpointPayloadCompat::Current(payload) => {
                if !(1..=2).contains(&payload.schema_version) {
                    return Err(invalid_checkpoint(format!(
                        "unsupported checkpoint payload version {}",
                        payload.schema_version
                    )));
                }
                if payload.schema_version == 1 && payload.pending_transcript_projection.is_some() {
                    return Err(invalid_checkpoint(
                        "legacy checkpoint payload cannot contain pending transcript projection"
                            .to_string(),
                    ));
                }
                Ok(ManagedAgentCheckpointPayload {
                    messages: payload.messages,
                    transcript_projection: payload.transcript_projection,
                    pending_transcript_projection: payload.pending_transcript_projection,
                })
            }
            AgentCheckpointPayloadCompat::Legacy(messages) => Ok(ManagedAgentCheckpointPayload {
                messages,
                transcript_projection: None,
                pending_transcript_projection: None,
            }),
        }
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

/// Revisioned runtime-state support advertised without performing I/O.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RuntimeStateCapability {
    Unsupported,
    RevisionedV1,
}

/// Durable version of one runtime generation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RuntimeStateVersion {
    Absent,
    Unmanaged { digest: String },
    Managed { revision: u64 },
    Retired { revision: u64, operation_id: String },
}

/// Version a compare-and-save request expects to replace.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RuntimeStateExpectedVersion {
    Absent,
    Unmanaged { digest: String },
    Managed { revision: u64 },
}

/// Durable lifecycle of one stable runtime scope.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RuntimeScopeLifecycle {
    Active,
    Retiring,
    Tombstoned,
}

/// Stable runtime-scope authority consulted by admission and checkpoint CAS.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuntimeScopeAuthority {
    pub scope_id: String,
    pub conversation_epoch: Option<u64>,
    pub revision: u64,
    pub lifecycle: RuntimeScopeLifecycle,
}

/// Revisioned view of one runtime checkpoint or retirement tombstone.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ManagedRuntimeStateSnapshot {
    pub scope: RuntimeScopeAuthority,
    pub runtime_state_id: String,
    pub version: RuntimeStateVersion,
    pub checkpoint: Option<AgentCheckpoint>,
}

/// Atomic checkpoint write request.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RuntimeCheckpointCasRequest {
    pub scope_id: String,
    pub runtime_state_id: String,
    pub conversation_epoch: Option<u64>,
    pub expected_scope_revision: u64,
    pub expected_state_version: RuntimeStateExpectedVersion,
    pub checkpoint: AgentCheckpoint,
}

/// Domain result of compare-and-save.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RuntimeCheckpointCasStatus {
    Applied,
    AlreadyCurrent,
    RevisionConflict,
    GenerationRetired,
    ScopeFenced,
}

/// Stable receipt for compare-and-save.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuntimeCheckpointCasReceipt {
    pub scope: RuntimeScopeAuthority,
    pub runtime_state_id: String,
    pub version: RuntimeStateVersion,
    pub status: RuntimeCheckpointCasStatus,
}

/// Idempotent request to retire one exact runtime generation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RuntimeGenerationRetireRequest {
    pub operation_id: String,
    pub scope_id: String,
    pub runtime_state_id: String,
    pub expected_scope_revision: u64,
    pub expected_state_version: RuntimeStateExpectedVersion,
}

/// Domain result of exact generation retirement.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RuntimeGenerationRetireStatus {
    Retired,
    AlreadyRetired,
    Conflict,
    PendingProjection,
    ScopeFenced,
}

/// Stable receipt for exact generation retirement.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuntimeGenerationRetireReceipt {
    pub scope: RuntimeScopeAuthority,
    pub runtime_state_id: String,
    pub version: RuntimeStateVersion,
    pub status: RuntimeGenerationRetireStatus,
}

/// Progress of one generation captured by a scope-retirement manifest.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ScopeRetirementItemStatus {
    Pending,
    DroppedByDelete,
}

/// One immutable generation entry captured before product deletion begins.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScopeRetirementItem {
    pub runtime_state_id: String,
    pub state_version: RuntimeStateVersion,
    pub pending_operation_id: Option<String>,
    pub status: ScopeRetirementItemStatus,
}

/// Durable manifest that fences every generation in a stable scope.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScopeRetirementManifest {
    pub delete_operation_id: String,
    pub payload_digest: String,
    pub expected_conversation_epoch: u64,
    pub items: Vec<ScopeRetirementItem>,
    pub conversation_delete_receipt: Option<crate::memory::ManagedConversationDeleteReceipt>,
}

/// Request to begin or replay product-scope retirement.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScopeRetirementRequest {
    pub delete_operation_id: String,
    pub payload_digest: String,
    pub scope_id: String,
    pub expected_scope_revision: u64,
    pub expected_conversation_epoch: u64,
}

/// One durable advancement of an existing retirement manifest.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ScopeRetirementAdvance {
    ConversationDeleted {
        receipt: crate::memory::ManagedConversationDeleteReceipt,
    },
    GenerationDropped {
        runtime_state_id: String,
        pending_operation_id: Option<String>,
    },
    Complete,
}

/// Stable lifecycle result for scope retirement.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ScopeRetirementStatus {
    Begun,
    InProgress,
    Completed,
    AlreadyCompleted,
    RevisionConflict,
    EpochConflict,
    ReceiptExpired,
    IdentityConflict,
}

/// Durable receipt and current manifest for product-scope retirement.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScopeRetirementReceipt {
    pub scope: RuntimeScopeAuthority,
    pub manifest: ScopeRetirementManifest,
    pub dropped_operation_ids: Vec<String>,
    pub retention_floor_epoch: u64,
    pub status: ScopeRetirementStatus,
}

/// Result of deleting one exact runtime-state incarnation from a stable scope.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct RuntimeStateClearReceipt {
    pub scope_id: String,
    pub runtime_state_id: String,
    pub checkpoint_removed: bool,
}

/// Result of deleting every indexed runtime-state incarnation in one scope.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
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
    /// Report revisioned checkpoint support without performing I/O.
    fn runtime_state_capability(&self) -> RuntimeStateCapability {
        RuntimeStateCapability::Unsupported
    }

    /// Load one managed checkpoint/tombstone and its stable scope revision.
    fn load_runtime_state<'a>(
        &'a self,
        _scope_id: &'a str,
        _runtime_state_id: &'a str,
    ) -> futures::future::BoxFuture<'a, crate::error::Result<Option<ManagedRuntimeStateSnapshot>>>
    {
        Box::pin(async {
            Err(echo_core::error::RuntimeStateError::Unsupported(
                "revisioned runtime-state load".to_string(),
            )
            .into())
        })
    }

    /// Load the stable scope authority used to fence admission and deletion.
    fn load_scope_authority<'a>(
        &'a self,
        _scope_id: &'a str,
    ) -> futures::future::BoxFuture<'a, crate::error::Result<Option<RuntimeScopeAuthority>>> {
        Box::pin(async {
            Err(echo_core::error::RuntimeStateError::Unsupported(
                "runtime scope authority load".to_string(),
            )
            .into())
        })
    }

    /// Atomically write a checkpoint only when scope and generation versions match.
    fn compare_and_save_checkpoint<'a>(
        &'a self,
        _request: RuntimeCheckpointCasRequest,
    ) -> futures::future::BoxFuture<'a, crate::error::Result<RuntimeCheckpointCasReceipt>> {
        Box::pin(async {
            Err(echo_core::error::RuntimeStateError::Unsupported(
                "runtime checkpoint compare-and-save".to_string(),
            )
            .into())
        })
    }

    /// Retire one generation behind a durable tombstone.
    fn retire_runtime_generation<'a>(
        &'a self,
        _request: RuntimeGenerationRetireRequest,
    ) -> futures::future::BoxFuture<'a, crate::error::Result<RuntimeGenerationRetireReceipt>> {
        Box::pin(async {
            Err(echo_core::error::RuntimeStateError::Unsupported(
                "runtime generation retirement".to_string(),
            )
            .into())
        })
    }

    /// Fence a stable scope and atomically capture its complete delete manifest.
    fn begin_scope_retirement<'a>(
        &'a self,
        _request: ScopeRetirementRequest,
    ) -> futures::future::BoxFuture<'a, crate::error::Result<ScopeRetirementReceipt>> {
        Box::pin(async {
            Err(echo_core::error::RuntimeStateError::Unsupported(
                "runtime scope retirement".to_string(),
            )
            .into())
        })
    }

    /// Persist one idempotent advancement of a scope-retirement saga.
    fn continue_scope_retirement<'a>(
        &'a self,
        _scope_id: &'a str,
        _delete_operation_id: &'a str,
        _expected_scope_revision: u64,
        _advance: ScopeRetirementAdvance,
    ) -> futures::future::BoxFuture<'a, crate::error::Result<ScopeRetirementReceipt>> {
        Box::pin(async {
            Err(echo_core::error::RuntimeStateError::Unsupported(
                "runtime scope retirement advancement".to_string(),
            )
            .into())
        })
    }

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

    #[test]
    fn pending_transcript_projection_round_trips_and_requires_managed_recovery()
    -> crate::error::Result<()> {
        let batch = crate::memory::TranscriptProjectionBatch::prepare(
            "product-conversation",
            1,
            "conversation-1",
            0,
            vec![crate::memory::StoredMessage {
                id: None,
                conversation_id: "product-conversation".to_string(),
                role: "user".to_string(),
                content: Some("hello".to_string()),
                attachments_json: None,
                tool_calls_json: None,
                tool_result_json: None,
                created_at: "2026-09-16T00:00:00Z".to_string(),
            }],
        )?;
        let projected = vec![TranscriptProjectionMessage {
            ordinal: 0,
            digest: batch
                .items
                .first()
                .map(|item| item.digest.clone())
                .ok_or_else(|| {
                    crate::error::ReactError::Other("prepared batch is empty".to_string())
                })?,
        }];
        let pending = PendingTranscriptProjection {
            batch,
            cursor_before: TranscriptProjectionCheckpoint {
                generation_id: "conversation-1".to_string(),
                next_ordinal: 0,
                projected: Vec::new(),
            },
            cursor_after: TranscriptProjectionCheckpoint {
                generation_id: "conversation-1".to_string(),
                next_ordinal: 1,
                projected,
            },
            base_runtime_revision: 1,
            prepared_at: Utc::now(),
            attempt: 1,
            last_attempt_class: None,
            last_error: None,
        };
        let mut checkpoint = AgentCheckpoint::new("conversation-1");
        checkpoint.messages_json = AgentCheckpoint::serialize_managed_payload(
            vec![Message::user("hello".to_string())],
            Some(pending.cursor_before.clone()),
            Some(pending.clone()),
        )?;

        assert!(checkpoint.restore_runtime_payload().is_err());
        let restored = checkpoint.restore_managed_runtime_payload()?;
        assert_eq!(restored.pending_transcript_projection, Some(pending));
        Ok(())
    }
}
