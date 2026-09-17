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
use sha2::{Digest, Sha256};

const RUNTIME_STATE_OPERATION_SCHEMA_VERSION: u16 = 1;

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
    DeadlineExceeded,
    Unsupported,
    InvalidConfiguration,
    CorruptState,
    SemanticConflict,
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
        let invalid_attempt_metadata = match (
            pending.last_attempt_class.as_ref(),
            pending.last_error.as_deref(),
        ) {
            (None, None) => false,
            (Some(_), Some(error)) => error.trim().is_empty(),
            _ => true,
        };
        if pending.batch.generation_id != self.conversation_id
            || pending.cursor_before.generation_id != self.conversation_id
            || pending.cursor_after.generation_id != self.conversation_id
            || pending.base_runtime_revision == 0
            || pending.attempt == 0
            || invalid_attempt_metadata
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
        let before = pending
            .cursor_before
            .projected
            .iter()
            .map(|message| (message.ordinal, message.digest.clone()))
            .collect::<std::collections::BTreeMap<_, _>>();
        let mut batch = std::collections::BTreeMap::new();
        for item in &pending.batch.items {
            batch.insert(
                item.ordinal,
                crate::memory::transcript_projection_message_digest(&item.message)?,
            );
        }
        let first_batch_ordinal = pending
            .batch
            .items
            .first()
            .map(|item| item.ordinal)
            .ok_or_else(|| invalid_checkpoint("pending transcript batch is empty".to_string()))?;
        for message in &pending.cursor_after.projected {
            let expected = if message.ordinal < first_batch_ordinal {
                before.get(&message.ordinal).map(String::as_str)
            } else {
                batch.get(&message.ordinal).map(String::as_str)
            };
            if expected != Some(message.digest.as_str()) {
                return Err(invalid_checkpoint(
                    "pending cursor-after is not derived from cursor-before and batch".to_string(),
                ));
            }
        }
        if batch.iter().any(|(ordinal, digest)| {
            pending
                .cursor_after
                .projected
                .iter()
                .find(|message| message.ordinal == *ordinal)
                .is_none_or(|message| message.digest.as_str() != digest)
        }) {
            return Err(invalid_checkpoint(
                "pending cursor-after omits a prepared transcript item".to_string(),
            ));
        }
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

/// Proof-carrying request that clears one applied transcript pending marker.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RuntimeTranscriptAckRequest {
    pub scope_id: String,
    pub runtime_state_id: String,
    pub conversation_epoch: u64,
    pub expected_scope_revision: u64,
    pub expected_state_version: RuntimeStateExpectedVersion,
    pub projection_receipt: crate::memory::TranscriptProjectionApplyReceipt,
    pub checkpoint: AgentCheckpoint,
}

impl RuntimeTranscriptAckRequest {
    pub fn validate(&self) -> crate::error::Result<()> {
        validate_runtime_operation_identity(&self.scope_id, &self.runtime_state_id)?;
        if self.conversation_epoch == 0 || self.checkpoint.conversation_id != self.runtime_state_id
        {
            return Err(invalid_runtime_operation(
                "invalid transcript acknowledgement identity",
            ));
        }
        if !matches!(
            self.projection_receipt.status,
            crate::memory::TranscriptProjectionApplyStatus::Applied
                | crate::memory::TranscriptProjectionApplyStatus::AlreadyApplied
        ) || self.projection_receipt.authority.conversation_id != self.scope_id
            || self.projection_receipt.authority.epoch != self.conversation_epoch
        {
            return Err(invalid_runtime_operation(
                "transcript acknowledgement lacks a matching applied receipt",
            ));
        }
        if self
            .checkpoint
            .restore_managed_runtime_payload()?
            .pending_transcript_projection
            .is_some()
        {
            return Err(invalid_runtime_operation(
                "transcript acknowledgement checkpoint still contains pending debt",
            ));
        }
        Ok(())
    }
}

/// Idempotent request to retire one exact runtime generation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RuntimeGenerationRetireRequest {
    pub schema_version: u16,
    pub operation_id: String,
    pub payload_digest: String,
    pub scope_id: String,
    pub runtime_state_id: String,
    pub expected_scope_revision: u64,
    pub expected_state_version: RuntimeStateExpectedVersion,
}

impl RuntimeGenerationRetireRequest {
    pub fn prepare(
        scope_id: impl Into<String>,
        runtime_state_id: impl Into<String>,
        expected_scope_revision: u64,
        expected_state_version: RuntimeStateExpectedVersion,
    ) -> crate::error::Result<Self> {
        let scope_id = scope_id.into();
        let runtime_state_id = runtime_state_id.into();
        validate_runtime_operation_identity(&scope_id, &runtime_state_id)?;
        let payload_digest = runtime_operation_digest(&(
            "runtime-generation-retire-v1",
            RUNTIME_STATE_OPERATION_SCHEMA_VERSION,
            &scope_id,
            &runtime_state_id,
            expected_scope_revision,
            &expected_state_version,
        ))?;
        Ok(Self {
            schema_version: RUNTIME_STATE_OPERATION_SCHEMA_VERSION,
            operation_id: format!("runtime-generation-retire-v1:{payload_digest}"),
            payload_digest,
            scope_id,
            runtime_state_id,
            expected_scope_revision,
            expected_state_version,
        })
    }

    pub fn validate(&self) -> crate::error::Result<()> {
        validate_runtime_operation_identity(&self.scope_id, &self.runtime_state_id)?;
        if self.schema_version != RUNTIME_STATE_OPERATION_SCHEMA_VERSION {
            return Err(invalid_runtime_operation(
                "unsupported runtime generation retirement schema",
            ));
        }
        let digest = runtime_operation_digest(&(
            "runtime-generation-retire-v1",
            self.schema_version,
            &self.scope_id,
            &self.runtime_state_id,
            self.expected_scope_revision,
            &self.expected_state_version,
        ))?;
        if digest != self.payload_digest
            || self.operation_id != format!("runtime-generation-retire-v1:{digest}")
        {
            return Err(invalid_runtime_operation(
                "runtime generation retirement identity does not match its payload",
            ));
        }
        Ok(())
    }
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

impl ScopeRetirementManifest {
    pub fn validate(&self, scope_id: &str) -> crate::error::Result<()> {
        let delete = crate::memory::ManagedConversationDelete {
            schema_version: RUNTIME_STATE_OPERATION_SCHEMA_VERSION,
            operation_id: self.delete_operation_id.clone(),
            payload_digest: self.payload_digest.clone(),
            conversation_id: scope_id.to_string(),
            expected_epoch: self.expected_conversation_epoch,
        };
        delete.validate()?;
        let mut runtime_ids = std::collections::BTreeSet::new();
        for item in &self.items {
            if item.runtime_state_id.trim().is_empty()
                || !runtime_ids.insert(item.runtime_state_id.as_str())
                || matches!(item.state_version, RuntimeStateVersion::Absent)
                || item
                    .pending_operation_id
                    .as_deref()
                    .is_some_and(str::is_empty)
            {
                return Err(invalid_runtime_operation(
                    "scope retirement manifest contains invalid generation identity",
                ));
            }
        }
        if let Some(receipt) = self.conversation_delete_receipt.as_ref()
            && (!matches!(
                receipt.status,
                crate::memory::ManagedConversationDeleteStatus::Deleted
                    | crate::memory::ManagedConversationDeleteStatus::AlreadyDeleted
            ) || receipt.operation_id != self.delete_operation_id
                || receipt.payload_digest != self.payload_digest
                || receipt.deleted_epoch != self.expected_conversation_epoch
                || receipt.retention_floor_epoch >= receipt.deleted_epoch)
        {
            return Err(invalid_runtime_operation(
                "scope retirement manifest contains invalid conversation delete receipt",
            ));
        }
        Ok(())
    }
}

/// Request to begin or replay product-scope retirement.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScopeRetirementRequest {
    pub schema_version: u16,
    pub delete_operation_id: String,
    pub payload_digest: String,
    pub scope_id: String,
    pub expected_scope_revision: u64,
    pub expected_conversation_epoch: u64,
}

impl ScopeRetirementRequest {
    pub fn prepare(
        scope_id: impl Into<String>,
        expected_scope_revision: u64,
        delete: &crate::memory::ManagedConversationDelete,
    ) -> crate::error::Result<Self> {
        delete.validate()?;
        let scope_id = scope_id.into();
        if scope_id != delete.conversation_id {
            return Err(invalid_runtime_operation(
                "scope retirement must share the managed conversation identity",
            ));
        }
        Ok(Self {
            schema_version: RUNTIME_STATE_OPERATION_SCHEMA_VERSION,
            delete_operation_id: delete.operation_id.clone(),
            payload_digest: delete.payload_digest.clone(),
            scope_id,
            expected_scope_revision,
            expected_conversation_epoch: delete.expected_epoch,
        })
    }

    pub fn validate(&self) -> crate::error::Result<()> {
        if self.schema_version != RUNTIME_STATE_OPERATION_SCHEMA_VERSION {
            return Err(invalid_runtime_operation(
                "unsupported runtime scope retirement schema",
            ));
        }
        let delete = crate::memory::ManagedConversationDelete {
            schema_version: self.schema_version,
            operation_id: self.delete_operation_id.clone(),
            payload_digest: self.payload_digest.clone(),
            conversation_id: self.scope_id.clone(),
            expected_epoch: self.expected_conversation_epoch,
        };
        delete.validate()
    }
}

fn validate_runtime_operation_identity(
    scope_id: &str,
    runtime_state_id: &str,
) -> crate::error::Result<()> {
    if scope_id.trim().is_empty() || runtime_state_id.trim().is_empty() {
        return Err(invalid_runtime_operation(
            "runtime operation identities must not be empty",
        ));
    }
    Ok(())
}

fn runtime_operation_digest(value: &impl Serialize) -> crate::error::Result<String> {
    let encoded = serde_json::to_vec(value).map_err(|error| {
        invalid_runtime_operation(format!("failed to serialize runtime operation: {error}"))
    })?;
    Ok(format!("{:x}", Sha256::digest(encoded)))
}

fn invalid_runtime_operation(message: impl Into<String>) -> crate::error::ReactError {
    echo_core::error::RuntimeStateError::SerializationError(message.into()).into()
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

impl ScopeRetirementReceipt {
    pub fn validate(&self) -> crate::error::Result<()> {
        self.manifest.validate(&self.scope.scope_id)?;
        let mut dropped = self.dropped_operation_ids.clone();
        dropped.sort();
        if dropped.windows(2).any(|pair| pair.first() == pair.get(1)) {
            return Err(invalid_runtime_operation(
                "scope retirement receipt contains duplicate dropped operation identity",
            ));
        }
        let mut expected = self
            .manifest
            .items
            .iter()
            .filter(|item| item.status == ScopeRetirementItemStatus::DroppedByDelete)
            .filter_map(|item| item.pending_operation_id.clone())
            .collect::<Vec<_>>();
        expected.sort();
        if dropped != expected {
            return Err(invalid_runtime_operation(
                "scope retirement dropped-operation receipt does not match manifest",
            ));
        }
        let expected_retention_floor = self
            .manifest
            .conversation_delete_receipt
            .as_ref()
            .map(|receipt| receipt.retention_floor_epoch)
            .unwrap_or(0);
        let retention_floor_valid = match self.status {
            ScopeRetirementStatus::Completed => {
                self.retention_floor_epoch == expected_retention_floor
            }
            ScopeRetirementStatus::AlreadyCompleted => {
                self.retention_floor_epoch >= expected_retention_floor
                    && self.retention_floor_epoch < self.manifest.expected_conversation_epoch
            }
            ScopeRetirementStatus::ReceiptExpired => {
                self.retention_floor_epoch >= expected_retention_floor
                    && self.retention_floor_epoch >= self.manifest.expected_conversation_epoch
            }
            _ => self.retention_floor_epoch == expected_retention_floor,
        };
        if !retention_floor_valid {
            return Err(invalid_runtime_operation(
                "scope retirement retention floor is inconsistent with lifecycle status",
            ));
        }
        let completed_history = matches!(
            self.status,
            ScopeRetirementStatus::Completed | ScopeRetirementStatus::AlreadyCompleted
        ) || (self.status == ScopeRetirementStatus::ReceiptExpired
            && self.manifest.conversation_delete_receipt.is_some());
        if completed_history
            && (self.scope.lifecycle != RuntimeScopeLifecycle::Tombstoned
                || self.scope.conversation_epoch != Some(self.manifest.expected_conversation_epoch)
                || self.manifest.conversation_delete_receipt.is_none()
                || self
                    .manifest
                    .items
                    .iter()
                    .any(|item| item.status != ScopeRetirementItemStatus::DroppedByDelete))
        {
            return Err(invalid_runtime_operation(
                "completed scope retirement receipt is incomplete",
            ));
        }
        if self.status == ScopeRetirementStatus::ReceiptExpired
            && self.manifest.conversation_delete_receipt.is_none()
            && (self.scope.lifecycle != RuntimeScopeLifecycle::Retiring
                || self
                    .manifest
                    .items
                    .iter()
                    .any(|item| item.status != ScopeRetirementItemStatus::Pending))
        {
            return Err(invalid_runtime_operation(
                "in-progress expired receipt advanced beyond its retained delete proof",
            ));
        }
        Ok(())
    }
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
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PersistedConversationDeleteReceipt {
    pub conversation_id: String,
    pub runtime_state_ids: Vec<String>,
    /// Complete durable managed-retirement result. Legacy stores do not have a
    /// revisioned retirement authority and therefore return `None`.
    pub retirement: Option<ScopeRetirementReceipt>,
}

#[allow(dead_code)] // The Agent integration slice switches fresh dispatch to CurrentAttempt.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TranscriptProjectionDispatch {
    CurrentAttempt,
    RetryDurablePending,
}

pub(crate) struct TranscriptProjectionSettlementOutcome {
    pub(crate) settlement: crate::memory::TranscriptProjectionSettlement,
    pub(crate) committed_version: Option<RuntimeStateVersion>,
    pub(crate) settled_cursor: Option<TranscriptProjectionCheckpoint>,
}

const PERSISTED_STATE_OPERATION_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);
const PERSISTENCE_FAILURE_RECORD_RESERVE: std::time::Duration = std::time::Duration::from_secs(2);

#[derive(Debug, Clone, Copy)]
struct PersistenceOperationDeadline {
    context: crate::memory::PersistenceCallContext,
    deadline: tokio::time::Instant,
    effect_deadline: tokio::time::Instant,
}

impl PersistenceOperationDeadline {
    #[cfg(test)]
    fn new() -> crate::error::Result<Self> {
        Self::from_context(crate::memory::PersistenceCallContext::with_timeout(
            PERSISTED_STATE_OPERATION_TIMEOUT,
        )?)
    }

    fn from_context(context: crate::memory::PersistenceCallContext) -> crate::error::Result<Self> {
        let absolute_deadline_ms = u64::try_from(context.absolute_deadline_unix_ms)
            .map_err(|_| persistence_deadline_error("invalid persistence deadline"))?;
        let system_deadline = std::time::UNIX_EPOCH
            .checked_add(std::time::Duration::from_millis(absolute_deadline_ms))
            .ok_or_else(|| persistence_deadline_error("persistence deadline overflow"))?;
        let remaining = system_deadline
            .duration_since(std::time::SystemTime::now())
            .map_err(|_| persistence_deadline_error("persistence deadline already elapsed"))?;
        let now = tokio::time::Instant::now();
        let deadline = now
            .checked_add(remaining)
            .ok_or_else(|| persistence_deadline_error("local persistence deadline overflow"))?;
        // Reserve part of a normal operation budget for the durable failure
        // CAS. Short caller budgets use their full deadline rather than timing
        // out an effect before it is first polled.
        let reserved = deadline
            .checked_sub(PERSISTENCE_FAILURE_RECORD_RESERVE)
            .filter(|reserved| *reserved > now)
            .unwrap_or(deadline);
        Ok(Self {
            context,
            deadline,
            effect_deadline: reserved,
        })
    }
}

async fn await_persistence<T>(
    budget: PersistenceOperationDeadline,
    operation: &str,
    future: impl std::future::Future<Output = crate::error::Result<T>>,
) -> crate::error::Result<T> {
    tokio::time::timeout_at(budget.deadline, future)
        .await
        .map_err(|_| persistence_deadline_error(operation))?
}

fn persistence_deadline_error(operation: &str) -> crate::error::ReactError {
    echo_core::error::RuntimeStateError::DeadlineExceeded(operation.to_string()).into()
}

fn persisted_delete_receipt(
    conversation_id: String,
    retirement: ScopeRetirementReceipt,
) -> PersistedConversationDeleteReceipt {
    let runtime_state_ids = retirement
        .manifest
        .items
        .iter()
        .map(|item| item.runtime_state_id.clone())
        .collect();
    PersistedConversationDeleteReceipt {
        conversation_id,
        runtime_state_ids,
        retirement: Some(retirement),
    }
}

/// Trait for persistent runtime state storage.
///
/// Implementations may use SQLite, JSON files, or another durable backend.
pub trait RuntimeStateStore: Send + Sync {
    /// Report revisioned checkpoint support without performing I/O.
    fn runtime_state_capability(&self) -> RuntimeStateCapability {
        RuntimeStateCapability::Unsupported
    }

    /// Report whether context-aware calls enforce the propagated absolute
    /// deadline instead of silently discarding it in an adapter default.
    fn persistence_call_capability(&self) -> crate::memory::PersistenceCallCapability {
        crate::memory::PersistenceCallCapability::Unsupported
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

    fn load_runtime_state_with_context<'a>(
        &'a self,
        _context: crate::memory::PersistenceCallContext,
        scope_id: &'a str,
        runtime_state_id: &'a str,
    ) -> futures::future::BoxFuture<'a, crate::error::Result<Option<ManagedRuntimeStateSnapshot>>>
    {
        self.load_runtime_state(scope_id, runtime_state_id)
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

    fn load_scope_authority_with_context<'a>(
        &'a self,
        _context: crate::memory::PersistenceCallContext,
        scope_id: &'a str,
    ) -> futures::future::BoxFuture<'a, crate::error::Result<Option<RuntimeScopeAuthority>>> {
        self.load_scope_authority(scope_id)
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

    fn compare_and_save_checkpoint_with_context<'a>(
        &'a self,
        _context: crate::memory::PersistenceCallContext,
        request: RuntimeCheckpointCasRequest,
    ) -> futures::future::BoxFuture<'a, crate::error::Result<RuntimeCheckpointCasReceipt>> {
        self.compare_and_save_checkpoint(request)
    }

    /// Clear one pending transcript effect only with a matching applied receipt.
    fn acknowledge_transcript_projection<'a>(
        &'a self,
        _request: RuntimeTranscriptAckRequest,
    ) -> futures::future::BoxFuture<'a, crate::error::Result<RuntimeCheckpointCasReceipt>> {
        Box::pin(async {
            Err(echo_core::error::RuntimeStateError::Unsupported(
                "runtime transcript acknowledgement".to_string(),
            )
            .into())
        })
    }

    fn acknowledge_transcript_projection_with_context<'a>(
        &'a self,
        _context: crate::memory::PersistenceCallContext,
        request: RuntimeTranscriptAckRequest,
    ) -> futures::future::BoxFuture<'a, crate::error::Result<RuntimeCheckpointCasReceipt>> {
        self.acknowledge_transcript_projection(request)
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

    fn retire_runtime_generation_with_context<'a>(
        &'a self,
        _context: crate::memory::PersistenceCallContext,
        request: RuntimeGenerationRetireRequest,
    ) -> futures::future::BoxFuture<'a, crate::error::Result<RuntimeGenerationRetireReceipt>> {
        self.retire_runtime_generation(request)
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

    fn begin_scope_retirement_with_context<'a>(
        &'a self,
        _context: crate::memory::PersistenceCallContext,
        request: ScopeRetirementRequest,
    ) -> futures::future::BoxFuture<'a, crate::error::Result<ScopeRetirementReceipt>> {
        self.begin_scope_retirement(request)
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

    fn continue_scope_retirement_with_context<'a>(
        &'a self,
        _context: crate::memory::PersistenceCallContext,
        scope_id: &'a str,
        delete_operation_id: &'a str,
        expected_scope_revision: u64,
        advance: ScopeRetirementAdvance,
    ) -> futures::future::BoxFuture<'a, crate::error::Result<ScopeRetirementReceipt>> {
        self.continue_scope_retirement(
            scope_id,
            delete_operation_id,
            expected_scope_revision,
            advance,
        )
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

/// Recover and settle the single durable transcript effect owned by one
/// runtime generation.
///
/// Recovery always reuses the batch identity stored in the checkpoint. It
/// first advances the durable attempt with runtime CAS, then dispatches the
/// exact batch and acknowledges it with the backend receipt. All Store calls
/// use `context`; the framework derives its local Tokio deadline from the
/// absolute Unix deadline without exposing process-local time over the API.
pub async fn settle_pending_transcript_projection(
    conversation_store: &dyn crate::memory::ConversationStore,
    runtime_state_store: &dyn RuntimeStateStore,
    scope_id: &str,
    runtime_state_id: &str,
    context: crate::memory::PersistenceCallContext,
) -> crate::error::Result<crate::memory::TranscriptProjectionSettlement> {
    if !managed_persistence_pair(conversation_store, runtime_state_store)? {
        return Err(echo_core::error::RuntimeStateError::Unsupported(
            "transcript settlement requires atomic conversation and revisioned runtime stores"
                .to_string(),
        )
        .into());
    }
    let budget = PersistenceOperationDeadline::from_context(context)?;
    await_persistence(
        budget,
        "durable pending transcript settlement",
        settle_pending_transcript_projection_with_budget(
            conversation_store,
            runtime_state_store,
            scope_id,
            runtime_state_id,
            budget,
        ),
    )
    .await
}

async fn settle_pending_transcript_projection_with_budget(
    conversation_store: &dyn crate::memory::ConversationStore,
    runtime_state_store: &dyn RuntimeStateStore,
    scope_id: &str,
    runtime_state_id: &str,
    budget: PersistenceOperationDeadline,
) -> crate::error::Result<crate::memory::TranscriptProjectionSettlement> {
    let state = await_persistence(
        budget,
        "runtime generation load for transcript settlement",
        runtime_state_store.load_runtime_state_with_context(
            budget.context,
            scope_id,
            runtime_state_id,
        ),
    )
    .await?
    .ok_or_else(|| {
        echo_core::error::RuntimeStateError::NotFound(format!(
            "runtime state {runtime_state_id} is not owned by scope {scope_id}"
        ))
    })?;
    let outcome = await_persistence(
        budget,
        "loaded durable pending transcript settlement",
        settle_loaded_pending_transcript_projection_with_budget(
            conversation_store,
            runtime_state_store,
            scope_id,
            runtime_state_id,
            state,
            TranscriptProjectionDispatch::RetryDurablePending,
            budget,
        ),
    )
    .await?;
    Ok(outcome.settlement)
}

/// Canonical loaded-state settlement core shared by fresh Agent dispatch and
/// durable recovery. Callers must not implement apply/ack ordering themselves.
pub(crate) async fn settle_loaded_pending_transcript_projection(
    conversation_store: &dyn crate::memory::ConversationStore,
    runtime_state_store: &dyn RuntimeStateStore,
    scope_id: &str,
    runtime_state_id: &str,
    state: ManagedRuntimeStateSnapshot,
    dispatch: TranscriptProjectionDispatch,
    context: crate::memory::PersistenceCallContext,
) -> crate::error::Result<TranscriptProjectionSettlementOutcome> {
    if !managed_persistence_pair(conversation_store, runtime_state_store)? {
        return Err(echo_core::error::RuntimeStateError::Unsupported(
            "loaded transcript settlement requires managed stores with absolute deadlines"
                .to_string(),
        )
        .into());
    }
    let budget = PersistenceOperationDeadline::from_context(context)?;
    await_persistence(
        budget,
        "loaded pending transcript settlement",
        settle_loaded_pending_transcript_projection_with_budget(
            conversation_store,
            runtime_state_store,
            scope_id,
            runtime_state_id,
            state,
            dispatch,
            budget,
        ),
    )
    .await
}

async fn settle_loaded_pending_transcript_projection_with_budget(
    conversation_store: &dyn crate::memory::ConversationStore,
    runtime_state_store: &dyn RuntimeStateStore,
    scope_id: &str,
    runtime_state_id: &str,
    state: ManagedRuntimeStateSnapshot,
    dispatch: TranscriptProjectionDispatch,
    budget: PersistenceOperationDeadline,
) -> crate::error::Result<TranscriptProjectionSettlementOutcome> {
    if state.scope.scope_id != scope_id || state.runtime_state_id != runtime_state_id {
        return Err(echo_core::error::RuntimeStateError::SerializationError(
            "loaded runtime state does not match transcript settlement identity".to_string(),
        )
        .into());
    }
    let Some(checkpoint) = state.checkpoint.as_ref() else {
        return Ok(transcript_settlement_outcome(transcript_settlement(
            crate::memory::TranscriptProjectionSettlementStatus::Settled,
            None,
            Some(scope_id.to_string()),
            Some(runtime_state_id.to_string()),
            0,
            None,
            None,
        )));
    };
    let payload = checkpoint.restore_managed_runtime_payload()?;
    let Some(pending) = payload.pending_transcript_projection else {
        return Ok(transcript_settlement_outcome(transcript_settlement(
            crate::memory::TranscriptProjectionSettlementStatus::Settled,
            None,
            Some(scope_id.to_string()),
            Some(runtime_state_id.to_string()),
            0,
            None,
            None,
        )));
    };
    if pending.batch.conversation_id != scope_id || pending.batch.generation_id != runtime_state_id
    {
        return Ok(transcript_settlement_outcome(pending_settlement(
            &pending,
            crate::memory::TranscriptProjectionSettlementStatus::Blocked,
            crate::memory::TranscriptProjectionErrorClass::CorruptState,
            "durable pending transcript identity does not match runtime ownership".to_string(),
        )));
    }
    if let Some(attempt_class) = pending.last_attempt_class.as_ref()
        && !transcript_attempt_is_retryable(attempt_class)
    {
        let status = if *attempt_class == TranscriptProjectionAttemptClass::SemanticConflict {
            crate::memory::TranscriptProjectionSettlementStatus::Conflict
        } else {
            crate::memory::TranscriptProjectionSettlementStatus::Blocked
        };
        return Ok(transcript_settlement_outcome(pending_settlement(
            &pending,
            status,
            transcript_attempt_error_class(attempt_class),
            pending
                .last_error
                .clone()
                .unwrap_or_else(|| "durable transcript failure is not retryable".to_string()),
        )));
    }
    if dispatch == TranscriptProjectionDispatch::CurrentAttempt
        && pending.last_attempt_class.is_some()
    {
        return Ok(transcript_settlement_outcome(pending_settlement(
            &pending,
            crate::memory::TranscriptProjectionSettlementStatus::Blocked,
            crate::memory::TranscriptProjectionErrorClass::CorruptState,
            "current-attempt dispatch requires a freshly prepared pending effect".to_string(),
        )));
    }

    let (state, messages, pending) = match dispatch {
        TranscriptProjectionDispatch::CurrentAttempt => (state, payload.messages, pending),
        TranscriptProjectionDispatch::RetryDurablePending => {
            match prepare_transcript_projection_retry(
                runtime_state_store,
                state,
                payload.messages,
                pending,
                budget,
            )
            .await?
            {
                TranscriptProjectionDispatchPreparation::Ready {
                    state,
                    messages,
                    pending,
                } => (state, messages, pending),
                TranscriptProjectionDispatchPreparation::Terminal(settlement) => {
                    return Ok(transcript_settlement_outcome(settlement));
                }
            }
        }
    };
    dispatch_transcript_projection(
        conversation_store,
        runtime_state_store,
        state,
        messages,
        pending,
        budget,
    )
    .await
}

enum TranscriptProjectionDispatchPreparation {
    Ready {
        state: ManagedRuntimeStateSnapshot,
        messages: Vec<crate::llm::types::Message>,
        pending: PendingTranscriptProjection,
    },
    Terminal(crate::memory::TranscriptProjectionSettlement),
}

async fn prepare_transcript_projection_retry(
    runtime_state_store: &dyn RuntimeStateStore,
    state: ManagedRuntimeStateSnapshot,
    messages: Vec<crate::llm::types::Message>,
    mut pending: PendingTranscriptProjection,
    budget: PersistenceOperationDeadline,
) -> crate::error::Result<TranscriptProjectionDispatchPreparation> {
    let current_revision = match state.version {
        RuntimeStateVersion::Managed { revision } if revision == pending.base_runtime_revision => {
            revision
        }
        _ => {
            return Ok(TranscriptProjectionDispatchPreparation::Terminal(
                pending_settlement(
                    &pending,
                    crate::memory::TranscriptProjectionSettlementStatus::Blocked,
                    crate::memory::TranscriptProjectionErrorClass::CorruptState,
                    "durable pending transcript revision does not match runtime authority"
                        .to_string(),
                ),
            ));
        }
    };
    let next_revision = current_revision.checked_add(1).ok_or_else(|| {
        echo_core::error::RuntimeStateError::RevisionExhausted(
            "pending retry revision reached u64::MAX".to_string(),
        )
    })?;
    let next_attempt = pending.attempt.checked_add(1).ok_or_else(|| {
        echo_core::error::RuntimeStateError::RevisionExhausted(
            "pending transcript attempt reached u32::MAX".to_string(),
        )
    })?;
    pending.attempt = next_attempt;
    pending.last_attempt_class = None;
    pending.last_error = None;
    pending.base_runtime_revision = next_revision;
    let mut checkpoint = state.checkpoint.clone().ok_or_else(|| {
        echo_core::error::RuntimeStateError::SerializationError(
            "durable pending runtime state lost its checkpoint".to_string(),
        )
    })?;
    checkpoint.messages_json = AgentCheckpoint::serialize_managed_payload(
        messages.clone(),
        Some(pending.cursor_before.clone()),
        Some(pending.clone()),
    )?;
    checkpoint.timestamp = Utc::now();
    let retry = tokio::time::timeout_at(
        budget.effect_deadline,
        runtime_state_store.compare_and_save_checkpoint_with_context(
            budget.context,
            RuntimeCheckpointCasRequest {
                scope_id: state.scope.scope_id.clone(),
                runtime_state_id: state.runtime_state_id.clone(),
                conversation_epoch: state.scope.conversation_epoch,
                expected_scope_revision: state.scope.revision,
                expected_state_version: RuntimeStateExpectedVersion::Managed {
                    revision: current_revision,
                },
                checkpoint: checkpoint.clone(),
            },
        ),
    )
    .await;
    let retry = match retry {
        Err(_) => {
            return Ok(TranscriptProjectionDispatchPreparation::Terminal(
                pending_settlement(
                    &pending,
                    crate::memory::TranscriptProjectionSettlementStatus::Deferred,
                    crate::memory::TranscriptProjectionErrorClass::DeadlineExceeded,
                    "pending retry checkpoint deadline elapsed with unknown outcome".to_string(),
                ),
            ));
        }
        Ok(Err(error)) => {
            let (status, error_class) = classify_transcript_persistence_error(&error);
            let reported_attempt = if error_class
                == crate::memory::TranscriptProjectionErrorClass::TransientNoCommit
            {
                pending.attempt.saturating_sub(1)
            } else {
                pending.attempt
            };
            let mut settlement =
                pending_settlement(&pending, status, error_class, error.to_string());
            settlement.attempt = reported_attempt;
            return Ok(TranscriptProjectionDispatchPreparation::Terminal(
                settlement,
            ));
        }
        Ok(Ok(receipt)) => receipt,
    };
    match retry.status {
        RuntimeCheckpointCasStatus::Applied | RuntimeCheckpointCasStatus::AlreadyCurrent => {
            Ok(TranscriptProjectionDispatchPreparation::Ready {
                state: ManagedRuntimeStateSnapshot {
                    scope: retry.scope,
                    runtime_state_id: state.runtime_state_id,
                    version: retry.version,
                    checkpoint: Some(checkpoint),
                },
                messages,
                pending,
            })
        }
        RuntimeCheckpointCasStatus::RevisionConflict => Ok(
            TranscriptProjectionDispatchPreparation::Terminal(pending_settlement(
                &pending,
                crate::memory::TranscriptProjectionSettlementStatus::Deferred,
                crate::memory::TranscriptProjectionErrorClass::RevisionConflict,
                "pending retry checkpoint revision changed".to_string(),
            )),
        ),
        RuntimeCheckpointCasStatus::GenerationRetired | RuntimeCheckpointCasStatus::ScopeFenced => {
            Ok(TranscriptProjectionDispatchPreparation::Terminal(
                pending_settlement(
                    &pending,
                    crate::memory::TranscriptProjectionSettlementStatus::Conflict,
                    crate::memory::TranscriptProjectionErrorClass::SemanticConflict,
                    format!("pending retry checkpoint was fenced: {:?}", retry.status),
                ),
            ))
        }
    }
}

async fn dispatch_transcript_projection(
    conversation_store: &dyn crate::memory::ConversationStore,
    runtime_state_store: &dyn RuntimeStateStore,
    state: ManagedRuntimeStateSnapshot,
    messages: Vec<crate::llm::types::Message>,
    pending: PendingTranscriptProjection,
    budget: PersistenceOperationDeadline,
) -> crate::error::Result<TranscriptProjectionSettlementOutcome> {
    if !matches!(
        state.version,
        RuntimeStateVersion::Managed { revision } if revision == pending.base_runtime_revision
    ) {
        return Ok(transcript_settlement_outcome(pending_settlement(
            &pending,
            crate::memory::TranscriptProjectionSettlementStatus::Blocked,
            crate::memory::TranscriptProjectionErrorClass::CorruptState,
            "pending transcript revision does not match runtime authority".to_string(),
        )));
    }
    let apply = tokio::time::timeout_at(
        budget.effect_deadline,
        conversation_store
            .apply_transcript_projection_with_context(budget.context, pending.batch.clone()),
    )
    .await;
    let projection_receipt = match apply {
        Err(_) => {
            let detail = "transcript apply deadline elapsed with unknown outcome";
            let settlement = recorded_transcript_failure_settlement(
                runtime_state_store,
                &state,
                &messages,
                &pending,
                crate::memory::TranscriptProjectionSettlementStatus::Deferred,
                crate::memory::TranscriptProjectionErrorClass::DeadlineExceeded,
                detail,
                budget,
            )
            .await;
            return Ok(transcript_settlement_outcome(settlement));
        }
        Ok(Err(error)) => {
            let (status, error_class) = classify_transcript_persistence_error(&error);
            let detail = error.to_string();
            let settlement = recorded_transcript_failure_settlement(
                runtime_state_store,
                &state,
                &messages,
                &pending,
                status,
                error_class,
                &detail,
                budget,
            )
            .await;
            return Ok(transcript_settlement_outcome(settlement));
        }
        Ok(Ok(receipt)) => receipt,
    };
    if !matches!(
        projection_receipt.status,
        crate::memory::TranscriptProjectionApplyStatus::Applied
            | crate::memory::TranscriptProjectionApplyStatus::AlreadyApplied
    ) {
        let detail = "conversation store rejected transcript projection";
        let settlement = recorded_transcript_failure_settlement(
            runtime_state_store,
            &state,
            &messages,
            &pending,
            crate::memory::TranscriptProjectionSettlementStatus::Conflict,
            crate::memory::TranscriptProjectionErrorClass::SemanticConflict,
            detail,
            budget,
        )
        .await;
        return Ok(transcript_settlement_outcome(settlement));
    }
    if projection_receipt.operation_id != pending.batch.operation_id
        || projection_receipt.payload_digest != pending.batch.payload_digest
    {
        let detail = "conversation store returned a mismatched projection receipt";
        let settlement = recorded_transcript_failure_settlement(
            runtime_state_store,
            &state,
            &messages,
            &pending,
            crate::memory::TranscriptProjectionSettlementStatus::Conflict,
            crate::memory::TranscriptProjectionErrorClass::SemanticConflict,
            detail,
            budget,
        )
        .await;
        return Ok(transcript_settlement_outcome(settlement));
    }

    let mut checkpoint = state.checkpoint.clone().ok_or_else(|| {
        echo_core::error::RuntimeStateError::SerializationError(
            "prepared pending runtime state lost its checkpoint".to_string(),
        )
    })?;
    checkpoint.messages_json = AgentCheckpoint::serialize_managed_payload(
        messages.clone(),
        Some(pending.cursor_after.clone()),
        None,
    )?;
    checkpoint.timestamp = Utc::now();
    let current_revision = match state.version {
        RuntimeStateVersion::Managed { revision } => revision,
        ref other => {
            return Err(
                echo_core::error::RuntimeStateError::SerializationError(format!(
                    "prepared pending checkpoint has non-managed version {other:?}"
                ))
                .into(),
            );
        }
    };
    let conversation_epoch = state.scope.conversation_epoch.ok_or_else(|| {
        echo_core::error::RuntimeStateError::SerializationError(
            "managed transcript acknowledgement is missing conversation epoch".to_string(),
        )
    })?;
    let ack = tokio::time::timeout_at(
        budget.effect_deadline,
        runtime_state_store.acknowledge_transcript_projection_with_context(
            budget.context,
            RuntimeTranscriptAckRequest {
                scope_id: state.scope.scope_id.clone(),
                runtime_state_id: state.runtime_state_id.clone(),
                conversation_epoch,
                expected_scope_revision: state.scope.revision,
                expected_state_version: RuntimeStateExpectedVersion::Managed {
                    revision: current_revision,
                },
                projection_receipt,
                checkpoint,
            },
        ),
    )
    .await;
    let ack = match ack {
        Err(_) => {
            let detail = "checkpoint acknowledgement deadline elapsed with unknown outcome";
            let settlement = recorded_transcript_failure_settlement(
                runtime_state_store,
                &state,
                &messages,
                &pending,
                crate::memory::TranscriptProjectionSettlementStatus::Deferred,
                crate::memory::TranscriptProjectionErrorClass::DeadlineExceeded,
                detail,
                budget,
            )
            .await;
            return Ok(transcript_settlement_outcome(settlement));
        }
        Ok(Err(error)) => {
            let (status, error_class) = classify_transcript_persistence_error(&error);
            let detail = error.to_string();
            let settlement = recorded_transcript_failure_settlement(
                runtime_state_store,
                &state,
                &messages,
                &pending,
                status,
                error_class,
                &detail,
                budget,
            )
            .await;
            return Ok(transcript_settlement_outcome(settlement));
        }
        Ok(Ok(receipt)) => receipt,
    };
    match ack.status {
        RuntimeCheckpointCasStatus::Applied | RuntimeCheckpointCasStatus::AlreadyCurrent => {
            Ok(TranscriptProjectionSettlementOutcome {
                settlement: transcript_settlement(
                    crate::memory::TranscriptProjectionSettlementStatus::Settled,
                    Some(pending.batch.operation_id),
                    Some(pending.batch.conversation_id),
                    Some(pending.batch.generation_id),
                    pending.attempt,
                    None,
                    None,
                ),
                committed_version: Some(ack.version),
                settled_cursor: Some(pending.cursor_after),
            })
        }
        RuntimeCheckpointCasStatus::RevisionConflict => {
            let detail = "checkpoint acknowledgement revision changed";
            let settlement = recorded_transcript_failure_settlement(
                runtime_state_store,
                &state,
                &messages,
                &pending,
                crate::memory::TranscriptProjectionSettlementStatus::Deferred,
                crate::memory::TranscriptProjectionErrorClass::RevisionConflict,
                detail,
                budget,
            )
            .await;
            Ok(transcript_settlement_outcome(settlement))
        }
        RuntimeCheckpointCasStatus::GenerationRetired | RuntimeCheckpointCasStatus::ScopeFenced => {
            let detail = format!("checkpoint acknowledgement was fenced: {:?}", ack.status);
            let settlement = recorded_transcript_failure_settlement(
                runtime_state_store,
                &state,
                &messages,
                &pending,
                crate::memory::TranscriptProjectionSettlementStatus::Conflict,
                crate::memory::TranscriptProjectionErrorClass::SemanticConflict,
                &detail,
                budget,
            )
            .await;
            Ok(transcript_settlement_outcome(settlement))
        }
    }
}

async fn recorded_transcript_failure_settlement(
    runtime_state_store: &dyn RuntimeStateStore,
    state: &ManagedRuntimeStateSnapshot,
    messages: &[crate::llm::types::Message],
    pending: &PendingTranscriptProjection,
    status: crate::memory::TranscriptProjectionSettlementStatus,
    error_class: crate::memory::TranscriptProjectionErrorClass,
    detail: &str,
    budget: PersistenceOperationDeadline,
) -> crate::memory::TranscriptProjectionSettlement {
    let attempt_class = transcript_error_attempt_class(error_class);
    let retryable = transcript_attempt_is_retryable(&attempt_class);
    if persist_transcript_attempt_failure(
        runtime_state_store,
        state,
        messages,
        pending,
        attempt_class,
        detail,
        budget,
    )
    .await
    {
        pending_settlement(pending, status, error_class, detail.to_string())
    } else {
        pending_settlement(
            pending,
            if retryable {
                crate::memory::TranscriptProjectionSettlementStatus::Deferred
            } else {
                status
            },
            crate::memory::TranscriptProjectionErrorClass::OutcomeUnknown,
            format!("failure classification is not durable; recovery must re-prove it: {detail}"),
        )
    }
}

async fn persist_transcript_attempt_failure(
    runtime_state_store: &dyn RuntimeStateStore,
    state: &ManagedRuntimeStateSnapshot,
    messages: &[crate::llm::types::Message],
    pending: &PendingTranscriptProjection,
    attempt_class: TranscriptProjectionAttemptClass,
    detail: &str,
    budget: PersistenceOperationDeadline,
) -> bool {
    let current_revision = match state.version {
        RuntimeStateVersion::Managed { revision } => revision,
        _ => return false,
    };
    let Some(next_revision) = current_revision.checked_add(1) else {
        return false;
    };
    let Some(mut checkpoint) = state.checkpoint.clone() else {
        return false;
    };
    let mut recorded = pending.clone();
    recorded.base_runtime_revision = next_revision;
    recorded.last_attempt_class = Some(attempt_class.clone());
    recorded.last_error = Some(detail.to_string());
    let Ok(messages_json) = AgentCheckpoint::serialize_managed_payload(
        messages.to_vec(),
        Some(recorded.cursor_before.clone()),
        Some(recorded),
    ) else {
        return false;
    };
    checkpoint.messages_json = messages_json;
    checkpoint.timestamp = Utc::now();
    let persisted = tokio::time::timeout_at(
        budget.deadline,
        runtime_state_store.compare_and_save_checkpoint_with_context(
            budget.context,
            RuntimeCheckpointCasRequest {
                scope_id: state.scope.scope_id.clone(),
                runtime_state_id: state.runtime_state_id.clone(),
                conversation_epoch: state.scope.conversation_epoch,
                expected_scope_revision: state.scope.revision,
                expected_state_version: RuntimeStateExpectedVersion::Managed {
                    revision: current_revision,
                },
                checkpoint,
            },
        ),
    )
    .await;
    if matches!(
        persisted,
        Ok(Ok(RuntimeCheckpointCasReceipt {
            status: RuntimeCheckpointCasStatus::Applied
                | RuntimeCheckpointCasStatus::AlreadyCurrent,
            ..
        }))
    ) {
        return true;
    }
    transcript_failure_record_is_durable(
        runtime_state_store,
        state,
        pending,
        &attempt_class,
        detail,
        budget,
    )
    .await
}

async fn transcript_failure_record_is_durable(
    runtime_state_store: &dyn RuntimeStateStore,
    state: &ManagedRuntimeStateSnapshot,
    pending: &PendingTranscriptProjection,
    attempt_class: &TranscriptProjectionAttemptClass,
    detail: &str,
    budget: PersistenceOperationDeadline,
) -> bool {
    let loaded = tokio::time::timeout_at(
        budget.deadline,
        runtime_state_store.load_runtime_state_with_context(
            budget.context,
            &state.scope.scope_id,
            &state.runtime_state_id,
        ),
    )
    .await;
    let Ok(Ok(Some(loaded))) = loaded else {
        return false;
    };
    let Some(checkpoint) = loaded.checkpoint.as_ref() else {
        return false;
    };
    let Ok(payload) = checkpoint.restore_managed_runtime_payload() else {
        return false;
    };
    let Some(recorded) = payload.pending_transcript_projection else {
        return false;
    };
    matches!(
        loaded.version,
        RuntimeStateVersion::Managed { revision }
            if revision == recorded.base_runtime_revision
    ) && recorded.batch.operation_id == pending.batch.operation_id
        && recorded.batch.payload_digest == pending.batch.payload_digest
        && recorded.attempt == pending.attempt
        && recorded.last_attempt_class.as_ref() == Some(attempt_class)
        && recorded.last_error.as_deref() == Some(detail)
}

fn transcript_attempt_is_retryable(attempt_class: &TranscriptProjectionAttemptClass) -> bool {
    matches!(
        attempt_class,
        TranscriptProjectionAttemptClass::OutcomeUnknown
            | TranscriptProjectionAttemptClass::TransientNoCommit
            | TranscriptProjectionAttemptClass::RevisionConflict
            | TranscriptProjectionAttemptClass::DeadlineExceeded
    )
}

fn transcript_error_attempt_class(
    error_class: crate::memory::TranscriptProjectionErrorClass,
) -> TranscriptProjectionAttemptClass {
    match error_class {
        crate::memory::TranscriptProjectionErrorClass::OutcomeUnknown => {
            TranscriptProjectionAttemptClass::OutcomeUnknown
        }
        crate::memory::TranscriptProjectionErrorClass::TransientNoCommit => {
            TranscriptProjectionAttemptClass::TransientNoCommit
        }
        crate::memory::TranscriptProjectionErrorClass::RevisionConflict => {
            TranscriptProjectionAttemptClass::RevisionConflict
        }
        crate::memory::TranscriptProjectionErrorClass::DeadlineExceeded => {
            TranscriptProjectionAttemptClass::DeadlineExceeded
        }
        crate::memory::TranscriptProjectionErrorClass::Unsupported => {
            TranscriptProjectionAttemptClass::Unsupported
        }
        crate::memory::TranscriptProjectionErrorClass::InvalidConfiguration => {
            TranscriptProjectionAttemptClass::InvalidConfiguration
        }
        crate::memory::TranscriptProjectionErrorClass::CorruptState => {
            TranscriptProjectionAttemptClass::CorruptState
        }
        crate::memory::TranscriptProjectionErrorClass::SemanticConflict => {
            TranscriptProjectionAttemptClass::SemanticConflict
        }
    }
}

fn transcript_attempt_error_class(
    attempt_class: &TranscriptProjectionAttemptClass,
) -> crate::memory::TranscriptProjectionErrorClass {
    match attempt_class {
        TranscriptProjectionAttemptClass::OutcomeUnknown => {
            crate::memory::TranscriptProjectionErrorClass::OutcomeUnknown
        }
        TranscriptProjectionAttemptClass::TransientNoCommit => {
            crate::memory::TranscriptProjectionErrorClass::TransientNoCommit
        }
        TranscriptProjectionAttemptClass::RevisionConflict => {
            crate::memory::TranscriptProjectionErrorClass::RevisionConflict
        }
        TranscriptProjectionAttemptClass::DeadlineExceeded => {
            crate::memory::TranscriptProjectionErrorClass::DeadlineExceeded
        }
        TranscriptProjectionAttemptClass::Unsupported => {
            crate::memory::TranscriptProjectionErrorClass::Unsupported
        }
        TranscriptProjectionAttemptClass::InvalidConfiguration => {
            crate::memory::TranscriptProjectionErrorClass::InvalidConfiguration
        }
        TranscriptProjectionAttemptClass::CorruptState => {
            crate::memory::TranscriptProjectionErrorClass::CorruptState
        }
        TranscriptProjectionAttemptClass::SemanticConflict => {
            crate::memory::TranscriptProjectionErrorClass::SemanticConflict
        }
    }
}

pub(crate) fn classify_transcript_persistence_error(
    error: &crate::error::ReactError,
) -> (
    crate::memory::TranscriptProjectionSettlementStatus,
    crate::memory::TranscriptProjectionErrorClass,
) {
    match error {
        crate::error::ReactError::Config(_) => (
            crate::memory::TranscriptProjectionSettlementStatus::Blocked,
            crate::memory::TranscriptProjectionErrorClass::InvalidConfiguration,
        ),
        crate::error::ReactError::Memory(error) => match error.as_ref() {
            echo_core::error::MemoryError::Unsupported(_) => (
                crate::memory::TranscriptProjectionSettlementStatus::Blocked,
                crate::memory::TranscriptProjectionErrorClass::Unsupported,
            ),
            echo_core::error::MemoryError::SerializationError(_)
            | echo_core::error::MemoryError::NotFound(_)
            | echo_core::error::MemoryError::ManagedConversationRequiresProjection(_)
            | echo_core::error::MemoryError::ProjectionEpochExhausted(_) => (
                crate::memory::TranscriptProjectionSettlementStatus::Blocked,
                crate::memory::TranscriptProjectionErrorClass::CorruptState,
            ),
            echo_core::error::MemoryError::TransientNoCommit(_) => (
                crate::memory::TranscriptProjectionSettlementStatus::Deferred,
                crate::memory::TranscriptProjectionErrorClass::TransientNoCommit,
            ),
            echo_core::error::MemoryError::DeadlineExceeded(_) => (
                crate::memory::TranscriptProjectionSettlementStatus::Deferred,
                crate::memory::TranscriptProjectionErrorClass::DeadlineExceeded,
            ),
            _ => (
                crate::memory::TranscriptProjectionSettlementStatus::Deferred,
                crate::memory::TranscriptProjectionErrorClass::OutcomeUnknown,
            ),
        },
        crate::error::ReactError::RuntimeState(error) => match error.as_ref() {
            echo_core::error::RuntimeStateError::Unsupported(_) => (
                crate::memory::TranscriptProjectionSettlementStatus::Blocked,
                crate::memory::TranscriptProjectionErrorClass::Unsupported,
            ),
            echo_core::error::RuntimeStateError::SerializationError(_)
            | echo_core::error::RuntimeStateError::NotFound(_)
            | echo_core::error::RuntimeStateError::ManagedStateRequiresCas(_)
            | echo_core::error::RuntimeStateError::RevisionExhausted(_) => (
                crate::memory::TranscriptProjectionSettlementStatus::Blocked,
                crate::memory::TranscriptProjectionErrorClass::CorruptState,
            ),
            echo_core::error::RuntimeStateError::DeadlineExceeded(_)
            | echo_core::error::RuntimeStateError::TranscriptProjectionDeferred { .. } => (
                crate::memory::TranscriptProjectionSettlementStatus::Deferred,
                crate::memory::TranscriptProjectionErrorClass::DeadlineExceeded,
            ),
            echo_core::error::RuntimeStateError::TransientNoCommit(_) => (
                crate::memory::TranscriptProjectionSettlementStatus::Deferred,
                crate::memory::TranscriptProjectionErrorClass::TransientNoCommit,
            ),
            _ => (
                crate::memory::TranscriptProjectionSettlementStatus::Deferred,
                crate::memory::TranscriptProjectionErrorClass::OutcomeUnknown,
            ),
        },
        _ => (
            crate::memory::TranscriptProjectionSettlementStatus::Deferred,
            crate::memory::TranscriptProjectionErrorClass::OutcomeUnknown,
        ),
    }
}

fn pending_settlement(
    pending: &PendingTranscriptProjection,
    status: crate::memory::TranscriptProjectionSettlementStatus,
    error_class: crate::memory::TranscriptProjectionErrorClass,
    detail: String,
) -> crate::memory::TranscriptProjectionSettlement {
    transcript_settlement(
        status,
        Some(pending.batch.operation_id.clone()),
        Some(pending.batch.conversation_id.clone()),
        Some(pending.batch.generation_id.clone()),
        pending.attempt,
        Some(error_class),
        Some(detail),
    )
}

fn transcript_settlement_outcome(
    settlement: crate::memory::TranscriptProjectionSettlement,
) -> TranscriptProjectionSettlementOutcome {
    TranscriptProjectionSettlementOutcome {
        settlement,
        committed_version: None,
        settled_cursor: None,
    }
}

fn transcript_settlement(
    status: crate::memory::TranscriptProjectionSettlementStatus,
    operation_id: Option<String>,
    conversation_id: Option<String>,
    generation_id: Option<String>,
    attempt: u32,
    error_class: Option<crate::memory::TranscriptProjectionErrorClass>,
    detail: Option<String>,
) -> crate::memory::TranscriptProjectionSettlement {
    crate::memory::TranscriptProjectionSettlement {
        status,
        operation_id,
        conversation_id,
        generation_id,
        attempt,
        error_class,
        detail,
    }
}

fn require_settled_transcript_projection(
    settlement: crate::memory::TranscriptProjectionSettlement,
) -> crate::error::Result<()> {
    match settlement.status {
        crate::memory::TranscriptProjectionSettlementStatus::Settled => Ok(()),
        crate::memory::TranscriptProjectionSettlementStatus::Deferred => Err(
            echo_core::error::RuntimeStateError::TranscriptProjectionDeferred {
                operation_id: settlement.operation_id,
                reason: settlement
                    .detail
                    .unwrap_or_else(|| "transcript settlement remains deferred".to_string()),
            }
            .into(),
        ),
        crate::memory::TranscriptProjectionSettlementStatus::Blocked
        | crate::memory::TranscriptProjectionSettlementStatus::Conflict => Err(
            echo_core::error::RuntimeStateError::TranscriptProjectionBlocked {
                status: format!("{:?}", settlement.status),
                reason: settlement
                    .detail
                    .unwrap_or_else(|| "transcript settlement is blocked".to_string()),
            }
            .into(),
        ),
    }
}

/// Delete one retired runtime incarnation without deleting its stable product
/// transcript.
///
/// Managed state first settles its durable transcript effect, verifies the
/// pending marker is gone, and retires the generation behind a tombstone. It
/// never deletes a separate transcript that happens to share the incarnation
/// ID because no stable transcript-delete identity is retained here. Legacy
/// unmanaged pairs keep their historical incarnation-transcript cleanup.
pub async fn clear_persisted_runtime_incarnation(
    conversation_store: &dyn crate::memory::ConversationStore,
    runtime_state_store: &dyn RuntimeStateStore,
    scope_id: &str,
    runtime_state_id: &str,
) -> crate::error::Result<RuntimeStateClearReceipt> {
    if managed_persistence_pair(conversation_store, runtime_state_store)? {
        return clear_persisted_runtime_incarnation_with_context(
            conversation_store,
            runtime_state_store,
            scope_id,
            runtime_state_id,
            crate::memory::PersistenceCallContext::with_timeout(PERSISTED_STATE_OPERATION_TIMEOUT)?,
        )
        .await;
    }
    clear_legacy_runtime_incarnation(
        conversation_store,
        runtime_state_store,
        scope_id,
        runtime_state_id,
    )
    .await
}

/// Context-aware managed exact clear. Legacy Store APIs do not carry call
/// context and therefore fail closed here instead of risking a late delete.
pub async fn clear_persisted_runtime_incarnation_with_context(
    conversation_store: &dyn crate::memory::ConversationStore,
    runtime_state_store: &dyn RuntimeStateStore,
    scope_id: &str,
    runtime_state_id: &str,
    context: crate::memory::PersistenceCallContext,
) -> crate::error::Result<RuntimeStateClearReceipt> {
    let budget = PersistenceOperationDeadline::from_context(context)?;
    if !managed_persistence_pair(conversation_store, runtime_state_store)? {
        return Err(echo_core::error::RuntimeStateError::Unsupported(
            "context-aware exact clear is unsupported for legacy stores".to_string(),
        )
        .into());
    }
    await_persistence(
        budget,
        "managed runtime incarnation clear",
        clear_managed_runtime_incarnation(
            conversation_store,
            runtime_state_store,
            scope_id,
            runtime_state_id,
            budget,
        ),
    )
    .await
}

fn managed_persistence_pair(
    conversation_store: &dyn crate::memory::ConversationStore,
    runtime_state_store: &dyn RuntimeStateStore,
) -> crate::error::Result<bool> {
    let conversation_managed = conversation_store.projection_capability()
        == crate::memory::ConversationProjectionCapability::AtomicV1;
    let runtime_managed =
        runtime_state_store.runtime_state_capability() == RuntimeStateCapability::RevisionedV1;
    match (conversation_managed, runtime_managed) {
        (false, false) => Ok(false),
        (true, true)
            if conversation_store.persistence_call_capability()
                == crate::memory::PersistenceCallCapability::AbsoluteDeadlineV1
                && runtime_state_store.persistence_call_capability()
                    == crate::memory::PersistenceCallCapability::AbsoluteDeadlineV1 =>
        {
            Ok(true)
        }
        (true, true) => Err(echo_core::error::RuntimeStateError::Unsupported(
            "managed persistence stores must enforce absolute deadline call context".to_string(),
        )
        .into()),
        _ => Err(echo_core::error::RuntimeStateError::Unsupported(
            "conversation and runtime stores must advertise compatible managed persistence capabilities"
                .to_string(),
        )
        .into()),
    }
}

async fn clear_legacy_runtime_incarnation(
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

async fn clear_managed_runtime_incarnation(
    conversation_store: &dyn crate::memory::ConversationStore,
    runtime_state_store: &dyn RuntimeStateStore,
    scope_id: &str,
    runtime_state_id: &str,
    budget: PersistenceOperationDeadline,
) -> crate::error::Result<RuntimeStateClearReceipt> {
    let settlement = settle_pending_transcript_projection_with_budget(
        conversation_store,
        runtime_state_store,
        scope_id,
        runtime_state_id,
        budget,
    )
    .await?;
    require_settled_transcript_projection(settlement)?;

    let state = await_persistence(
        budget,
        "runtime generation load after transcript settlement",
        runtime_state_store.load_runtime_state_with_context(
            budget.context,
            scope_id,
            runtime_state_id,
        ),
    )
    .await?
    .ok_or_else(|| {
        echo_core::error::RuntimeStateError::NotFound(format!(
            "runtime state {runtime_state_id} is not owned by scope {scope_id}"
        ))
    })?;
    if let Some(pending) = pending_transcript_projection(&state)? {
        return Err(
            echo_core::error::RuntimeStateError::TranscriptProjectionDeferred {
                operation_id: Some(pending.batch.operation_id),
                reason:
                    "canonical transcript settlement returned without clearing durable pending state"
                        .to_string(),
            }
            .into(),
        );
    }

    let checkpoint_removed = if matches!(state.version, RuntimeStateVersion::Retired { .. }) {
        false
    } else {
        let request = RuntimeGenerationRetireRequest::prepare(
            scope_id,
            runtime_state_id,
            state.scope.revision,
            expected_runtime_state_version(&state.version),
        )?;
        let receipt = await_persistence(
            budget,
            "runtime generation retirement",
            runtime_state_store.retire_runtime_generation_with_context(budget.context, request),
        )
        .await?;
        match receipt.status {
            RuntimeGenerationRetireStatus::Retired
            | RuntimeGenerationRetireStatus::AlreadyRetired => true,
            RuntimeGenerationRetireStatus::PendingProjection => {
                let operation_id = await_persistence(
                    budget,
                    "runtime generation reload after pending retirement",
                    runtime_state_store.load_runtime_state_with_context(
                        budget.context,
                        scope_id,
                        runtime_state_id,
                    ),
                )
                .await?
                .as_ref()
                .map(pending_transcript_projection)
                .transpose()?
                .flatten()
                .map(|pending| pending.batch.operation_id);
                return Err(
                    echo_core::error::RuntimeStateError::TranscriptProjectionDeferred {
                        operation_id,
                        reason: "runtime generation acquired a new unsettled transcript projection"
                            .to_string(),
                    }
                    .into(),
                );
            }
            RuntimeGenerationRetireStatus::Conflict
            | RuntimeGenerationRetireStatus::ScopeFenced => {
                return Err(
                    echo_core::error::RuntimeStateError::ManagedStateRequiresCas(format!(
                        "runtime generation retirement did not settle: {:?}",
                        receipt.status
                    ))
                    .into(),
                );
            }
        }
    };

    Ok(RuntimeStateClearReceipt {
        scope_id: scope_id.to_string(),
        runtime_state_id: runtime_state_id.to_string(),
        checkpoint_removed,
    })
}

fn pending_transcript_projection(
    state: &ManagedRuntimeStateSnapshot,
) -> crate::error::Result<Option<PendingTranscriptProjection>> {
    state
        .checkpoint
        .as_ref()
        .map(AgentCheckpoint::restore_managed_runtime_payload)
        .transpose()
        .map(|payload| payload.and_then(|payload| payload.pending_transcript_projection))
}

fn expected_runtime_state_version(version: &RuntimeStateVersion) -> RuntimeStateExpectedVersion {
    match version {
        RuntimeStateVersion::Absent => RuntimeStateExpectedVersion::Absent,
        RuntimeStateVersion::Unmanaged { digest } => RuntimeStateExpectedVersion::Unmanaged {
            digest: digest.clone(),
        },
        RuntimeStateVersion::Managed { revision } => RuntimeStateExpectedVersion::Managed {
            revision: *revision,
        },
        RuntimeStateVersion::Retired { .. } => RuntimeStateExpectedVersion::Absent,
    }
}

/// Delete a stable user-visible transcript and every runtime checkpoint bound
/// to the same scope.
///
/// This compatibility entry point only operates on legacy unmanaged stores.
/// Managed stores require [`delete_persisted_conversation_managed`] so retries
/// retain the original epoch-fenced delete identity.
pub async fn delete_persisted_conversation(
    conversation_store: &dyn crate::memory::ConversationStore,
    runtime_state_store: &dyn RuntimeStateStore,
    conversation_id: &str,
) -> crate::error::Result<PersistedConversationDeleteReceipt> {
    if managed_persistence_pair(conversation_store, runtime_state_store)? {
        return Err(echo_core::error::RuntimeStateError::ManagedStateRequiresCas(
            "managed conversation deletion requires delete_persisted_conversation_managed with a stable ManagedConversationDelete request"
                .to_string(),
        )
        .into());
    }
    delete_legacy_persisted_conversation(conversation_store, runtime_state_store, conversation_id)
        .await
}

/// Context-aware delete compatibility boundary. Managed stores must use the
/// stable-request API; legacy stores fail closed because their raw delete APIs
/// cannot fence a blocking write after caller cancellation.
pub async fn delete_persisted_conversation_with_context(
    conversation_store: &dyn crate::memory::ConversationStore,
    runtime_state_store: &dyn RuntimeStateStore,
    _conversation_id: &str,
    context: crate::memory::PersistenceCallContext,
) -> crate::error::Result<PersistedConversationDeleteReceipt> {
    let _budget = PersistenceOperationDeadline::from_context(context)?;
    if managed_persistence_pair(conversation_store, runtime_state_store)? {
        return Err(echo_core::error::RuntimeStateError::ManagedStateRequiresCas(
            "managed conversation deletion requires delete_persisted_conversation_managed with a stable ManagedConversationDelete request"
                .to_string(),
        )
        .into());
    }
    Err(echo_core::error::RuntimeStateError::Unsupported(
        "context-aware conversation delete is unsupported for legacy stores".to_string(),
    )
    .into())
}

async fn delete_legacy_persisted_conversation(
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
        retirement: None,
    })
}

/// Delete one managed conversation using a caller-retained, retry-stable
/// request identity.
///
/// This is the only managed entry point. A retry must pass the same
/// [`crate::memory::ManagedConversationDelete`], including after a lost
/// acknowledgement or after the same conversation ID has been recreated. The
/// runtime retirement authority is queried before any transcript mutation, so
/// a retained completion receipt wins over the current conversation epoch.
pub async fn delete_persisted_conversation_managed(
    conversation_store: &dyn crate::memory::ConversationStore,
    runtime_state_store: &dyn RuntimeStateStore,
    delete: crate::memory::ManagedConversationDelete,
) -> crate::error::Result<PersistedConversationDeleteReceipt> {
    delete_persisted_conversation_managed_with_context(
        conversation_store,
        runtime_state_store,
        delete,
        crate::memory::PersistenceCallContext::with_timeout(PERSISTED_STATE_OPERATION_TIMEOUT)?,
    )
    .await
}

/// Context-aware managed delete. A retry supplies the same durable request
/// while refreshing only the non-authoritative absolute call deadline.
pub async fn delete_persisted_conversation_managed_with_context(
    conversation_store: &dyn crate::memory::ConversationStore,
    runtime_state_store: &dyn RuntimeStateStore,
    delete: crate::memory::ManagedConversationDelete,
    context: crate::memory::PersistenceCallContext,
) -> crate::error::Result<PersistedConversationDeleteReceipt> {
    let budget = PersistenceOperationDeadline::from_context(context)?;
    delete.validate()?;
    if !managed_persistence_pair(conversation_store, runtime_state_store)? {
        return Err(echo_core::error::RuntimeStateError::Unsupported(
            "managed conversation deletion requires atomic conversation and revisioned runtime stores"
                .to_string(),
        )
        .into());
    }
    await_persistence(
        budget,
        "managed persisted conversation delete",
        delete_managed_persisted_conversation(
            conversation_store,
            runtime_state_store,
            delete,
            budget,
        ),
    )
    .await
}

async fn delete_managed_persisted_conversation(
    conversation_store: &dyn crate::memory::ConversationStore,
    runtime_state_store: &dyn RuntimeStateStore,
    delete: crate::memory::ManagedConversationDelete,
    budget: PersistenceOperationDeadline,
) -> crate::error::Result<PersistedConversationDeleteReceipt> {
    let conversation_id = delete.conversation_id.clone();
    let scope_revision = await_persistence(
        budget,
        "runtime scope load before managed delete",
        runtime_state_store.load_scope_authority_with_context(budget.context, &conversation_id),
    )
    .await?
    .map(|scope| scope.revision)
    .unwrap_or(0);
    let request = ScopeRetirementRequest::prepare(&conversation_id, scope_revision, &delete)?;
    let mut retirement = await_persistence(
        budget,
        "runtime scope retirement begin",
        runtime_state_store.begin_scope_retirement_with_context(budget.context, request),
    )
    .await?;
    retirement.validate()?;
    if matches!(
        retirement.status,
        ScopeRetirementStatus::Completed | ScopeRetirementStatus::AlreadyCompleted
    ) || !scope_retirement_can_advance(retirement.status)
    {
        return Ok(persisted_delete_receipt(conversation_id, retirement));
    }

    if retirement.manifest.conversation_delete_receipt.is_none() {
        let conversation_receipt = await_persistence(
            budget,
            "managed conversation transcript delete",
            conversation_store
                .delete_managed_conversation_with_context(budget.context, delete.clone()),
        )
        .await?;
        retirement = await_persistence(
            budget,
            "managed conversation delete receipt persistence",
            runtime_state_store.continue_scope_retirement_with_context(
                budget.context,
                &conversation_id,
                &delete.operation_id,
                retirement.scope.revision,
                ScopeRetirementAdvance::ConversationDeleted {
                    receipt: conversation_receipt,
                },
            ),
        )
        .await?;
        retirement.validate()?;
        if !scope_retirement_can_advance(retirement.status) {
            return Ok(persisted_delete_receipt(conversation_id, retirement));
        }
    }

    let pending_items = retirement
        .manifest
        .items
        .iter()
        .filter(|item| item.status == ScopeRetirementItemStatus::Pending)
        .map(|item| {
            (
                item.runtime_state_id.clone(),
                item.pending_operation_id.clone(),
            )
        })
        .collect::<Vec<_>>();
    for (runtime_state_id, pending_operation_id) in pending_items {
        retirement = await_persistence(
            budget,
            "managed runtime generation drop",
            runtime_state_store.continue_scope_retirement_with_context(
                budget.context,
                &conversation_id,
                &delete.operation_id,
                retirement.scope.revision,
                ScopeRetirementAdvance::GenerationDropped {
                    runtime_state_id,
                    pending_operation_id,
                },
            ),
        )
        .await?;
        retirement.validate()?;
        if !scope_retirement_can_advance(retirement.status) {
            return Ok(persisted_delete_receipt(conversation_id, retirement));
        }
    }
    retirement = await_persistence(
        budget,
        "managed runtime scope retirement completion",
        runtime_state_store.continue_scope_retirement_with_context(
            budget.context,
            &conversation_id,
            &delete.operation_id,
            retirement.scope.revision,
            ScopeRetirementAdvance::Complete,
        ),
    )
    .await?;
    retirement.validate()?;
    Ok(persisted_delete_receipt(conversation_id, retirement))
}

fn scope_retirement_can_advance(status: ScopeRetirementStatus) -> bool {
    matches!(
        status,
        ScopeRetirementStatus::Begun | ScopeRetirementStatus::InProgress
    )
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

    fn managed_projection_request(
        conversation_id: &str,
        expected_tombstone_epoch: Option<u64>,
    ) -> crate::memory::EnsureConversationProjectionRequest {
        crate::memory::EnsureConversationProjectionRequest {
            conversation: crate::memory::NewConversation {
                conversation_id: conversation_id.to_string(),
                user_id: "default".to_string(),
                agent_type: None,
                title: None,
            },
            expected_tombstone_epoch,
        }
    }

    fn expired_persistence_context() -> crate::memory::PersistenceCallContext {
        crate::memory::PersistenceCallContext {
            absolute_deadline_unix_ms: 1,
        }
    }

    fn pending_projection_checkpoint(
        conversation_id: &str,
        runtime_state_id: &str,
        epoch: u64,
    ) -> crate::error::Result<(AgentCheckpoint, PendingTranscriptProjection)> {
        let batch = crate::memory::TranscriptProjectionBatch::prepare(
            conversation_id,
            epoch,
            runtime_state_id,
            0,
            vec![crate::memory::StoredMessage {
                id: None,
                conversation_id: conversation_id.to_string(),
                role: "user".to_string(),
                content: Some("pending transcript".to_string()),
                attachments_json: None,
                tool_calls_json: None,
                tool_result_json: None,
                created_at: "2026-09-17T00:00:00Z".to_string(),
            }],
        )?;
        let projected_item = batch.items.first().ok_or_else(|| {
            crate::error::ReactError::Other("prepared batch is empty".to_string())
        })?;
        let projected = TranscriptProjectionMessage {
            ordinal: projected_item.ordinal,
            digest: crate::memory::transcript_projection_message_digest(&projected_item.message)?,
        };
        let pending = PendingTranscriptProjection {
            batch,
            cursor_before: TranscriptProjectionCheckpoint {
                generation_id: runtime_state_id.to_string(),
                next_ordinal: 0,
                projected: Vec::new(),
            },
            cursor_after: TranscriptProjectionCheckpoint {
                generation_id: runtime_state_id.to_string(),
                next_ordinal: 1,
                projected: vec![projected],
            },
            base_runtime_revision: 1,
            prepared_at: Utc::now(),
            attempt: 1,
            last_attempt_class: None,
            last_error: None,
        };
        let mut checkpoint = AgentCheckpoint::new(runtime_state_id);
        checkpoint.messages_json = AgentCheckpoint::serialize_managed_payload(
            vec![Message::user("pending transcript".to_string())],
            Some(pending.cursor_before.clone()),
            Some(pending.clone()),
        )?;
        Ok((checkpoint, pending))
    }

    #[derive(Clone)]
    struct RejectFailureCasRuntimeStore {
        inner: FileRuntimeStateStore,
    }

    impl RejectFailureCasRuntimeStore {
        fn rejects(request: &RuntimeCheckpointCasRequest) -> bool {
            request
                .checkpoint
                .restore_managed_runtime_payload()
                .map(|payload| {
                    payload
                        .pending_transcript_projection
                        .is_some_and(|pending| pending.last_attempt_class.is_some())
                })
                .unwrap_or(false)
        }

        fn rejected_receipt() -> crate::error::ReactError {
            echo_core::error::RuntimeStateError::TransientNoCommit(
                "injected failure-class CAS rejection".to_string(),
            )
            .into()
        }
    }

    impl RuntimeStateStore for RejectFailureCasRuntimeStore {
        fn runtime_state_capability(&self) -> RuntimeStateCapability {
            RuntimeStateCapability::RevisionedV1
        }

        fn persistence_call_capability(&self) -> crate::memory::PersistenceCallCapability {
            crate::memory::PersistenceCallCapability::AbsoluteDeadlineV1
        }

        fn load_runtime_state<'a>(
            &'a self,
            scope_id: &'a str,
            runtime_state_id: &'a str,
        ) -> futures::future::BoxFuture<'a, crate::error::Result<Option<ManagedRuntimeStateSnapshot>>>
        {
            self.inner.load_runtime_state(scope_id, runtime_state_id)
        }

        fn load_runtime_state_with_context<'a>(
            &'a self,
            context: crate::memory::PersistenceCallContext,
            scope_id: &'a str,
            runtime_state_id: &'a str,
        ) -> futures::future::BoxFuture<'a, crate::error::Result<Option<ManagedRuntimeStateSnapshot>>>
        {
            self.inner
                .load_runtime_state_with_context(context, scope_id, runtime_state_id)
        }

        fn load_scope_authority<'a>(
            &'a self,
            scope_id: &'a str,
        ) -> futures::future::BoxFuture<'a, crate::error::Result<Option<RuntimeScopeAuthority>>>
        {
            self.inner.load_scope_authority(scope_id)
        }

        fn load_scope_authority_with_context<'a>(
            &'a self,
            context: crate::memory::PersistenceCallContext,
            scope_id: &'a str,
        ) -> futures::future::BoxFuture<'a, crate::error::Result<Option<RuntimeScopeAuthority>>>
        {
            self.inner
                .load_scope_authority_with_context(context, scope_id)
        }

        fn compare_and_save_checkpoint<'a>(
            &'a self,
            request: RuntimeCheckpointCasRequest,
        ) -> futures::future::BoxFuture<'a, crate::error::Result<RuntimeCheckpointCasReceipt>>
        {
            if Self::rejects(&request) {
                return Box::pin(async { Err(Self::rejected_receipt()) });
            }
            self.inner.compare_and_save_checkpoint(request)
        }

        fn compare_and_save_checkpoint_with_context<'a>(
            &'a self,
            context: crate::memory::PersistenceCallContext,
            request: RuntimeCheckpointCasRequest,
        ) -> futures::future::BoxFuture<'a, crate::error::Result<RuntimeCheckpointCasReceipt>>
        {
            if Self::rejects(&request) {
                return Box::pin(async { Err(Self::rejected_receipt()) });
            }
            self.inner
                .compare_and_save_checkpoint_with_context(context, request)
        }

        fn acknowledge_transcript_projection<'a>(
            &'a self,
            request: RuntimeTranscriptAckRequest,
        ) -> futures::future::BoxFuture<'a, crate::error::Result<RuntimeCheckpointCasReceipt>>
        {
            self.inner.acknowledge_transcript_projection(request)
        }

        fn acknowledge_transcript_projection_with_context<'a>(
            &'a self,
            context: crate::memory::PersistenceCallContext,
            request: RuntimeTranscriptAckRequest,
        ) -> futures::future::BoxFuture<'a, crate::error::Result<RuntimeCheckpointCasReceipt>>
        {
            self.inner
                .acknowledge_transcript_projection_with_context(context, request)
        }

        fn retire_runtime_generation<'a>(
            &'a self,
            request: RuntimeGenerationRetireRequest,
        ) -> futures::future::BoxFuture<'a, crate::error::Result<RuntimeGenerationRetireReceipt>>
        {
            self.inner.retire_runtime_generation(request)
        }

        fn get_checkpoint<'a>(
            &'a self,
            conversation_id: &'a str,
        ) -> futures::future::BoxFuture<'a, crate::error::Result<Option<AgentCheckpoint>>> {
            self.inner.get_checkpoint(conversation_id)
        }

        fn save_checkpoint<'a>(
            &'a self,
            checkpoint: &'a AgentCheckpoint,
        ) -> futures::future::BoxFuture<'a, crate::error::Result<()>> {
            self.inner.save_checkpoint(checkpoint)
        }

        fn save_checkpoint_for_scope<'a>(
            &'a self,
            scope_id: &'a str,
            checkpoint: &'a AgentCheckpoint,
        ) -> futures::future::BoxFuture<'a, crate::error::Result<()>> {
            self.inner.save_checkpoint_for_scope(scope_id, checkpoint)
        }

        fn runtime_state_ids<'a>(
            &'a self,
            scope_id: &'a str,
        ) -> futures::future::BoxFuture<'a, crate::error::Result<Vec<String>>> {
            self.inner.runtime_state_ids(scope_id)
        }

        fn clear_runtime_state<'a>(
            &'a self,
            scope_id: &'a str,
            runtime_state_id: &'a str,
        ) -> futures::future::BoxFuture<'a, crate::error::Result<RuntimeStateClearReceipt>>
        {
            self.inner.clear_runtime_state(scope_id, runtime_state_id)
        }

        fn clear_runtime_state_scope<'a>(
            &'a self,
            scope_id: &'a str,
        ) -> futures::future::BoxFuture<'a, crate::error::Result<RuntimeStateScopeClearReceipt>>
        {
            self.inner.clear_runtime_state_scope(scope_id)
        }

        fn clear_conversation<'a>(
            &'a self,
            conversation_id: &'a str,
        ) -> futures::future::BoxFuture<'a, crate::error::Result<()>> {
            self.inner.clear_conversation(conversation_id)
        }
    }

    #[derive(Default)]
    struct LegacyConversationProbe {
        calls: std::sync::atomic::AtomicUsize,
    }

    impl LegacyConversationProbe {
        fn unexpected<T: Send + 'static>(
            &self,
        ) -> futures::future::BoxFuture<'static, crate::error::Result<T>> {
            self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Box::pin(async {
                Err(crate::error::ReactError::Other(
                    "legacy conversation probe was called".to_string(),
                ))
            })
        }
    }

    impl crate::memory::ConversationStore for LegacyConversationProbe {
        fn create_conversation<'a>(
            &'a self,
            _conv: crate::memory::NewConversation,
        ) -> futures::future::BoxFuture<'a, crate::error::Result<crate::memory::Conversation>>
        {
            self.unexpected()
        }

        fn get_conversation<'a>(
            &'a self,
            _conversation_id: &'a str,
        ) -> futures::future::BoxFuture<'a, crate::error::Result<Option<crate::memory::Conversation>>>
        {
            self.unexpected()
        }

        fn list_conversations<'a>(
            &'a self,
            _filter: crate::memory::ConversationFilter,
        ) -> futures::future::BoxFuture<
            'a,
            crate::error::Result<Vec<crate::memory::ConversationMeta>>,
        > {
            self.unexpected()
        }

        fn update_conversation<'a>(
            &'a self,
            _conversation_id: &'a str,
            _title: Option<&'a str>,
            _summary: Option<&'a str>,
            _compressed_before_id: Option<i64>,
        ) -> futures::future::BoxFuture<'a, crate::error::Result<()>> {
            self.unexpected()
        }

        fn delete_conversation<'a>(
            &'a self,
            _conversation_id: &'a str,
        ) -> futures::future::BoxFuture<'a, crate::error::Result<()>> {
            self.unexpected()
        }

        fn save_messages<'a>(
            &'a self,
            _conversation_id: &'a str,
            _messages: &'a [crate::memory::StoredMessage],
        ) -> futures::future::BoxFuture<'a, crate::error::Result<()>> {
            self.unexpected()
        }

        fn get_messages<'a>(
            &'a self,
            _conversation_id: &'a str,
        ) -> futures::future::BoxFuture<'a, crate::error::Result<Vec<crate::memory::StoredMessage>>>
        {
            self.unexpected()
        }

        fn count_messages<'a>(
            &'a self,
            _conversation_id: &'a str,
        ) -> futures::future::BoxFuture<'a, crate::error::Result<usize>> {
            self.unexpected()
        }
    }

    #[derive(Default)]
    struct LegacyRuntimeProbe {
        calls: std::sync::atomic::AtomicUsize,
    }

    impl LegacyRuntimeProbe {
        fn unexpected<T: Send + 'static>(
            &self,
        ) -> futures::future::BoxFuture<'static, crate::error::Result<T>> {
            self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Box::pin(async {
                Err(crate::error::ReactError::Other(
                    "legacy runtime probe was called".to_string(),
                ))
            })
        }
    }

    impl RuntimeStateStore for LegacyRuntimeProbe {
        fn get_checkpoint<'a>(
            &'a self,
            _conversation_id: &'a str,
        ) -> futures::future::BoxFuture<'a, crate::error::Result<Option<AgentCheckpoint>>> {
            self.unexpected()
        }

        fn save_checkpoint<'a>(
            &'a self,
            _checkpoint: &'a AgentCheckpoint,
        ) -> futures::future::BoxFuture<'a, crate::error::Result<()>> {
            self.unexpected()
        }

        fn save_checkpoint_for_scope<'a>(
            &'a self,
            _scope_id: &'a str,
            _checkpoint: &'a AgentCheckpoint,
        ) -> futures::future::BoxFuture<'a, crate::error::Result<()>> {
            self.unexpected()
        }

        fn runtime_state_ids<'a>(
            &'a self,
            _scope_id: &'a str,
        ) -> futures::future::BoxFuture<'a, crate::error::Result<Vec<String>>> {
            self.unexpected()
        }

        fn clear_runtime_state<'a>(
            &'a self,
            _scope_id: &'a str,
            _runtime_state_id: &'a str,
        ) -> futures::future::BoxFuture<'a, crate::error::Result<RuntimeStateClearReceipt>>
        {
            self.unexpected()
        }

        fn clear_runtime_state_scope<'a>(
            &'a self,
            _scope_id: &'a str,
        ) -> futures::future::BoxFuture<'a, crate::error::Result<RuntimeStateScopeClearReceipt>>
        {
            self.unexpected()
        }

        fn clear_conversation<'a>(
            &'a self,
            _conversation_id: &'a str,
        ) -> futures::future::BoxFuture<'a, crate::error::Result<()>> {
            self.unexpected()
        }
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
        let projected_item = batch.items.first().ok_or_else(|| {
            crate::error::ReactError::Other("prepared batch is empty".to_string())
        })?;
        let projected = vec![TranscriptProjectionMessage {
            ordinal: 0,
            digest: crate::memory::transcript_projection_message_digest(&projected_item.message)?,
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
        assert_ne!(
            pending.batch.items.first().map(|item| item.digest.as_str()),
            pending
                .cursor_after
                .projected
                .first()
                .map(|message| message.digest.as_str())
        );
        let mut checkpoint = AgentCheckpoint::new("conversation-1");
        checkpoint.messages_json = AgentCheckpoint::serialize_managed_payload(
            vec![Message::user("hello".to_string())],
            Some(pending.cursor_before.clone()),
            Some(pending.clone()),
        )?;

        assert!(checkpoint.restore_runtime_payload().is_err());
        let restored = checkpoint.restore_managed_runtime_payload()?;
        assert_eq!(
            restored.pending_transcript_projection,
            Some(pending.clone())
        );

        for (attempt_class, last_error) in [
            (None, Some("missing class".to_string())),
            (
                Some(TranscriptProjectionAttemptClass::TransientNoCommit),
                None,
            ),
            (
                Some(TranscriptProjectionAttemptClass::DeadlineExceeded),
                Some("   ".to_string()),
            ),
        ] {
            let mut corrupt = pending.clone();
            corrupt.last_attempt_class = attempt_class;
            corrupt.last_error = last_error;
            checkpoint.messages_json = AgentCheckpoint::serialize_managed_payload(
                vec![Message::user("hello".to_string())],
                Some(corrupt.cursor_before.clone()),
                Some(corrupt),
            )?;
            assert!(checkpoint.restore_managed_runtime_payload().is_err());
        }
        Ok(())
    }

    #[test]
    fn runtime_retirement_identity_rejects_revision_rebinding() -> crate::error::Result<()> {
        let mut request = RuntimeGenerationRetireRequest::prepare(
            "scope",
            "runtime",
            3,
            RuntimeStateExpectedVersion::Managed { revision: 7 },
        )?;
        request.expected_scope_revision = 4;

        assert!(request.validate().is_err());
        Ok(())
    }

    #[test]
    fn scope_retirement_reuses_managed_delete_identity() -> crate::error::Result<()> {
        let delete = crate::memory::ManagedConversationDelete::prepare("scope", 5)?;
        let request = ScopeRetirementRequest::prepare("scope", 9, &delete)?;

        assert_eq!(request.delete_operation_id, delete.operation_id);
        assert_eq!(request.payload_digest, delete.payload_digest);
        request.validate()?;
        Ok(())
    }

    #[test]
    fn already_completed_retirement_requires_tombstoned_scope() -> crate::error::Result<()> {
        let delete = crate::memory::ManagedConversationDelete::prepare("scope", 5)?;
        let receipt = ScopeRetirementReceipt {
            scope: RuntimeScopeAuthority {
                scope_id: "scope".to_string(),
                conversation_epoch: Some(6),
                revision: 12,
                lifecycle: RuntimeScopeLifecycle::Active,
            },
            manifest: ScopeRetirementManifest {
                delete_operation_id: delete.operation_id.clone(),
                payload_digest: delete.payload_digest.clone(),
                expected_conversation_epoch: delete.expected_epoch,
                items: Vec::new(),
                conversation_delete_receipt: Some(
                    crate::memory::ManagedConversationDeleteReceipt {
                        operation_id: delete.operation_id,
                        payload_digest: delete.payload_digest,
                        deleted_epoch: delete.expected_epoch,
                        retention_floor_epoch: 0,
                        status: crate::memory::ManagedConversationDeleteStatus::AlreadyDeleted,
                    },
                ),
            },
            dropped_operation_ids: Vec::new(),
            retention_floor_epoch: 0,
            status: ScopeRetirementStatus::AlreadyCompleted,
        };

        assert!(receipt.validate().is_err());
        Ok(())
    }

    #[test]
    fn expired_retirement_projects_new_floor_without_mutating_history() -> crate::error::Result<()>
    {
        let delete = crate::memory::ManagedConversationDelete::prepare("scope", 5)?;
        let receipt = ScopeRetirementReceipt {
            scope: RuntimeScopeAuthority {
                scope_id: "scope".to_string(),
                conversation_epoch: Some(5),
                revision: 13,
                lifecycle: RuntimeScopeLifecycle::Tombstoned,
            },
            manifest: ScopeRetirementManifest {
                delete_operation_id: delete.operation_id.clone(),
                payload_digest: delete.payload_digest.clone(),
                expected_conversation_epoch: delete.expected_epoch,
                items: Vec::new(),
                conversation_delete_receipt: Some(
                    crate::memory::ManagedConversationDeleteReceipt {
                        operation_id: delete.operation_id,
                        payload_digest: delete.payload_digest,
                        deleted_epoch: delete.expected_epoch,
                        retention_floor_epoch: 0,
                        status: crate::memory::ManagedConversationDeleteStatus::AlreadyDeleted,
                    },
                ),
            },
            dropped_operation_ids: Vec::new(),
            retention_floor_epoch: 5,
            status: ScopeRetirementStatus::ReceiptExpired,
        };

        receipt.validate()?;
        let mut stale_floor = receipt.clone();
        stale_floor.retention_floor_epoch = 4;
        assert!(stale_floor.validate().is_err());
        let mut active_scope = receipt.clone();
        active_scope.scope.lifecycle = RuntimeScopeLifecycle::Active;
        assert!(active_scope.validate().is_err());
        let mut missing_delete = receipt.clone();
        missing_delete.manifest.conversation_delete_receipt = None;
        assert!(missing_delete.validate().is_err());
        let mut incomplete_manifest = receipt;
        incomplete_manifest
            .manifest
            .items
            .push(ScopeRetirementItem {
                runtime_state_id: "runtime".to_string(),
                state_version: RuntimeStateVersion::Managed { revision: 1 },
                pending_operation_id: None,
                status: ScopeRetirementItemStatus::Pending,
            });
        assert!(incomplete_manifest.validate().is_err());
        Ok(())
    }

    #[test]
    fn in_progress_expired_retirement_keeps_all_generations_pending() -> crate::error::Result<()> {
        let delete = crate::memory::ManagedConversationDelete::prepare("scope", 5)?;
        let receipt = ScopeRetirementReceipt {
            scope: RuntimeScopeAuthority {
                scope_id: "scope".to_string(),
                conversation_epoch: Some(5),
                revision: 9,
                lifecycle: RuntimeScopeLifecycle::Retiring,
            },
            manifest: ScopeRetirementManifest {
                delete_operation_id: delete.operation_id,
                payload_digest: delete.payload_digest,
                expected_conversation_epoch: delete.expected_epoch,
                items: vec![ScopeRetirementItem {
                    runtime_state_id: "runtime".to_string(),
                    state_version: RuntimeStateVersion::Managed { revision: 2 },
                    pending_operation_id: None,
                    status: ScopeRetirementItemStatus::Pending,
                }],
                conversation_delete_receipt: None,
            },
            dropped_operation_ids: Vec::new(),
            retention_floor_epoch: 5,
            status: ScopeRetirementStatus::ReceiptExpired,
        };

        receipt.validate()?;
        let mut mixed_items = receipt.clone();
        if let Some(item) = mixed_items.manifest.items.first_mut() {
            item.status = ScopeRetirementItemStatus::DroppedByDelete;
        }
        assert!(mixed_items.validate().is_err());
        let mut active_scope = receipt.clone();
        active_scope.scope.lifecycle = RuntimeScopeLifecycle::Active;
        assert!(active_scope.validate().is_err());
        let mut tombstoned_without_receipt = receipt;
        tombstoned_without_receipt.scope.lifecycle = RuntimeScopeLifecycle::Tombstoned;
        assert!(tombstoned_without_receipt.validate().is_err());
        Ok(())
    }

    #[tokio::test]
    async fn managed_delete_retry_after_recreate_recovers_original_receipt()
    -> crate::error::Result<()> {
        use crate::memory::{ConversationProjectionLifecycle, ConversationStore};

        let temp = tempfile::tempdir()?;
        let conversations = crate::memory::FileConversationStore::new(temp.path())?;
        let runtime = FileRuntimeStateStore::new(temp.path())?;
        let acquired = conversations
            .ensure_projection_epoch(managed_projection_request("delete-retry", None))
            .await?;
        let (checkpoint, pending) =
            pending_projection_checkpoint("delete-retry", "delete-generation", 1)?;
        let prepared = runtime
            .compare_and_save_checkpoint(RuntimeCheckpointCasRequest {
                scope_id: "delete-retry".to_string(),
                runtime_state_id: "delete-generation".to_string(),
                conversation_epoch: Some(acquired.authority.epoch),
                expected_scope_revision: 0,
                expected_state_version: RuntimeStateExpectedVersion::Absent,
                checkpoint,
            })
            .await?;
        assert_eq!(prepared.status, RuntimeCheckpointCasStatus::Applied);

        let delete = crate::memory::ManagedConversationDelete::prepare(
            "delete-retry",
            acquired.authority.epoch,
        )?;
        assert!(
            delete_persisted_conversation(&conversations, &runtime, "delete-retry")
                .await
                .is_err()
        );
        let deleted =
            delete_persisted_conversation_managed(&conversations, &runtime, delete.clone()).await?;
        let retirement = deleted.retirement.as_ref().ok_or_else(|| {
            crate::error::ReactError::Other(
                "managed delete lost its retirement receipt".to_string(),
            )
        })?;
        assert_eq!(retirement.status, ScopeRetirementStatus::Completed);
        assert_eq!(retirement.manifest.delete_operation_id, delete.operation_id);
        assert_eq!(retirement.manifest.expected_conversation_epoch, 1);
        assert_eq!(
            retirement.dropped_operation_ids,
            vec![pending.batch.operation_id.clone()]
        );
        assert_eq!(deleted.runtime_state_ids, vec!["delete-generation"]);
        let transcript_receipt = retirement
            .manifest
            .conversation_delete_receipt
            .as_ref()
            .ok_or_else(|| {
                crate::error::ReactError::Other(
                    "managed delete lost its transcript receipt".to_string(),
                )
            })?;
        assert_eq!(transcript_receipt.deleted_epoch, 1);
        assert_eq!(transcript_receipt.retention_floor_epoch, 0);
        assert_eq!(
            transcript_receipt.status,
            crate::memory::ManagedConversationDeleteStatus::Deleted
        );

        let recreated = conversations
            .ensure_projection_epoch(managed_projection_request("delete-retry", Some(1)))
            .await?;
        assert_eq!(recreated.authority.epoch, 2);
        assert_eq!(
            recreated.authority.lifecycle,
            ConversationProjectionLifecycle::Live
        );

        let replayed =
            delete_persisted_conversation_managed(&conversations, &runtime, delete).await?;
        assert_eq!(
            replayed.retirement.map(|receipt| receipt.status),
            Some(ScopeRetirementStatus::AlreadyCompleted)
        );
        let current = conversations
            .get_projection_authority("delete-retry")
            .await?
            .ok_or_else(|| {
                crate::error::ReactError::Other(
                    "recreated conversation authority disappeared".to_string(),
                )
            })?;
        assert_eq!(current.epoch, 2);
        assert_eq!(current.lifecycle, ConversationProjectionLifecycle::Live);
        assert!(
            conversations
                .get_conversation("delete-retry")
                .await?
                .is_some()
        );
        Ok(())
    }

    #[tokio::test]
    async fn expired_caller_deadline_has_no_clear_or_delete_side_effects()
    -> crate::error::Result<()> {
        use crate::memory::ConversationStore;

        let temp = tempfile::tempdir()?;
        let conversations = crate::memory::FileConversationStore::new(temp.path())?;
        let runtime = FileRuntimeStateStore::new(temp.path())?;
        let acquired = conversations
            .ensure_projection_epoch(managed_projection_request("expired-managed", None))
            .await?;
        let (checkpoint, _) =
            pending_projection_checkpoint("expired-managed", "expired-generation", 1)?;
        let prepared = runtime
            .compare_and_save_checkpoint(RuntimeCheckpointCasRequest {
                scope_id: "expired-managed".to_string(),
                runtime_state_id: "expired-generation".to_string(),
                conversation_epoch: Some(acquired.authority.epoch),
                expected_scope_revision: 0,
                expected_state_version: RuntimeStateExpectedVersion::Absent,
                checkpoint,
            })
            .await?;
        let before = runtime
            .load_runtime_state("expired-managed", "expired-generation")
            .await?
            .ok_or_else(|| {
                crate::error::ReactError::Other("prepared runtime state is absent".to_string())
            })?;
        let expired = expired_persistence_context();
        assert!(
            clear_persisted_runtime_incarnation_with_context(
                &conversations,
                &runtime,
                "expired-managed",
                "expired-generation",
                expired,
            )
            .await
            .is_err()
        );
        assert!(
            delete_persisted_conversation_managed_with_context(
                &conversations,
                &runtime,
                crate::memory::ManagedConversationDelete::prepare("expired-managed", 1)?,
                expired,
            )
            .await
            .is_err()
        );
        assert!(
            delete_persisted_conversation_with_context(
                &conversations,
                &runtime,
                "expired-managed",
                expired,
            )
            .await
            .is_err()
        );
        let after = runtime
            .load_runtime_state("expired-managed", "expired-generation")
            .await?
            .ok_or_else(|| {
                crate::error::ReactError::Other(
                    "runtime state disappeared after expired call".to_string(),
                )
            })?;
        assert_eq!(after.scope, before.scope);
        assert_eq!(after.version, before.version);
        assert_eq!(
            after
                .checkpoint
                .as_ref()
                .map(|checkpoint| checkpoint.messages_json.as_str()),
            before
                .checkpoint
                .as_ref()
                .map(|checkpoint| checkpoint.messages_json.as_str())
        );
        assert_eq!(
            runtime.load_scope_authority("expired-managed").await?,
            Some(prepared.scope)
        );
        assert!(
            conversations
                .get_conversation("expired-managed")
                .await?
                .is_some()
        );

        let legacy_conversations = LegacyConversationProbe::default();
        let legacy_runtime = LegacyRuntimeProbe::default();
        let legacy_context = crate::memory::PersistenceCallContext::with_timeout(
            std::time::Duration::from_secs(10),
        )?;
        assert!(
            clear_persisted_runtime_incarnation_with_context(
                &legacy_conversations,
                &legacy_runtime,
                "legacy-scope",
                "legacy-runtime",
                legacy_context,
            )
            .await
            .is_err()
        );
        assert!(
            delete_persisted_conversation_with_context(
                &legacy_conversations,
                &legacy_runtime,
                "legacy-scope",
                legacy_context,
            )
            .await
            .is_err()
        );
        assert_eq!(
            legacy_conversations
                .calls
                .load(std::sync::atomic::Ordering::SeqCst),
            0
        );
        assert_eq!(
            legacy_runtime
                .calls
                .load(std::sync::atomic::Ordering::SeqCst),
            0
        );

        Ok(())
    }

    #[tokio::test]
    async fn terminal_failure_without_durable_classification_never_publishes_success()
    -> crate::error::Result<()> {
        use crate::memory::{ConversationStore, TranscriptProjectionErrorClass};

        let temp = tempfile::tempdir()?;
        let conversations = crate::memory::FileConversationStore::new(temp.path())?;
        let runtime = RejectFailureCasRuntimeStore {
            inner: FileRuntimeStateStore::new(temp.path())?,
        };
        let acquired = conversations
            .ensure_projection_epoch(managed_projection_request("failure-cas", None))
            .await?;
        let (checkpoint, _) = pending_projection_checkpoint("failure-cas", "failure-runtime", 1)?;
        let prepared = runtime
            .compare_and_save_checkpoint(RuntimeCheckpointCasRequest {
                scope_id: "failure-cas".to_string(),
                runtime_state_id: "failure-runtime".to_string(),
                conversation_epoch: Some(acquired.authority.epoch),
                expected_scope_revision: 0,
                expected_state_version: RuntimeStateExpectedVersion::Absent,
                checkpoint,
            })
            .await?;
        assert_eq!(prepared.status, RuntimeCheckpointCasStatus::Applied);
        conversations
            .delete_managed_conversation(crate::memory::ManagedConversationDelete::prepare(
                "failure-cas",
                acquired.authority.epoch,
            )?)
            .await?;
        conversations
            .ensure_projection_epoch(managed_projection_request(
                "failure-cas",
                Some(acquired.authority.epoch),
            ))
            .await?;

        for expected_attempt in [2_u32, 3_u32] {
            let context = crate::memory::PersistenceCallContext::with_timeout(
                std::time::Duration::from_secs(10),
            )?;
            let settlement = settle_pending_transcript_projection(
                &conversations,
                &runtime,
                "failure-cas",
                "failure-runtime",
                context,
            )
            .await?;
            assert_eq!(
                settlement.status,
                crate::memory::TranscriptProjectionSettlementStatus::Conflict
            );
            assert_eq!(
                settlement.error_class,
                Some(TranscriptProjectionErrorClass::OutcomeUnknown)
            );
            assert!(
                settlement
                    .detail
                    .is_some_and(|detail| detail.contains("not durable"))
            );
            let durable = runtime
                .load_runtime_state("failure-cas", "failure-runtime")
                .await?
                .ok_or_else(|| {
                    crate::error::ReactError::Other(
                        "runtime pending disappeared after rejected failure CAS".to_string(),
                    )
                })?;
            let payload = durable
                .checkpoint
                .as_ref()
                .ok_or_else(|| {
                    crate::error::ReactError::Other(
                        "runtime pending checkpoint disappeared".to_string(),
                    )
                })?
                .restore_managed_runtime_payload()?;
            let pending = payload.pending_transcript_projection.ok_or_else(|| {
                crate::error::ReactError::Other(
                    "runtime pending debt was cleared without acknowledgement".to_string(),
                )
            })?;
            assert_eq!(pending.attempt, expected_attempt);
            assert!(pending.last_attempt_class.is_none());
            assert!(pending.last_error.is_none());
        }

        let durable = runtime
            .load_runtime_state("failure-cas", "failure-runtime")
            .await?
            .ok_or_else(|| {
                crate::error::ReactError::Other("runtime pending disappeared".to_string())
            })?;
        let payload = durable
            .checkpoint
            .as_ref()
            .ok_or_else(|| {
                crate::error::ReactError::Other(
                    "runtime pending checkpoint disappeared".to_string(),
                )
            })?
            .restore_managed_runtime_payload()?;
        let pending = payload.pending_transcript_projection.ok_or_else(|| {
            crate::error::ReactError::Other("runtime pending debt disappeared".to_string())
        })?;
        let blocked = recorded_transcript_failure_settlement(
            &runtime,
            &durable,
            &payload.messages,
            &pending,
            crate::memory::TranscriptProjectionSettlementStatus::Blocked,
            TranscriptProjectionErrorClass::Unsupported,
            "injected unsupported apply",
            PersistenceOperationDeadline::new()?,
        )
        .await;
        assert_eq!(
            blocked.status,
            crate::memory::TranscriptProjectionSettlementStatus::Blocked
        );
        assert_eq!(
            blocked.error_class,
            Some(TranscriptProjectionErrorClass::OutcomeUnknown)
        );
        Ok(())
    }

    #[tokio::test]
    async fn exact_clear_settles_pending_projection_before_retirement() -> crate::error::Result<()>
    {
        use crate::memory::ConversationStore;

        let temp = tempfile::tempdir()?;
        let conversations = crate::memory::FileConversationStore::new(temp.path())?;
        let runtime = FileRuntimeStateStore::new(temp.path())?;
        let acquired = conversations
            .ensure_projection_epoch(managed_projection_request("clear-pending", None))
            .await?;
        let (checkpoint, pending) =
            pending_projection_checkpoint("clear-pending", "clear-pending", 1)?;
        let operation_id = pending.batch.operation_id.clone();
        let prepared = runtime
            .compare_and_save_checkpoint(RuntimeCheckpointCasRequest {
                scope_id: "clear-pending".to_string(),
                runtime_state_id: "clear-pending".to_string(),
                conversation_epoch: Some(acquired.authority.epoch),
                expected_scope_revision: 0,
                expected_state_version: RuntimeStateExpectedVersion::Absent,
                checkpoint,
            })
            .await?;
        assert_eq!(prepared.status, RuntimeCheckpointCasStatus::Applied);

        let cleared = clear_persisted_runtime_incarnation(
            &conversations,
            &runtime,
            "clear-pending",
            "clear-pending",
        )
        .await?;
        assert!(cleared.checkpoint_removed);
        let retired = runtime
            .load_runtime_state("clear-pending", "clear-pending")
            .await?
            .ok_or_else(|| {
                crate::error::ReactError::Other("retired runtime tombstone is absent".to_string())
            })?;
        assert!(matches!(
            retired.version,
            RuntimeStateVersion::Retired { .. }
        ));
        assert_eq!(
            conversations
                .apply_transcript_projection(pending.batch)
                .await?
                .operation_id,
            operation_id
        );
        Ok(())
    }

    #[tokio::test]
    async fn exact_clear_retry_does_not_delete_recreated_incarnation_transcript()
    -> crate::error::Result<()> {
        use crate::memory::{ConversationProjectionLifecycle, ConversationStore};

        let temp = tempfile::tempdir()?;
        let conversations = crate::memory::FileConversationStore::new(temp.path())?;
        let runtime = FileRuntimeStateStore::new(temp.path())?;
        let scope = conversations
            .ensure_projection_epoch(managed_projection_request("clear-scope", None))
            .await?;
        let incarnation = conversations
            .ensure_projection_epoch(managed_projection_request("clear-generation", None))
            .await?;
        let (checkpoint, _) = pending_projection_checkpoint(
            "clear-scope",
            "clear-generation",
            scope.authority.epoch,
        )?;
        let prepared = runtime
            .compare_and_save_checkpoint(RuntimeCheckpointCasRequest {
                scope_id: "clear-scope".to_string(),
                runtime_state_id: "clear-generation".to_string(),
                conversation_epoch: Some(scope.authority.epoch),
                expected_scope_revision: 0,
                expected_state_version: RuntimeStateExpectedVersion::Absent,
                checkpoint,
            })
            .await?;
        assert_eq!(prepared.status, RuntimeCheckpointCasStatus::Applied);

        let first = clear_persisted_runtime_incarnation(
            &conversations,
            &runtime,
            "clear-scope",
            "clear-generation",
        )
        .await?;
        assert!(first.checkpoint_removed);
        assert!(
            conversations
                .get_conversation("clear-generation")
                .await?
                .is_some()
        );

        let incarnation_delete = crate::memory::ManagedConversationDelete::prepare(
            "clear-generation",
            incarnation.authority.epoch,
        )?;
        assert_eq!(
            conversations
                .delete_managed_conversation(incarnation_delete)
                .await?
                .status,
            crate::memory::ManagedConversationDeleteStatus::Deleted
        );
        let recreated = conversations
            .ensure_projection_epoch(managed_projection_request(
                "clear-generation",
                Some(incarnation.authority.epoch),
            ))
            .await?;
        assert_eq!(recreated.authority.epoch, 2);

        let replayed = clear_persisted_runtime_incarnation(
            &conversations,
            &runtime,
            "clear-scope",
            "clear-generation",
        )
        .await?;
        assert!(!replayed.checkpoint_removed);
        let current = conversations
            .get_projection_authority("clear-generation")
            .await?
            .ok_or_else(|| {
                crate::error::ReactError::Other(
                    "recreated incarnation transcript disappeared".to_string(),
                )
            })?;
        assert_eq!(current.epoch, 2);
        assert_eq!(current.lifecycle, ConversationProjectionLifecycle::Live);
        assert!(
            conversations
                .get_conversation("clear-generation")
                .await?
                .is_some()
        );
        Ok(())
    }
}
