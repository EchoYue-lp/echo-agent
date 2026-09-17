//! File-backed [`RuntimeStateStore`] — a no-dependency JSON-file backend.
//!
//! One record per runtime identity under
//! `<base>/runtime_state/_runtime_owners/<safe_id>.json`. The record owns the
//! stable scope binding, lifecycle phase, and checkpoint atomically. The
//! `_scope_index` files are rebuildable projections, never deletion authority.
//!
//! This is the no-SQLite alternative to `SqliteRuntimeStateStore` (feature
//! `sqlite`). Suitable for a single-process local agent (typical
//! echo-agent consumer). For multi-process concurrency, use the SQLite backend.
//!
//! ## Robustness
//!
//! - **Path-safe ids.** Conversation ids are sanitized before joining into the
//!   path (rejecting `/`, `\`, `..`, empty) to prevent directory escapes.
//! - **Corrupt JSON is an error.** A malformed owner record surfaces as a
//!   typed runtime-state serialization error rather than silently returning
//!   `None`.
//! - **Unique temp names.** Each atomic write uses a uuid-suffixed temp file
//!   (no cross-write collisions; multi-process belt-and-suspenders).
//!
//! Atomic writes use tmp + fsync + rename so a crash never leaves a
//! half-written file.

use echo_core::error::ReactError;
use echo_core::utils::blocking::{
    BlockingFileOperationKey, BlockingFileOperationScope, run_keyed_file_operation,
};
use echo_core::utils::fs::{
    ExclusiveFileLease, create_dir_all_durable, remove_file_durable, try_exclusive_file_lease,
};
use futures::future::BoxFuture;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock, Weak};

use super::{
    AgentCheckpoint, ManagedRuntimeStateSnapshot, RuntimeCheckpointCasReceipt,
    RuntimeCheckpointCasRequest, RuntimeCheckpointCasStatus, RuntimeGenerationRetireReceipt,
    RuntimeGenerationRetireRequest, RuntimeGenerationRetireStatus, RuntimeScopeAuthority,
    RuntimeScopeLifecycle, RuntimeStateCapability, RuntimeStateClearReceipt,
    RuntimeStateExpectedVersion, RuntimeStateScopeClearReceipt, RuntimeStateStore,
    RuntimeStateVersion, RuntimeTranscriptAckRequest, ScopeRetirementAdvance, ScopeRetirementItem,
    ScopeRetirementItemStatus, ScopeRetirementManifest, ScopeRetirementReceipt,
    ScopeRetirementRequest, ScopeRetirementStatus,
};

const SCOPE_INDEX_VERSION: u8 = 1;
const LEGACY_RUNTIME_STATE_RECORD_VERSION: u8 = 1;
const RUNTIME_STATE_RECORD_VERSION: u8 = 2;
const RUNTIME_SCOPE_RECORD_VERSION: u8 = 1;
const RUNTIME_STATE_SHARDS: usize = 64;
const MAX_CONVERSATION_EPOCH: u64 = i64::MAX as u64;

#[derive(Debug, Serialize, Deserialize)]
struct RuntimeStateScopeIndex {
    version: u8,
    scope_id: String,
    runtime_state_ids: BTreeSet<String>,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum RuntimeStatePhase {
    #[default]
    Active,
    Deleting,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct RuntimeStateOwner {
    version: u8,
    runtime_state_id: String,
    scope_id: String,
    #[serde(default)]
    phase: RuntimeStatePhase,
    checkpoint: Option<AgentCheckpoint>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    state_version: Option<RuntimeStateVersion>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    conversation_epoch: Option<u64>,
    #[serde(default)]
    scope_revision: u64,
    #[serde(default = "active_scope_lifecycle")]
    scope_lifecycle: RuntimeScopeLifecycle,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    cas_expected_state_version: Option<RuntimeStateExpectedVersion>,
    #[serde(default)]
    cas_expected_scope_revision: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    transcript_ack_proof: Option<RuntimeTranscriptAckProof>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
struct RuntimeTranscriptAckProof {
    operation_id: String,
    payload_digest: String,
}

fn active_scope_lifecycle() -> RuntimeScopeLifecycle {
    RuntimeScopeLifecycle::Active
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct RuntimeScopeOwner {
    version: u8,
    authority: RuntimeScopeAuthority,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    current_retirement: Option<ScopeRetirementManifest>,
    #[serde(default)]
    completed_retirements: Vec<ScopeRetirementReceipt>,
}

/// File-backed runtime state store.
///
/// [`Self::new`] performs synchronous directory bootstrap. Construct the store
/// before entering latency-sensitive async work, or call it from a blocking
/// setup task. The [`RuntimeStateStore`] methods offload their file operations.
struct FileRuntimeStateAuthority {
    scope_shards: [Mutex<()>; RUNTIME_STATE_SHARDS],
    shards: [Mutex<()>; RUNTIME_STATE_SHARDS],
    _lease: ExclusiveFileLease,
}

impl FileRuntimeStateAuthority {
    fn new(lease: ExclusiveFileLease) -> Self {
        Self {
            scope_shards: std::array::from_fn(|_| Mutex::new(())),
            shards: std::array::from_fn(|_| Mutex::new(())),
            _lease: lease,
        }
    }
}

fn runtime_state_authorities() -> &'static Mutex<HashMap<PathBuf, Weak<FileRuntimeStateAuthority>>>
{
    static REGISTRY: OnceLock<Mutex<HashMap<PathBuf, Weak<FileRuntimeStateAuthority>>>> =
        OnceLock::new();
    REGISTRY.get_or_init(|| Mutex::new(HashMap::new()))
}

#[derive(Clone)]
pub struct FileRuntimeStateStore {
    base: PathBuf,
    authority: Arc<FileRuntimeStateAuthority>,
    call_context: Option<crate::memory::PersistenceCallContext>,
}

impl FileRuntimeStateStore {
    /// Create a file-backed state store rooted at `base/runtime_state/`.
    ///
    /// This synchronous bootstrap creates and canonicalizes the directory.
    pub fn new(base: impl AsRef<Path>) -> Result<Self, ReactError> {
        let base = base.as_ref().join("runtime_state");
        create_dir_all_durable(&base)
            .map_err(|error| Self::to_react_err(format!("create runtime_state dir: {error}")))?;
        create_dir_all_durable(&base.join("_runtime_owners"))
            .map_err(|error| Self::to_react_err(format!("create runtime owners dir: {error}")))?;
        create_dir_all_durable(&base.join("_scope_index"))
            .map_err(|error| Self::to_react_err(format!("create runtime scope dir: {error}")))?;
        create_dir_all_durable(&base.join("_scope_owners")).map_err(|error| {
            Self::to_react_err(format!("create runtime scope owners dir: {error}"))
        })?;
        let base = std::fs::canonicalize(&base).map_err(|error| {
            Self::to_react_err(format!("canonicalize runtime_state dir: {error}"))
        })?;
        let mut registry = runtime_state_authorities()
            .lock()
            .map_err(|error| Self::to_react_err(format!("runtime authority poisoned: {error}")))?;
        let authority = match registry.get(&base).and_then(Weak::upgrade) {
            Some(authority) => authority,
            None => {
                let lease = try_exclusive_file_lease(&base).map_err(Self::to_react_err)?;
                let authority = Arc::new(FileRuntimeStateAuthority::new(lease));
                registry.insert(base.clone(), Arc::downgrade(&authority));
                authority
            }
        };
        Ok(Self {
            base,
            authority,
            call_context: None,
        })
    }

    #[cfg(test)]
    fn checkpoint_path(&self, conversation_id: &str) -> Result<PathBuf, ReactError> {
        self.runtime_owner_path(conversation_id)
    }

    fn scope_index_path(&self, scope_id: &str) -> Result<PathBuf, ReactError> {
        let safe = safe_segment(scope_id)?;
        Ok(self.base.join("_scope_index").join(format!("{safe}.json")))
    }

    fn scope_owner_path(&self, scope_id: &str) -> Result<PathBuf, ReactError> {
        let safe = safe_segment(scope_id)?;
        Ok(self.base.join("_scope_owners").join(format!("{safe}.json")))
    }

    fn runtime_owner_path(&self, runtime_state_id: &str) -> Result<PathBuf, ReactError> {
        let safe = safe_segment(runtime_state_id)?;
        Ok(self
            .base
            .join("_runtime_owners")
            .join(format!("{safe}.json")))
    }

    fn shard(identity: &str) -> usize {
        let hash = identity.as_bytes().iter().fold(0_u64, |hash, byte| {
            hash.wrapping_mul(1099511628211)
                .wrapping_add(u64::from(*byte))
        });
        usize::try_from(hash % u64::try_from(RUNTIME_STATE_SHARDS).unwrap_or(1)).unwrap_or(0)
    }

    fn lock_scope(&self, scope_id: &str) -> crate::error::Result<std::sync::MutexGuard<'_, ()>> {
        self.authority
            .scope_shards
            .get(Self::shard(scope_id))
            .ok_or_else(|| Self::to_react_err("scope shard index is out of bounds"))?
            .lock()
            .map_err(|error| Self::to_react_err(format!("scope shard poisoned: {error}")))
    }

    fn lock_runtime(
        &self,
        runtime_state_id: &str,
    ) -> crate::error::Result<std::sync::MutexGuard<'_, ()>> {
        self.authority
            .shards
            .get(Self::shard(runtime_state_id))
            .ok_or_else(|| Self::to_react_err("runtime shard index is out of bounds"))?
            .lock()
            .map_err(|error| Self::to_react_err(format!("runtime shard poisoned: {error}")))
    }

    fn lock_all_runtime_shards(&self) -> crate::error::Result<Vec<std::sync::MutexGuard<'_, ()>>> {
        let mut guards = Vec::with_capacity(RUNTIME_STATE_SHARDS);
        for shard in &self.authority.shards {
            guards.push(
                shard.lock().map_err(|error| {
                    Self::to_react_err(format!("runtime shard poisoned: {error}"))
                })?,
            );
        }
        Ok(guards)
    }

    fn to_react_err(e: impl std::fmt::Display) -> ReactError {
        echo_core::error::RuntimeStateError::Io(format!("FileRuntimeStateStore: {e}")).into()
    }

    fn invalid_state(message: impl Into<String>) -> ReactError {
        echo_core::error::RuntimeStateError::SerializationError(message.into()).into()
    }

    fn managed_state_error(message: impl Into<String>) -> ReactError {
        echo_core::error::RuntimeStateError::ManagedStateRequiresCas(message.into()).into()
    }

    fn revision_exhausted(message: impl Into<String>) -> ReactError {
        echo_core::error::RuntimeStateError::RevisionExhausted(message.into()).into()
    }

    fn with_call_context(&self, context: crate::memory::PersistenceCallContext) -> Self {
        let mut store = self.clone();
        store.call_context = Some(context);
        store
    }

    fn ensure_call_deadline(
        context: crate::memory::PersistenceCallContext,
        operation: &str,
    ) -> crate::error::Result<()> {
        context.ensure_not_expired().map_err(|error| match error {
            echo_core::error::MemoryError::DeadlineExceeded(_) => {
                echo_core::error::RuntimeStateError::DeadlineExceeded(operation.to_string()).into()
            }
            other => Self::invalid_state(format!(
                "invalid persistence deadline for {operation}: {other}"
            )),
        })
    }

    fn ensure_active_deadline(&self, operation: &str) -> crate::error::Result<()> {
        self.call_context.map_or(Ok(()), |context| {
            Self::ensure_call_deadline(context, operation)
        })
    }

    fn next_revision(current: u64, identity: &str) -> crate::error::Result<u64> {
        current
            .checked_add(1)
            .ok_or_else(|| Self::revision_exhausted(identity.to_string()))
    }

    fn checkpoint_digest(checkpoint: &AgentCheckpoint) -> crate::error::Result<String> {
        let encoded = serde_json::to_vec(checkpoint).map_err(|error| {
            echo_core::error::RuntimeStateError::SerializationError(format!(
                "failed to serialize checkpoint digest: {error}"
            ))
        })?;
        Ok(format!("{:x}", Sha256::digest(encoded)))
    }

    fn serialized_eq<T: Serialize>(left: &T, right: &T) -> crate::error::Result<bool> {
        let left = serde_json::to_vec(left).map_err(|error| {
            Self::invalid_state(format!("failed to serialize runtime payload: {error}"))
        })?;
        let right = serde_json::to_vec(right).map_err(|error| {
            Self::invalid_state(format!("failed to serialize runtime payload: {error}"))
        })?;
        Ok(left == right)
    }

    fn validate_unmanaged_checkpoint(checkpoint: &AgentCheckpoint) -> crate::error::Result<()> {
        let payload = checkpoint.restore_managed_runtime_payload()?;
        if payload.pending_transcript_projection.is_some() {
            return Err(Self::managed_state_error(
                "pending checkpoint payload requires compare-and-save",
            ));
        }
        Ok(())
    }

    fn validate_runtime_owner_payload(owner: &RuntimeStateOwner) -> crate::error::Result<()> {
        match owner.state_version.as_ref() {
            None | Some(RuntimeStateVersion::Unmanaged { .. }) => {
                if let Some(checkpoint) = owner.checkpoint.as_ref() {
                    Self::validate_unmanaged_checkpoint(checkpoint)?;
                }
            }
            Some(RuntimeStateVersion::Managed { revision }) => {
                let checkpoint = owner.checkpoint.as_ref().ok_or_else(|| {
                    Self::invalid_state(format!(
                        "managed runtime state {} lost its checkpoint",
                        owner.runtime_state_id
                    ))
                })?;
                let payload = checkpoint.restore_managed_runtime_payload()?;
                if payload
                    .pending_transcript_projection
                    .as_ref()
                    .is_some_and(|pending| pending.base_runtime_revision != *revision)
                {
                    return Err(Self::invalid_state(
                        "pending transcript base revision does not match managed state",
                    ));
                }
            }
            Some(RuntimeStateVersion::Retired { .. }) => {
                if owner.checkpoint.is_some() || owner.transcript_ack_proof.is_some() {
                    return Err(Self::invalid_state(
                        "retired runtime state retained checkpoint or acknowledgement proof",
                    ));
                }
            }
            Some(RuntimeStateVersion::Absent) => {
                return Err(Self::invalid_state(
                    "runtime state persisted an absent version",
                ));
            }
        }
        if let Some(proof) = owner.transcript_ack_proof.as_ref() {
            let payload = owner
                .checkpoint
                .as_ref()
                .ok_or_else(|| {
                    Self::invalid_state("acknowledged runtime state is missing checkpoint")
                })?
                .restore_managed_runtime_payload()?;
            if proof.payload_digest.len() != 64
                || !proof
                    .payload_digest
                    .bytes()
                    .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
                || proof.operation_id
                    != format!("transcript-projection-v1:{}", proof.payload_digest)
                || !matches!(
                    owner.state_version,
                    Some(RuntimeStateVersion::Managed { .. })
                )
                || payload.pending_transcript_projection.is_some()
                || payload.transcript_projection.is_none()
            {
                return Err(Self::invalid_state(
                    "runtime transcript acknowledgement proof is corrupt",
                ));
            }
        }
        Ok(())
    }

    fn owner_state_version(owner: &RuntimeStateOwner) -> crate::error::Result<RuntimeStateVersion> {
        if let Some(version) = owner.state_version.as_ref() {
            return Ok(version.clone());
        }
        let checkpoint = owner.checkpoint.as_ref().ok_or_else(|| {
            Self::invalid_state(format!(
                "legacy runtime state {} is missing its checkpoint",
                owner.runtime_state_id
            ))
        })?;
        Ok(RuntimeStateVersion::Unmanaged {
            digest: Self::checkpoint_digest(checkpoint)?,
        })
    }

    fn state_revision(version: &RuntimeStateVersion) -> u64 {
        match version {
            RuntimeStateVersion::Absent => 0,
            RuntimeStateVersion::Unmanaged { .. } => 0,
            RuntimeStateVersion::Managed { revision }
            | RuntimeStateVersion::Retired { revision, .. } => *revision,
        }
    }

    fn state_matches_expected(
        current: Option<&RuntimeStateVersion>,
        expected: &RuntimeStateExpectedVersion,
    ) -> bool {
        match (current, expected) {
            (None, RuntimeStateExpectedVersion::Absent) => true,
            (
                Some(RuntimeStateVersion::Unmanaged { digest: current }),
                RuntimeStateExpectedVersion::Unmanaged { digest: expected },
            ) => current == expected,
            (
                Some(RuntimeStateVersion::Managed { revision: current }),
                RuntimeStateExpectedVersion::Managed { revision: expected },
            ) => current == expected,
            _ => false,
        }
    }

    fn epoch_is_valid(epoch: Option<u64>) -> bool {
        epoch.is_none_or(|epoch| (1..=MAX_CONVERSATION_EPOCH).contains(&epoch))
    }

    fn validate_checkpoint_cas_request(
        request: &RuntimeCheckpointCasRequest,
    ) -> crate::error::Result<()> {
        let _safe_scope = safe_segment(&request.scope_id)?;
        let _safe_runtime = safe_segment(&request.runtime_state_id)?;
        if request.checkpoint.conversation_id != request.runtime_state_id {
            return Err(Self::invalid_state(
                "checkpoint identity does not match CAS runtime identity",
            ));
        }
        if !Self::epoch_is_valid(request.conversation_epoch) {
            return Err(Self::invalid_state(
                "conversation epoch is outside the supported range",
            ));
        }
        let payload = request.checkpoint.restore_managed_runtime_payload()?;
        if let Some(pending) = payload.pending_transcript_projection.as_ref()
            && (pending.batch.conversation_id != request.scope_id
                || Some(pending.batch.conversation_epoch) != request.conversation_epoch
                || payload.transcript_projection.as_ref() != Some(&pending.cursor_before))
        {
            return Err(Self::invalid_state(
                "pending transcript projection does not match CAS scope, epoch, and cursor",
            ));
        }
        Ok(())
    }

    fn validate_pending_transition(
        owner: Option<&RuntimeStateOwner>,
        checkpoint: &AgentCheckpoint,
    ) -> crate::error::Result<()> {
        let next = checkpoint.restore_managed_runtime_payload()?;
        let current = match owner {
            Some(owner)
                if matches!(
                    owner.state_version,
                    Some(RuntimeStateVersion::Managed { .. })
                ) =>
            {
                Some(
                    owner
                        .checkpoint
                        .as_ref()
                        .ok_or_else(|| {
                            Self::invalid_state(format!(
                                "managed runtime state {} lost its checkpoint",
                                owner.runtime_state_id
                            ))
                        })?
                        .restore_managed_runtime_payload()?,
                )
            }
            _ => None,
        };
        let current_revision = owner
            .and_then(|owner| owner.state_version.as_ref())
            .map(Self::state_revision)
            .unwrap_or(0);
        let resulting_revision =
            Self::next_revision(current_revision, &checkpoint.conversation_id)?;
        if let (Some(owner), Some(current_pending)) = (
            owner,
            current
                .as_ref()
                .and_then(|payload| payload.pending_transcript_projection.as_ref()),
        ) && current_pending.base_runtime_revision
            != owner
                .state_version
                .as_ref()
                .map(Self::state_revision)
                .unwrap_or(0)
        {
            return Err(Self::invalid_state(
                "current pending transcript revision does not match runtime authority",
            ));
        }
        let messages_match = match current.as_ref() {
            Some(current) => Self::serialized_eq(&current.messages, &next.messages)?,
            None => true,
        };
        match (
            current
                .as_ref()
                .and_then(|payload| payload.pending_transcript_projection.as_ref()),
            next.pending_transcript_projection.as_ref(),
        ) {
            (None, None) => Ok(()),
            (None, Some(next_pending))
                if next_pending.base_runtime_revision == resulting_revision =>
            {
                Ok(())
            }
            (None, Some(_)) => Err(Self::invalid_state(
                "new pending transcript revision does not match resulting runtime revision",
            )),
            (Some(_), None) => Err(Self::managed_state_error(
                "pending transcript projection requires proof-carrying acknowledgement",
            )),
            (Some(current_pending), Some(next_pending))
                if current_pending.batch == next_pending.batch
                    && current_pending.cursor_before == next_pending.cursor_before
                    && current_pending.cursor_after == next_pending.cursor_after
                    && next_pending.base_runtime_revision == resulting_revision
                    && current_pending.prepared_at == next_pending.prepared_at
                    && Self::valid_pending_attempt_transition(current_pending, next_pending)
                    && messages_match
                    && owner.is_some_and(|owner| {
                        owner.checkpoint.as_ref().is_some_and(|current_checkpoint| {
                            current_checkpoint.current_plan == checkpoint.current_plan
                                && current_checkpoint.active_skills == checkpoint.active_skills
                                && current_checkpoint.blocked_reason == checkpoint.blocked_reason
                                && current_checkpoint.working_dir == checkpoint.working_dir
                        })
                    })
                    && current.as_ref().is_some_and(|current| {
                        current.transcript_projection == next.transcript_projection
                    }) =>
            {
                Ok(())
            }
            (Some(_), Some(_)) => Err(Self::managed_state_error(
                "pending transcript projection identity is immutable until acknowledgement",
            )),
        }
    }

    fn valid_pending_attempt_transition(
        current: &super::PendingTranscriptProjection,
        next: &super::PendingTranscriptProjection,
    ) -> bool {
        let next_has_failure = next.last_attempt_class.is_some()
            && next
                .last_error
                .as_deref()
                .is_some_and(|error| !error.trim().is_empty());
        let same_attempt_failure = next.attempt == current.attempt
            && current.last_attempt_class.is_none()
            && current.last_error.is_none()
            && next_has_failure;
        let retry_dispatch = current
            .attempt
            .checked_add(1)
            .is_some_and(|attempt| next.attempt == attempt)
            && next.last_attempt_class.is_none()
            && next.last_error.is_none()
            && current
                .last_attempt_class
                .as_ref()
                .is_none_or(Self::retryable_attempt_class);
        same_attempt_failure || retry_dispatch
    }

    fn retryable_attempt_class(class: &super::TranscriptProjectionAttemptClass) -> bool {
        matches!(
            class,
            super::TranscriptProjectionAttemptClass::OutcomeUnknown
                | super::TranscriptProjectionAttemptClass::TransientNoCommit
                | super::TranscriptProjectionAttemptClass::RevisionConflict
                | super::TranscriptProjectionAttemptClass::DeadlineExceeded
        )
    }

    fn validate_transcript_ack(
        owner: &RuntimeStateOwner,
        request: &RuntimeTranscriptAckRequest,
    ) -> crate::error::Result<()> {
        let checkpoint = owner.checkpoint.as_ref().ok_or_else(|| {
            Self::invalid_state(format!(
                "managed runtime state {} lost its checkpoint",
                owner.runtime_state_id
            ))
        })?;
        let current = checkpoint.restore_managed_runtime_payload()?;
        let pending = current.pending_transcript_projection.ok_or_else(|| {
            Self::managed_state_error(
                "runtime transcript acknowledgement has no current pending projection",
            )
        })?;
        let current_revision = owner
            .state_version
            .as_ref()
            .map(Self::state_revision)
            .unwrap_or(0);
        if pending.base_runtime_revision != current_revision {
            return Err(Self::invalid_state(
                "current pending transcript revision does not match runtime authority",
            ));
        }
        let acknowledged = request.checkpoint.restore_managed_runtime_payload()?;
        let messages_match = Self::serialized_eq(&acknowledged.messages, &current.messages)?;
        if pending.batch.operation_id != request.projection_receipt.operation_id
            || pending.batch.payload_digest != request.projection_receipt.payload_digest
            || pending.batch.conversation_id != request.scope_id
            || pending.batch.generation_id != request.runtime_state_id
            || pending.batch.conversation_epoch != request.conversation_epoch
            || acknowledged.transcript_projection.as_ref() != Some(&pending.cursor_after)
            || !messages_match
            || request.checkpoint.current_plan != checkpoint.current_plan
            || request.checkpoint.active_skills != checkpoint.active_skills
            || request.checkpoint.blocked_reason != checkpoint.blocked_reason
            || request.checkpoint.working_dir != checkpoint.working_dir
        {
            return Err(Self::managed_state_error(
                "runtime transcript acknowledgement does not match current pending projection",
            ));
        }
        Ok(())
    }

    fn is_direct_cas_replay(
        owner: &RuntimeStateOwner,
        request: &RuntimeCheckpointCasRequest,
    ) -> crate::error::Result<bool> {
        Ok(matches!(
            owner.state_version,
            Some(RuntimeStateVersion::Managed { .. })
        ) && owner.cas_expected_state_version.as_ref() == Some(&request.expected_state_version)
            && owner.cas_expected_scope_revision == request.expected_scope_revision
            && request.expected_scope_revision.checked_add(1) == Some(owner.scope_revision)
            && Self::checkpoint_is_current(owner, &request.checkpoint)?)
    }

    fn is_direct_retirement_replay(
        owner: &RuntimeStateOwner,
        request: &RuntimeGenerationRetireRequest,
    ) -> bool {
        matches!(
            owner.state_version.as_ref(),
            Some(RuntimeStateVersion::Retired { operation_id, .. })
                if operation_id == &request.operation_id
        ) && owner.cas_expected_state_version.as_ref() == Some(&request.expected_state_version)
            && owner.cas_expected_scope_revision == request.expected_scope_revision
            && request.expected_scope_revision.checked_add(1) == Some(owner.scope_revision)
    }

    #[cfg(test)]
    fn save_checkpoint_sync(&self, checkpoint: &AgentCheckpoint) -> crate::error::Result<()> {
        self.write_runtime_owner_sync(&RuntimeStateOwner {
            version: LEGACY_RUNTIME_STATE_RECORD_VERSION,
            runtime_state_id: checkpoint.conversation_id.clone(),
            scope_id: checkpoint.conversation_id.clone(),
            phase: RuntimeStatePhase::Active,
            checkpoint: Some(checkpoint.clone()),
            state_version: None,
            conversation_epoch: None,
            scope_revision: 0,
            scope_lifecycle: RuntimeScopeLifecycle::Active,
            cas_expected_state_version: None,
            cas_expected_scope_revision: 0,
            transcript_ack_proof: None,
        })
    }

    fn write_scope_index_sync(&self, index: &RuntimeStateScopeIndex) -> crate::error::Result<()> {
        let path = self.scope_index_path(&index.scope_id)?;
        if index.runtime_state_ids.is_empty() {
            return remove_file_durable(&path)
                .map(|_removed| ())
                .map_err(Self::to_react_err);
        }
        let parent = path
            .parent()
            .ok_or_else(|| Self::invalid_state("runtime state scope index has no parent"))?;
        create_dir_all_durable(parent).map_err(Self::to_react_err)?;
        let raw = serde_json::to_vec_pretty(index)
            .map_err(|error| Self::invalid_state(format!("serialize scope index: {error}")))?;
        echo_core::utils::fs::atomic_write(&path, &raw).map_err(Self::to_react_err)
    }

    fn read_scope_owner_sync(
        &self,
        scope_id: &str,
    ) -> crate::error::Result<Option<RuntimeScopeOwner>> {
        let path = self.scope_owner_path(scope_id)?;
        let raw = match std::fs::read_to_string(&path) {
            Ok(raw) => raw,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(Self::to_react_err(error)),
        };
        let owner: RuntimeScopeOwner = serde_json::from_str(&raw)
            .map_err(|error| Self::invalid_state(format!("parse {}: {error}", path.display())))?;
        if owner.version != RUNTIME_SCOPE_RECORD_VERSION || owner.authority.scope_id != scope_id {
            return Err(Self::invalid_state(format!(
                "runtime scope owner identity mismatch at {}",
                path.display()
            )));
        }
        if owner.authority.revision == 0 {
            return Err(Self::invalid_state(format!(
                "managed runtime scope has zero revision at {}",
                path.display()
            )));
        }
        if !Self::epoch_is_valid(owner.authority.conversation_epoch) {
            return Err(Self::invalid_state(format!(
                "runtime scope has an invalid conversation epoch at {}",
                path.display()
            )));
        }
        Self::validate_scope_owner(&owner)?;
        self.validate_retirement_tombstones_sync(&owner)?;
        Ok(Some(owner))
    }

    fn write_scope_owner_sync(&self, owner: &RuntimeScopeOwner) -> crate::error::Result<()> {
        Self::validate_scope_owner(owner)?;
        self.validate_retirement_tombstones_sync(owner)?;
        let path = self.scope_owner_path(&owner.authority.scope_id)?;
        let parent = path.parent().ok_or_else(|| {
            Self::invalid_state("runtime scope owner path has no parent directory")
        })?;
        create_dir_all_durable(parent).map_err(Self::to_react_err)?;
        let raw = serde_json::to_vec_pretty(owner)
            .map_err(|error| Self::invalid_state(format!("serialize runtime scope: {error}")))?;
        echo_core::utils::fs::atomic_write(&path, &raw).map_err(Self::to_react_err)
    }

    fn validate_scope_owner(owner: &RuntimeScopeOwner) -> crate::error::Result<()> {
        match (owner.authority.lifecycle, owner.current_retirement.as_ref()) {
            (RuntimeScopeLifecycle::Retiring, Some(manifest)) => {
                if owner.authority.conversation_epoch != Some(manifest.expected_conversation_epoch)
                {
                    return Err(Self::invalid_state(
                        "retiring runtime scope manifest epoch does not match its authority",
                    ));
                }
                manifest.validate(&owner.authority.scope_id)?;
            }
            (RuntimeScopeLifecycle::Retiring, None) => {
                return Err(Self::invalid_state(
                    "retiring runtime scope is missing its manifest",
                ));
            }
            (RuntimeScopeLifecycle::Active | RuntimeScopeLifecycle::Tombstoned, Some(_)) => {
                return Err(Self::invalid_state(
                    "non-retiring runtime scope must not retain a retirement manifest",
                ));
            }
            (RuntimeScopeLifecycle::Active | RuntimeScopeLifecycle::Tombstoned, None) => {}
        }
        for receipt in &owner.completed_retirements {
            if receipt.scope.scope_id != owner.authority.scope_id {
                return Err(Self::invalid_state(
                    "completed retirement receipt belongs to a different scope",
                ));
            }
            if receipt.status != ScopeRetirementStatus::Completed
                || receipt.scope.lifecycle != RuntimeScopeLifecycle::Tombstoned
                || receipt.scope.conversation_epoch
                    != Some(receipt.manifest.expected_conversation_epoch)
            {
                return Err(Self::invalid_state(
                    "completed retirement history contains a non-completed receipt",
                ));
            }
            receipt.validate()?;
        }
        Ok(())
    }

    fn validate_retirement_tombstones_sync(
        &self,
        owner: &RuntimeScopeOwner,
    ) -> crate::error::Result<()> {
        let manifests = owner.current_retirement.iter().chain(
            owner
                .completed_retirements
                .iter()
                .map(|receipt| &receipt.manifest),
        );
        for manifest in manifests {
            for item in manifest
                .items
                .iter()
                .filter(|item| item.status == ScopeRetirementItemStatus::DroppedByDelete)
            {
                let runtime = self
                    .read_runtime_owner_sync(&item.runtime_state_id)?
                    .ok_or_else(|| {
                        Self::invalid_state(format!(
                            "dropped runtime state {} is missing its retirement tombstone",
                            item.runtime_state_id
                        ))
                    })?;
                if runtime.scope_id != owner.authority.scope_id
                    || runtime.phase != RuntimeStatePhase::Deleting
                    || !matches!(
                        runtime.state_version.as_ref(),
                        Some(RuntimeStateVersion::Retired { operation_id, .. })
                            if operation_id == &manifest.delete_operation_id
                    )
                {
                    return Err(Self::invalid_state(format!(
                        "dropped runtime state {} lacks the matching retirement tombstone",
                        item.runtime_state_id
                    )));
                }
            }
        }
        Ok(())
    }

    fn synthetic_scope_authority(
        scope_id: &str,
        owners: &[RuntimeStateOwner],
    ) -> Option<RuntimeScopeAuthority> {
        owners
            .iter()
            .filter(|owner| owner.scope_id == scope_id)
            .max_by_key(|owner| owner.scope_revision)
            .map(|owner| RuntimeScopeAuthority {
                scope_id: scope_id.to_string(),
                conversation_epoch: owner.conversation_epoch,
                revision: owner.scope_revision,
                lifecycle: owner.scope_lifecycle,
            })
    }

    fn reconcile_scope_owner_sync(
        &self,
        scope_id: &str,
    ) -> crate::error::Result<Option<RuntimeScopeOwner>> {
        let owners = self.runtime_records_sync()?;
        let mut record = self.read_scope_owner_sync(scope_id)?;
        let Some(recovered) = Self::synthetic_scope_authority(scope_id, &owners) else {
            return Ok(record);
        };
        if recovered.revision == 0 {
            return Ok(record);
        }
        match record.as_mut() {
            Some(record) if recovered.revision > record.authority.revision => {
                record.authority.revision = recovered.revision;
                record.authority.conversation_epoch = recovered.conversation_epoch;
                record.authority.lifecycle = recovered.lifecycle;
                if let Some(manifest) = record.current_retirement.as_mut()
                    && recovered.lifecycle == RuntimeScopeLifecycle::Retiring
                {
                    let delete_operation_id = manifest.delete_operation_id.clone();
                    for item in &mut manifest.items {
                        let dropped = owners.iter().any(|owner| {
                            owner.runtime_state_id == item.runtime_state_id
                                && matches!(
                                    owner.state_version.as_ref(),
                                    Some(RuntimeStateVersion::Retired { operation_id, .. })
                                        if operation_id == &delete_operation_id
                                )
                        });
                        if dropped {
                            item.status = ScopeRetirementItemStatus::DroppedByDelete;
                        }
                    }
                }
                self.write_scope_owner_sync(record)?;
            }
            None => {
                let recovered = RuntimeScopeOwner {
                    version: RUNTIME_SCOPE_RECORD_VERSION,
                    authority: recovered,
                    current_retirement: None,
                    completed_retirements: Vec::new(),
                };
                self.write_scope_owner_sync(&recovered)?;
                record = Some(recovered);
            }
            _ => {}
        }
        Ok(record)
    }

    fn read_runtime_owner_sync(
        &self,
        runtime_state_id: &str,
    ) -> crate::error::Result<Option<RuntimeStateOwner>> {
        let path = self.runtime_owner_path(runtime_state_id)?;
        let raw = match std::fs::read_to_string(&path) {
            Ok(raw) => raw,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(Self::to_react_err(error)),
        };
        let owner: RuntimeStateOwner = serde_json::from_str(&raw)
            .map_err(|error| Self::invalid_state(format!("parse {}: {error}", path.display())))?;
        if !matches!(
            owner.version,
            LEGACY_RUNTIME_STATE_RECORD_VERSION | RUNTIME_STATE_RECORD_VERSION
        ) || owner.runtime_state_id != runtime_state_id
        {
            return Err(Self::invalid_state(format!(
                "runtime state owner identity mismatch at {}",
                path.display()
            )));
        }
        let _safe = safe_segment(&owner.scope_id)?;
        if owner.version == LEGACY_RUNTIME_STATE_RECORD_VERSION && owner.state_version.is_some() {
            return Err(Self::invalid_state(format!(
                "legacy runtime state unexpectedly contains managed metadata at {}",
                path.display()
            )));
        }
        if owner.version == LEGACY_RUNTIME_STATE_RECORD_VERSION
            && owner.transcript_ack_proof.is_some()
        {
            return Err(Self::invalid_state(format!(
                "legacy runtime state unexpectedly contains transcript acknowledgement proof at {}",
                path.display()
            )));
        }
        if owner.version == RUNTIME_STATE_RECORD_VERSION
            && (owner.state_version.is_none() || owner.scope_revision == 0)
        {
            return Err(Self::invalid_state(format!(
                "managed runtime state is missing revision metadata at {}",
                path.display()
            )));
        }
        if matches!(owner.state_version, Some(RuntimeStateVersion::Absent)) {
            return Err(Self::invalid_state(format!(
                "runtime state persisted an absent version at {}",
                path.display()
            )));
        }
        if !Self::epoch_is_valid(owner.conversation_epoch) {
            return Err(Self::invalid_state(format!(
                "runtime state has an invalid conversation epoch at {}",
                path.display()
            )));
        }
        if let Some(proof) = owner.transcript_ack_proof.as_ref() {
            let payload = owner
                .checkpoint
                .as_ref()
                .ok_or_else(|| {
                    Self::invalid_state(format!(
                        "acknowledged runtime state is missing checkpoint at {}",
                        path.display()
                    ))
                })?
                .restore_managed_runtime_payload()?;
            if proof.operation_id.trim().is_empty()
                || proof.payload_digest.trim().is_empty()
                || !matches!(
                    owner.state_version,
                    Some(RuntimeStateVersion::Managed { .. })
                )
                || payload.pending_transcript_projection.is_some()
            {
                return Err(Self::invalid_state(format!(
                    "runtime transcript acknowledgement proof is corrupt at {}",
                    path.display()
                )));
            }
        }
        match owner.phase {
            RuntimeStatePhase::Active => {
                if owner
                    .checkpoint
                    .as_ref()
                    .is_none_or(|checkpoint| checkpoint.conversation_id != runtime_state_id)
                {
                    return Err(Self::invalid_state(format!(
                        "active runtime state record is missing checkpoint at {}",
                        path.display()
                    )));
                }
            }
            RuntimeStatePhase::Deleting => {
                if owner.checkpoint.is_some() {
                    return Err(Self::invalid_state(format!(
                        "deleting runtime state record retained checkpoint at {}",
                        path.display()
                    )));
                }
                if owner.version == RUNTIME_STATE_RECORD_VERSION
                    && !matches!(
                        owner.state_version,
                        Some(RuntimeStateVersion::Retired { .. })
                    )
                {
                    return Err(Self::invalid_state(format!(
                        "managed deleting runtime state is missing a retirement tombstone at {}",
                        path.display()
                    )));
                }
            }
        }
        Self::validate_runtime_owner_payload(&owner)?;
        Ok(Some(owner))
    }

    /// Normalize a legacy clear crash-cut while the caller holds this runtime's shard.
    fn read_live_runtime_owner_sync(
        &self,
        runtime_state_id: &str,
    ) -> crate::error::Result<Option<RuntimeStateOwner>> {
        let owner = self.read_runtime_owner_sync(runtime_state_id)?;
        if owner.as_ref().is_some_and(|owner| {
            owner.version == LEGACY_RUNTIME_STATE_RECORD_VERSION
                && owner.phase == RuntimeStatePhase::Deleting
        }) {
            let _removed = self.remove_runtime_record_sync(runtime_state_id)?;
            return Ok(None);
        }
        Ok(owner)
    }

    fn write_runtime_owner_sync(&self, owner: &RuntimeStateOwner) -> crate::error::Result<()> {
        Self::validate_runtime_owner_payload(owner)?;
        let _safe_scope = safe_segment(&owner.scope_id)?;
        let path = self.runtime_owner_path(&owner.runtime_state_id)?;
        let parent = path
            .parent()
            .ok_or_else(|| Self::invalid_state("runtime state owner path has no parent"))?;
        create_dir_all_durable(parent).map_err(Self::to_react_err)?;
        let raw = serde_json::to_vec_pretty(owner)
            .map_err(|error| Self::invalid_state(format!("serialize runtime owner: {error}")))?;
        echo_core::utils::fs::atomic_write(&path, &raw).map_err(Self::to_react_err)
    }

    fn mark_runtime_deleting_sync(
        &self,
        scope_id: &str,
        runtime_state_id: &str,
    ) -> crate::error::Result<bool> {
        let Some(owner) = self.read_runtime_owner_sync(runtime_state_id)? else {
            return Ok(false);
        };
        if owner.scope_id != scope_id {
            return Err(Self::invalid_state(format!(
                "runtime state {runtime_state_id} belongs to scope {}, not {scope_id}",
                owner.scope_id
            )));
        }
        if owner.phase == RuntimeStatePhase::Active {
            self.write_runtime_owner_sync(&RuntimeStateOwner {
                version: owner.version,
                runtime_state_id: runtime_state_id.to_string(),
                scope_id: scope_id.to_string(),
                phase: RuntimeStatePhase::Deleting,
                checkpoint: None,
                state_version: owner.state_version,
                conversation_epoch: owner.conversation_epoch,
                scope_revision: owner.scope_revision,
                scope_lifecycle: owner.scope_lifecycle,
                cas_expected_state_version: owner.cas_expected_state_version,
                cas_expected_scope_revision: owner.cas_expected_scope_revision,
                transcript_ack_proof: owner.transcript_ack_proof,
            })?;
        }
        Ok(true)
    }

    fn remove_runtime_record_sync(&self, runtime_state_id: &str) -> crate::error::Result<bool> {
        remove_file_durable(&self.runtime_owner_path(runtime_state_id)?).map_err(Self::to_react_err)
    }

    fn runtime_records_sync(&self) -> crate::error::Result<Vec<RuntimeStateOwner>> {
        let root = self.base.join("_runtime_owners");
        let mut records = Vec::new();
        for entry in std::fs::read_dir(&root).map_err(Self::to_react_err)? {
            let entry = entry.map_err(Self::to_react_err)?;
            let path = entry.path();
            if path.extension().and_then(|extension| extension.to_str()) != Some("json") {
                continue;
            }
            let raw = std::fs::read_to_string(&path).map_err(Self::to_react_err)?;
            let record: RuntimeStateOwner = serde_json::from_str(&raw).map_err(|error| {
                Self::invalid_state(format!("parse {}: {error}", path.display()))
            })?;
            if self.runtime_owner_path(&record.runtime_state_id)? != path {
                return Err(Self::invalid_state(format!(
                    "runtime state record filename does not match identity: {}",
                    path.display()
                )));
            }
            let validated = self
                .read_runtime_owner_sync(&record.runtime_state_id)?
                .ok_or_else(|| {
                    echo_core::error::RuntimeStateError::NotFound(format!(
                        "runtime state record disappeared: {}",
                        path.display()
                    ))
                })?;
            if validated.version == LEGACY_RUNTIME_STATE_RECORD_VERSION
                && validated.phase == RuntimeStatePhase::Deleting
            {
                continue;
            }
            records.push(validated);
        }
        records.sort_by(|left, right| left.runtime_state_id.cmp(&right.runtime_state_id));
        Ok(records)
    }

    fn scope_runtime_state_ids(scope_id: &str, records: &[RuntimeStateOwner]) -> Vec<String> {
        records
            .iter()
            .filter(|record| record.scope_id == scope_id)
            .map(|record| record.runtime_state_id.clone())
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect()
    }

    fn reconcile_scope_index_sync(
        &self,
        scope_id: &str,
        records: &[RuntimeStateOwner],
    ) -> crate::error::Result<()> {
        let runtime_state_ids = Self::scope_runtime_state_ids(scope_id, records);
        self.write_scope_index_sync(&RuntimeStateScopeIndex {
            version: SCOPE_INDEX_VERSION,
            scope_id: scope_id.to_string(),
            runtime_state_ids: runtime_state_ids.into_iter().collect(),
        })
    }

    fn repair_scope_index_best_effort(&self, scope_id: &str) {
        let repaired = self
            .runtime_records_sync()
            .and_then(|records| self.reconcile_scope_index_sync(scope_id, &records));
        if let Err(error) = repaired {
            tracing::warn!(
                %scope_id,
                %error,
                "runtime scope projection repair failed; owner records remain authoritative"
            );
        }
    }

    fn authority_or_unmanaged(
        scope_id: &str,
        record: Option<&RuntimeScopeOwner>,
        owners: &[RuntimeStateOwner],
    ) -> RuntimeScopeAuthority {
        record
            .map(|record| record.authority.clone())
            .or_else(|| Self::synthetic_scope_authority(scope_id, owners))
            .unwrap_or(RuntimeScopeAuthority {
                scope_id: scope_id.to_string(),
                conversation_epoch: None,
                revision: 0,
                lifecycle: RuntimeScopeLifecycle::Active,
            })
    }

    fn checkpoint_is_current(
        owner: &RuntimeStateOwner,
        checkpoint: &AgentCheckpoint,
    ) -> crate::error::Result<bool> {
        match owner.checkpoint.as_ref() {
            Some(current) => {
                Ok(Self::checkpoint_digest(current)? == Self::checkpoint_digest(checkpoint)?)
            }
            None => Ok(false),
        }
    }

    fn pending_operation_id(owner: &RuntimeStateOwner) -> crate::error::Result<Option<String>> {
        let Some(checkpoint) = owner.checkpoint.as_ref() else {
            return Ok(None);
        };
        Ok(checkpoint
            .restore_managed_runtime_payload()?
            .pending_transcript_projection
            .map(|pending| pending.batch.operation_id))
    }

    fn retirement_receipt(
        scope: RuntimeScopeAuthority,
        manifest: ScopeRetirementManifest,
        status: ScopeRetirementStatus,
    ) -> ScopeRetirementReceipt {
        let mut dropped_operation_ids = manifest
            .items
            .iter()
            .filter(|item| item.status == ScopeRetirementItemStatus::DroppedByDelete)
            .filter_map(|item| item.pending_operation_id.clone())
            .collect::<Vec<_>>();
        dropped_operation_ids.sort();
        dropped_operation_ids.dedup();
        let retention_floor_epoch = manifest
            .conversation_delete_receipt
            .as_ref()
            .map(|receipt| receipt.retention_floor_epoch)
            .unwrap_or(0);
        ScopeRetirementReceipt {
            scope,
            manifest,
            dropped_operation_ids,
            retention_floor_epoch,
            status,
        }
    }

    fn completed_retirement(
        record: &RuntimeScopeOwner,
        delete_operation_id: &str,
    ) -> Option<ScopeRetirementReceipt> {
        let retention_floor_epoch = record
            .completed_retirements
            .iter()
            .map(|receipt| receipt.retention_floor_epoch)
            .chain(
                record
                    .current_retirement
                    .as_ref()
                    .and_then(|manifest| manifest.conversation_delete_receipt.as_ref())
                    .map(|receipt| receipt.retention_floor_epoch),
            )
            .max()
            .unwrap_or(0);
        record
            .completed_retirements
            .iter()
            .find(|receipt| receipt.manifest.delete_operation_id == delete_operation_id)
            .cloned()
            .map(|mut receipt| {
                receipt.retention_floor_epoch = retention_floor_epoch;
                receipt.status =
                    if retention_floor_epoch >= receipt.manifest.expected_conversation_epoch {
                        ScopeRetirementStatus::ReceiptExpired
                    } else {
                        ScopeRetirementStatus::AlreadyCompleted
                    };
                receipt
            })
    }

    fn successful_conversation_delete(
        receipt: &crate::memory::ManagedConversationDeleteReceipt,
    ) -> bool {
        matches!(
            receipt.status,
            crate::memory::ManagedConversationDeleteStatus::Deleted
                | crate::memory::ManagedConversationDeleteStatus::AlreadyDeleted
        ) && receipt.retention_floor_epoch < receipt.deleted_epoch
    }

    fn conversation_delete_failure_status(
        receipt: &crate::memory::ManagedConversationDeleteReceipt,
    ) -> Option<ScopeRetirementStatus> {
        match receipt.status {
            crate::memory::ManagedConversationDeleteStatus::Deleted
            | crate::memory::ManagedConversationDeleteStatus::AlreadyDeleted => None,
            crate::memory::ManagedConversationDeleteStatus::EpochConflict => {
                Some(ScopeRetirementStatus::EpochConflict)
            }
            crate::memory::ManagedConversationDeleteStatus::ReceiptExpired => {
                Some(ScopeRetirementStatus::ReceiptExpired)
            }
            crate::memory::ManagedConversationDeleteStatus::IdentityConflict => {
                Some(ScopeRetirementStatus::IdentityConflict)
            }
        }
    }

    fn same_conversation_delete_effect(
        left: &crate::memory::ManagedConversationDeleteReceipt,
        right: &crate::memory::ManagedConversationDeleteReceipt,
    ) -> bool {
        left.operation_id == right.operation_id
            && left.payload_digest == right.payload_digest
            && left.deleted_epoch == right.deleted_epoch
            && Self::successful_conversation_delete(left)
            && Self::successful_conversation_delete(right)
    }

    fn run_blocking<'a, T, F>(
        &'a self,
        conversation_id: String,
        operation: F,
    ) -> BoxFuture<'a, crate::error::Result<T>>
    where
        T: Send + 'static,
        F: FnOnce(Self, String) -> crate::error::Result<T> + Send + 'static,
    {
        let store = self.clone();
        Box::pin(async move {
            store.ensure_active_deadline("runtime-state entity admission")?;
            let safe = safe_segment(&conversation_id)?;
            let key = BlockingFileOperationKey::new(
                "runtime-state",
                store.base.clone(),
                BlockingFileOperationScope::Entity(safe),
            );
            run_keyed_file_operation(key, move || {
                store.ensure_active_deadline("runtime-state entity durable work")?;
                operation(store, conversation_id)
            })
            .await
            .map_err(Self::to_react_err)?
        })
    }

    fn run_scope_blocking<'a, T, F>(
        &'a self,
        scope_id: String,
        operation: F,
    ) -> BoxFuture<'a, crate::error::Result<T>>
    where
        T: Send + 'static,
        F: FnOnce(Self, String) -> crate::error::Result<T> + Send + 'static,
    {
        let store = self.clone();
        Box::pin(async move {
            store.ensure_active_deadline("runtime-state scope admission")?;
            let safe = safe_segment(&scope_id)?;
            let key = BlockingFileOperationKey::new(
                "runtime-state",
                store.base.clone(),
                BlockingFileOperationScope::Collection(safe),
            );
            run_keyed_file_operation(key, move || {
                store.ensure_active_deadline("runtime-state scope durable work")?;
                operation(store, scope_id)
            })
            .await
            .map_err(Self::to_react_err)?
        })
    }
}

impl RuntimeStateStore for FileRuntimeStateStore {
    fn runtime_state_capability(&self) -> RuntimeStateCapability {
        RuntimeStateCapability::RevisionedV1
    }

    fn persistence_call_capability(&self) -> crate::memory::PersistenceCallCapability {
        crate::memory::PersistenceCallCapability::AbsoluteDeadlineV1
    }

    fn load_runtime_state_with_context<'a>(
        &'a self,
        context: crate::memory::PersistenceCallContext,
        scope_id: &'a str,
        runtime_state_id: &'a str,
    ) -> BoxFuture<'a, crate::error::Result<Option<ManagedRuntimeStateSnapshot>>> {
        if let Err(error) = Self::ensure_call_deadline(context, "runtime-state load") {
            return Box::pin(async move { Err(error) });
        }
        let store = self.with_call_context(context);
        let scope_id = scope_id.to_string();
        let runtime_state_id = runtime_state_id.to_string();
        Box::pin(async move { store.load_runtime_state(&scope_id, &runtime_state_id).await })
    }

    fn load_scope_authority_with_context<'a>(
        &'a self,
        context: crate::memory::PersistenceCallContext,
        scope_id: &'a str,
    ) -> BoxFuture<'a, crate::error::Result<Option<RuntimeScopeAuthority>>> {
        if let Err(error) = Self::ensure_call_deadline(context, "runtime scope load") {
            return Box::pin(async move { Err(error) });
        }
        let store = self.with_call_context(context);
        let scope_id = scope_id.to_string();
        Box::pin(async move { store.load_scope_authority(&scope_id).await })
    }

    fn compare_and_save_checkpoint_with_context<'a>(
        &'a self,
        context: crate::memory::PersistenceCallContext,
        request: RuntimeCheckpointCasRequest,
    ) -> BoxFuture<'a, crate::error::Result<RuntimeCheckpointCasReceipt>> {
        if let Err(error) = Self::ensure_call_deadline(context, "runtime checkpoint CAS") {
            return Box::pin(async move { Err(error) });
        }
        let store = self.with_call_context(context);
        Box::pin(async move { store.compare_and_save_checkpoint(request).await })
    }

    fn acknowledge_transcript_projection_with_context<'a>(
        &'a self,
        context: crate::memory::PersistenceCallContext,
        request: RuntimeTranscriptAckRequest,
    ) -> BoxFuture<'a, crate::error::Result<RuntimeCheckpointCasReceipt>> {
        if let Err(error) =
            Self::ensure_call_deadline(context, "runtime transcript acknowledgement")
        {
            return Box::pin(async move { Err(error) });
        }
        let store = self.with_call_context(context);
        Box::pin(async move { store.acknowledge_transcript_projection(request).await })
    }

    fn retire_runtime_generation_with_context<'a>(
        &'a self,
        context: crate::memory::PersistenceCallContext,
        request: RuntimeGenerationRetireRequest,
    ) -> BoxFuture<'a, crate::error::Result<RuntimeGenerationRetireReceipt>> {
        if let Err(error) = Self::ensure_call_deadline(context, "runtime generation retirement") {
            return Box::pin(async move { Err(error) });
        }
        let store = self.with_call_context(context);
        Box::pin(async move { store.retire_runtime_generation(request).await })
    }

    fn begin_scope_retirement_with_context<'a>(
        &'a self,
        context: crate::memory::PersistenceCallContext,
        request: ScopeRetirementRequest,
    ) -> BoxFuture<'a, crate::error::Result<ScopeRetirementReceipt>> {
        if let Err(error) = Self::ensure_call_deadline(context, "runtime scope retirement begin") {
            return Box::pin(async move { Err(error) });
        }
        let store = self.with_call_context(context);
        Box::pin(async move { store.begin_scope_retirement(request).await })
    }

    fn continue_scope_retirement_with_context<'a>(
        &'a self,
        context: crate::memory::PersistenceCallContext,
        scope_id: &'a str,
        delete_operation_id: &'a str,
        expected_scope_revision: u64,
        advance: ScopeRetirementAdvance,
    ) -> BoxFuture<'a, crate::error::Result<ScopeRetirementReceipt>> {
        if let Err(error) = Self::ensure_call_deadline(context, "runtime scope retirement advance")
        {
            return Box::pin(async move { Err(error) });
        }
        let store = self.with_call_context(context);
        let scope_id = scope_id.to_string();
        let delete_operation_id = delete_operation_id.to_string();
        Box::pin(async move {
            store
                .continue_scope_retirement(
                    &scope_id,
                    &delete_operation_id,
                    expected_scope_revision,
                    advance,
                )
                .await
        })
    }

    fn load_runtime_state<'a>(
        &'a self,
        scope_id: &'a str,
        runtime_state_id: &'a str,
    ) -> BoxFuture<'a, crate::error::Result<Option<ManagedRuntimeStateSnapshot>>> {
        let scope_id = scope_id.to_string();
        let runtime_state_id = runtime_state_id.to_string();
        self.run_blocking(runtime_state_id, move |store, runtime_state_id| {
            let _scope = store.lock_scope(&scope_id)?;
            let _runtime = store.lock_runtime(&runtime_state_id)?;
            store.ensure_active_deadline("runtime-state load after lock")?;
            let scope_record = store.reconcile_scope_owner_sync(&scope_id)?;
            let Some(owner) = store.read_live_runtime_owner_sync(&runtime_state_id)? else {
                return Ok(None);
            };
            if owner.scope_id != scope_id {
                return Err(Self::invalid_state(format!(
                    "runtime state {runtime_state_id} belongs to scope {}, not {scope_id}",
                    owner.scope_id
                )));
            }
            let owners = vec![owner.clone()];
            let scope = Self::authority_or_unmanaged(&scope_id, scope_record.as_ref(), &owners);
            let version = Self::owner_state_version(&owner)?;
            Ok(Some(ManagedRuntimeStateSnapshot {
                scope,
                runtime_state_id,
                version,
                checkpoint: owner.checkpoint,
            }))
        })
    }

    fn load_scope_authority<'a>(
        &'a self,
        scope_id: &'a str,
    ) -> BoxFuture<'a, crate::error::Result<Option<RuntimeScopeAuthority>>> {
        self.run_scope_blocking(scope_id.to_string(), move |store, scope_id| {
            let _scope = store.lock_scope(&scope_id)?;
            store.ensure_active_deadline("runtime scope load after lock")?;
            let record = store.reconcile_scope_owner_sync(&scope_id)?;
            if let Some(record) = record {
                return Ok(Some(record.authority));
            }
            let owners = store.runtime_records_sync()?;
            Ok(Self::synthetic_scope_authority(&scope_id, &owners))
        })
    }

    fn compare_and_save_checkpoint<'a>(
        &'a self,
        request: RuntimeCheckpointCasRequest,
    ) -> BoxFuture<'a, crate::error::Result<RuntimeCheckpointCasReceipt>> {
        if let Err(error) = Self::validate_checkpoint_cas_request(&request) {
            return Box::pin(async move { Err(error) });
        }
        let runtime_state_id = request.runtime_state_id.clone();
        self.run_blocking(runtime_state_id, move |store, runtime_state_id| {
            let _scope = store.lock_scope(&request.scope_id)?;
            let _runtime = store.lock_runtime(&runtime_state_id)?;
            store.ensure_active_deadline("runtime checkpoint CAS after lock")?;
            let mut scope_record = store.reconcile_scope_owner_sync(&request.scope_id)?;
            let owner = store.read_live_runtime_owner_sync(&runtime_state_id)?;
            if let Some(owner) = owner.as_ref()
                && owner.scope_id != request.scope_id
            {
                return Err(Self::invalid_state(format!(
                    "runtime state {runtime_state_id} already belongs to scope {}",
                    owner.scope_id
                )));
            }
            let owners = owner.iter().cloned().collect::<Vec<_>>();
            let scope =
                Self::authority_or_unmanaged(&request.scope_id, scope_record.as_ref(), &owners);
            let current_version = owner.as_ref().map(Self::owner_state_version).transpose()?;
            if let Some(RuntimeStateVersion::Retired { .. }) = current_version.as_ref() {
                return Ok(RuntimeCheckpointCasReceipt {
                    scope,
                    runtime_state_id,
                    version: current_version.ok_or_else(|| {
                        Self::invalid_state("retired runtime state lost its version")
                    })?,
                    status: RuntimeCheckpointCasStatus::GenerationRetired,
                });
            }
            let epoch_matches = scope.conversation_epoch == request.conversation_epoch;
            let can_bind_epoch = scope.revision == 0 && scope.conversation_epoch.is_none();
            let can_reopen = scope.lifecycle == RuntimeScopeLifecycle::Tombstoned
                && request.conversation_epoch.is_some()
                && request.conversation_epoch > scope.conversation_epoch;
            let fenced = match scope.lifecycle {
                RuntimeScopeLifecycle::Active => !epoch_matches && !can_bind_epoch,
                RuntimeScopeLifecycle::Retiring => true,
                RuntimeScopeLifecycle::Tombstoned => !can_reopen,
            };
            if fenced {
                let version = current_version.unwrap_or(RuntimeStateVersion::Absent);
                return Ok(RuntimeCheckpointCasReceipt {
                    scope,
                    runtime_state_id,
                    version,
                    status: RuntimeCheckpointCasStatus::ScopeFenced,
                });
            }
            if let Some(owner) = owner.as_ref()
                && Self::is_direct_cas_replay(owner, &request)?
            {
                return Ok(RuntimeCheckpointCasReceipt {
                    scope,
                    runtime_state_id,
                    version: current_version.ok_or_else(|| {
                        Self::invalid_state("current runtime state lost its version")
                    })?,
                    status: RuntimeCheckpointCasStatus::AlreadyCurrent,
                });
            }
            if scope.revision != request.expected_scope_revision
                || !Self::state_matches_expected(
                    current_version.as_ref(),
                    &request.expected_state_version,
                )
            {
                let version = current_version.unwrap_or(RuntimeStateVersion::Absent);
                return Ok(RuntimeCheckpointCasReceipt {
                    scope,
                    runtime_state_id,
                    version,
                    status: RuntimeCheckpointCasStatus::RevisionConflict,
                });
            }
            Self::validate_pending_transition(owner.as_ref(), &request.checkpoint)?;
            let current_state_revision = current_version
                .as_ref()
                .map(Self::state_revision)
                .unwrap_or(0);
            let state_revision = Self::next_revision(current_state_revision, &runtime_state_id)?;
            let scope_revision = Self::next_revision(scope.revision, &request.scope_id)?;
            let version = RuntimeStateVersion::Managed {
                revision: state_revision,
            };
            let authority = RuntimeScopeAuthority {
                scope_id: request.scope_id.clone(),
                conversation_epoch: request.conversation_epoch,
                revision: scope_revision,
                lifecycle: RuntimeScopeLifecycle::Active,
            };
            store.write_runtime_owner_sync(&RuntimeStateOwner {
                version: RUNTIME_STATE_RECORD_VERSION,
                runtime_state_id: runtime_state_id.clone(),
                scope_id: request.scope_id.clone(),
                phase: RuntimeStatePhase::Active,
                checkpoint: Some(request.checkpoint),
                state_version: Some(version.clone()),
                conversation_epoch: request.conversation_epoch,
                scope_revision,
                scope_lifecycle: RuntimeScopeLifecycle::Active,
                cas_expected_state_version: Some(request.expected_state_version),
                cas_expected_scope_revision: request.expected_scope_revision,
                transcript_ack_proof: None,
            })?;
            let completed_retirements = scope_record
                .take()
                .map(|record| record.completed_retirements)
                .unwrap_or_default();
            store.write_scope_owner_sync(&RuntimeScopeOwner {
                version: RUNTIME_SCOPE_RECORD_VERSION,
                authority: authority.clone(),
                current_retirement: None,
                completed_retirements,
            })?;
            store.repair_scope_index_best_effort(&request.scope_id);
            Ok(RuntimeCheckpointCasReceipt {
                scope: authority,
                runtime_state_id,
                version,
                status: RuntimeCheckpointCasStatus::Applied,
            })
        })
    }

    fn acknowledge_transcript_projection<'a>(
        &'a self,
        request: RuntimeTranscriptAckRequest,
    ) -> BoxFuture<'a, crate::error::Result<RuntimeCheckpointCasReceipt>> {
        if let Err(error) = request.validate() {
            return Box::pin(async move { Err(error) });
        }
        let runtime_state_id = request.runtime_state_id.clone();
        self.run_blocking(runtime_state_id, move |store, runtime_state_id| {
            let _scope = store.lock_scope(&request.scope_id)?;
            let _runtime = store.lock_runtime(&runtime_state_id)?;
            store.ensure_active_deadline("runtime transcript acknowledgement after lock")?;
            let mut scope_record = store.reconcile_scope_owner_sync(&request.scope_id)?;
            let owner = store.read_live_runtime_owner_sync(&runtime_state_id)?;
            if let Some(owner) = owner.as_ref()
                && owner.scope_id != request.scope_id
            {
                return Err(Self::invalid_state(format!(
                    "runtime state {runtime_state_id} belongs to scope {}, not {}",
                    owner.scope_id, request.scope_id
                )));
            }
            let owners = owner.iter().cloned().collect::<Vec<_>>();
            let scope =
                Self::authority_or_unmanaged(&request.scope_id, scope_record.as_ref(), &owners);
            let current_version = owner.as_ref().map(Self::owner_state_version).transpose()?;
            if matches!(current_version, Some(RuntimeStateVersion::Retired { .. })) {
                return Ok(RuntimeCheckpointCasReceipt {
                    scope,
                    runtime_state_id,
                    version: current_version.unwrap_or(RuntimeStateVersion::Absent),
                    status: RuntimeCheckpointCasStatus::GenerationRetired,
                });
            }
            if scope.lifecycle != RuntimeScopeLifecycle::Active
                || scope.conversation_epoch != Some(request.conversation_epoch)
            {
                return Ok(RuntimeCheckpointCasReceipt {
                    scope,
                    runtime_state_id,
                    version: current_version.unwrap_or(RuntimeStateVersion::Absent),
                    status: RuntimeCheckpointCasStatus::ScopeFenced,
                });
            }
            if let Some(owner) = owner.as_ref()
                && owner.cas_expected_state_version.as_ref()
                    == Some(&request.expected_state_version)
                && owner.cas_expected_scope_revision == request.expected_scope_revision
                && request.expected_scope_revision.checked_add(1) == Some(owner.scope_revision)
                && owner.transcript_ack_proof.as_ref()
                    == Some(&RuntimeTranscriptAckProof {
                        operation_id: request.projection_receipt.operation_id.clone(),
                        payload_digest: request.projection_receipt.payload_digest.clone(),
                    })
                && Self::checkpoint_is_current(owner, &request.checkpoint)?
            {
                return Ok(RuntimeCheckpointCasReceipt {
                    scope,
                    runtime_state_id,
                    version: current_version.ok_or_else(|| {
                        Self::invalid_state("acknowledged runtime state lost its version")
                    })?,
                    status: RuntimeCheckpointCasStatus::AlreadyCurrent,
                });
            }
            if scope.revision != request.expected_scope_revision
                || !Self::state_matches_expected(
                    current_version.as_ref(),
                    &request.expected_state_version,
                )
            {
                return Ok(RuntimeCheckpointCasReceipt {
                    scope,
                    runtime_state_id,
                    version: current_version.unwrap_or(RuntimeStateVersion::Absent),
                    status: RuntimeCheckpointCasStatus::RevisionConflict,
                });
            }
            let owner = owner.as_ref().ok_or_else(|| {
                echo_core::error::RuntimeStateError::NotFound(format!(
                    "runtime state {runtime_state_id} is absent"
                ))
            })?;
            Self::validate_transcript_ack(owner, &request)?;
            let state_revision = Self::next_revision(
                current_version
                    .as_ref()
                    .map(Self::state_revision)
                    .unwrap_or(0),
                &runtime_state_id,
            )?;
            let scope_revision = Self::next_revision(scope.revision, &request.scope_id)?;
            let version = RuntimeStateVersion::Managed {
                revision: state_revision,
            };
            let authority = RuntimeScopeAuthority {
                scope_id: request.scope_id.clone(),
                conversation_epoch: Some(request.conversation_epoch),
                revision: scope_revision,
                lifecycle: RuntimeScopeLifecycle::Active,
            };
            store.write_runtime_owner_sync(&RuntimeStateOwner {
                version: RUNTIME_STATE_RECORD_VERSION,
                runtime_state_id: runtime_state_id.clone(),
                scope_id: request.scope_id.clone(),
                phase: RuntimeStatePhase::Active,
                checkpoint: Some(request.checkpoint),
                state_version: Some(version.clone()),
                conversation_epoch: Some(request.conversation_epoch),
                scope_revision,
                scope_lifecycle: RuntimeScopeLifecycle::Active,
                cas_expected_state_version: Some(request.expected_state_version),
                cas_expected_scope_revision: request.expected_scope_revision,
                transcript_ack_proof: Some(RuntimeTranscriptAckProof {
                    operation_id: request.projection_receipt.operation_id,
                    payload_digest: request.projection_receipt.payload_digest,
                }),
            })?;
            let completed_retirements = scope_record
                .take()
                .map(|record| record.completed_retirements)
                .unwrap_or_default();
            store.write_scope_owner_sync(&RuntimeScopeOwner {
                version: RUNTIME_SCOPE_RECORD_VERSION,
                authority: authority.clone(),
                current_retirement: None,
                completed_retirements,
            })?;
            store.repair_scope_index_best_effort(&request.scope_id);
            Ok(RuntimeCheckpointCasReceipt {
                scope: authority,
                runtime_state_id,
                version,
                status: RuntimeCheckpointCasStatus::Applied,
            })
        })
    }

    fn retire_runtime_generation<'a>(
        &'a self,
        request: RuntimeGenerationRetireRequest,
    ) -> BoxFuture<'a, crate::error::Result<RuntimeGenerationRetireReceipt>> {
        if let Err(error) = request.validate() {
            return Box::pin(async move { Err(error) });
        }
        let runtime_state_id = request.runtime_state_id.clone();
        self.run_blocking(runtime_state_id, move |store, runtime_state_id| {
            let _safe_scope = safe_segment(&request.scope_id)?;
            let _scope = store.lock_scope(&request.scope_id)?;
            let _runtime = store.lock_runtime(&runtime_state_id)?;
            store.ensure_active_deadline("runtime generation retirement after lock")?;
            let mut scope_record = store.reconcile_scope_owner_sync(&request.scope_id)?;
            let owner = store.read_live_runtime_owner_sync(&runtime_state_id)?;
            if let Some(owner) = owner.as_ref()
                && owner.scope_id != request.scope_id
            {
                return Err(Self::invalid_state(format!(
                    "runtime state {runtime_state_id} belongs to scope {}",
                    owner.scope_id
                )));
            }
            let owners = owner.iter().cloned().collect::<Vec<_>>();
            let scope =
                Self::authority_or_unmanaged(&request.scope_id, scope_record.as_ref(), &owners);
            let current_version = owner.as_ref().map(Self::owner_state_version).transpose()?;
            if matches!(current_version, Some(RuntimeStateVersion::Retired { .. })) {
                return Ok(RuntimeGenerationRetireReceipt {
                    scope,
                    runtime_state_id,
                    version: current_version.clone().ok_or_else(|| {
                        Self::invalid_state("retired runtime state lost its version")
                    })?,
                    status: if owner
                        .as_ref()
                        .is_some_and(|owner| Self::is_direct_retirement_replay(owner, &request))
                    {
                        RuntimeGenerationRetireStatus::AlreadyRetired
                    } else {
                        RuntimeGenerationRetireStatus::Conflict
                    },
                });
            }
            if scope.lifecycle != RuntimeScopeLifecycle::Active {
                let version = current_version.unwrap_or(RuntimeStateVersion::Absent);
                return Ok(RuntimeGenerationRetireReceipt {
                    scope,
                    runtime_state_id,
                    version,
                    status: RuntimeGenerationRetireStatus::ScopeFenced,
                });
            }
            if let Some(owner) = owner.as_ref()
                && Self::pending_operation_id(owner)?.is_some()
            {
                return Ok(RuntimeGenerationRetireReceipt {
                    scope,
                    runtime_state_id,
                    version: current_version.ok_or_else(|| {
                        Self::invalid_state("pending runtime state lost its version")
                    })?,
                    status: RuntimeGenerationRetireStatus::PendingProjection,
                });
            }
            if scope.revision != request.expected_scope_revision
                || !Self::state_matches_expected(
                    current_version.as_ref(),
                    &request.expected_state_version,
                )
            {
                let version = current_version.unwrap_or(RuntimeStateVersion::Absent);
                return Ok(RuntimeGenerationRetireReceipt {
                    scope,
                    runtime_state_id,
                    version,
                    status: RuntimeGenerationRetireStatus::Conflict,
                });
            }
            let state_revision = Self::next_revision(
                current_version
                    .as_ref()
                    .map(Self::state_revision)
                    .unwrap_or(0),
                &runtime_state_id,
            )?;
            let scope_revision = Self::next_revision(scope.revision, &request.scope_id)?;
            let version = RuntimeStateVersion::Retired {
                revision: state_revision,
                operation_id: request.operation_id,
            };
            store.write_runtime_owner_sync(&RuntimeStateOwner {
                version: RUNTIME_STATE_RECORD_VERSION,
                runtime_state_id: runtime_state_id.clone(),
                scope_id: request.scope_id.clone(),
                phase: RuntimeStatePhase::Deleting,
                checkpoint: None,
                state_version: Some(version.clone()),
                conversation_epoch: scope.conversation_epoch,
                scope_revision,
                scope_lifecycle: RuntimeScopeLifecycle::Active,
                cas_expected_state_version: Some(request.expected_state_version),
                cas_expected_scope_revision: request.expected_scope_revision,
                transcript_ack_proof: None,
            })?;
            let mut record = scope_record.take().unwrap_or(RuntimeScopeOwner {
                version: RUNTIME_SCOPE_RECORD_VERSION,
                authority: scope,
                current_retirement: None,
                completed_retirements: Vec::new(),
            });
            record.authority.revision = scope_revision;
            store.write_scope_owner_sync(&record)?;
            store.repair_scope_index_best_effort(&request.scope_id);
            Ok(RuntimeGenerationRetireReceipt {
                scope: record.authority,
                runtime_state_id,
                version,
                status: RuntimeGenerationRetireStatus::Retired,
            })
        })
    }

    fn begin_scope_retirement<'a>(
        &'a self,
        request: ScopeRetirementRequest,
    ) -> BoxFuture<'a, crate::error::Result<ScopeRetirementReceipt>> {
        if let Err(error) = request.validate() {
            return Box::pin(async move { Err(error) });
        }
        self.run_scope_blocking(request.scope_id.clone(), move |store, scope_id| {
            let _scope = store.lock_scope(&scope_id)?;
            let _runtime_shards = store.lock_all_runtime_shards()?;
            store.ensure_active_deadline("runtime scope retirement begin after lock")?;
            let mut record = store.reconcile_scope_owner_sync(&scope_id)?;
            if let Some(record) = record.as_ref() {
                if let Some(mut receipt) =
                    Self::completed_retirement(record, &request.delete_operation_id)
                {
                    if receipt.manifest.payload_digest != request.payload_digest
                        || receipt.manifest.expected_conversation_epoch
                            != request.expected_conversation_epoch
                    {
                        receipt.status = ScopeRetirementStatus::IdentityConflict;
                    }
                    return Ok(receipt);
                }
                if let Some(manifest) = record.current_retirement.as_ref()
                    && manifest.delete_operation_id == request.delete_operation_id
                {
                    let status = if manifest.payload_digest == request.payload_digest
                        && manifest.expected_conversation_epoch
                            == request.expected_conversation_epoch
                    {
                        ScopeRetirementStatus::InProgress
                    } else {
                        ScopeRetirementStatus::IdentityConflict
                    };
                    return Ok(Self::retirement_receipt(
                        record.authority.clone(),
                        manifest.clone(),
                        status,
                    ));
                }
            }
            let owners = store.runtime_records_sync()?;
            let authority = Self::authority_or_unmanaged(&scope_id, record.as_ref(), &owners);
            if authority.conversation_epoch.is_some()
                && authority.conversation_epoch != Some(request.expected_conversation_epoch)
            {
                return Ok(Self::retirement_receipt(
                    authority,
                    ScopeRetirementManifest {
                        delete_operation_id: request.delete_operation_id,
                        payload_digest: request.payload_digest,
                        expected_conversation_epoch: request.expected_conversation_epoch,
                        items: Vec::new(),
                        conversation_delete_receipt: None,
                    },
                    ScopeRetirementStatus::EpochConflict,
                ));
            }
            if authority.lifecycle != RuntimeScopeLifecycle::Active {
                let status = if authority.lifecycle == RuntimeScopeLifecycle::Retiring {
                    ScopeRetirementStatus::IdentityConflict
                } else {
                    ScopeRetirementStatus::EpochConflict
                };
                return Ok(Self::retirement_receipt(
                    authority,
                    ScopeRetirementManifest {
                        delete_operation_id: request.delete_operation_id,
                        payload_digest: request.payload_digest,
                        expected_conversation_epoch: request.expected_conversation_epoch,
                        items: Vec::new(),
                        conversation_delete_receipt: None,
                    },
                    status,
                ));
            }
            if authority.revision != request.expected_scope_revision {
                return Ok(Self::retirement_receipt(
                    authority,
                    ScopeRetirementManifest {
                        delete_operation_id: request.delete_operation_id,
                        payload_digest: request.payload_digest,
                        expected_conversation_epoch: request.expected_conversation_epoch,
                        items: Vec::new(),
                        conversation_delete_receipt: None,
                    },
                    ScopeRetirementStatus::RevisionConflict,
                ));
            }
            let mut items = Vec::new();
            for owner in owners.iter().filter(|owner| owner.scope_id == scope_id) {
                let state_version = Self::owner_state_version(owner)?;
                if matches!(state_version, RuntimeStateVersion::Retired { .. }) {
                    continue;
                }
                items.push(ScopeRetirementItem {
                    runtime_state_id: owner.runtime_state_id.clone(),
                    state_version,
                    pending_operation_id: Self::pending_operation_id(owner)?,
                    status: ScopeRetirementItemStatus::Pending,
                });
            }
            items.sort_by(|left, right| left.runtime_state_id.cmp(&right.runtime_state_id));
            let manifest = ScopeRetirementManifest {
                delete_operation_id: request.delete_operation_id,
                payload_digest: request.payload_digest,
                expected_conversation_epoch: request.expected_conversation_epoch,
                items,
                conversation_delete_receipt: None,
            };
            let revision = Self::next_revision(authority.revision, &scope_id)?;
            let authority = RuntimeScopeAuthority {
                scope_id: scope_id.clone(),
                conversation_epoch: Some(request.expected_conversation_epoch),
                revision,
                lifecycle: RuntimeScopeLifecycle::Retiring,
            };
            let completed_retirements = record
                .take()
                .map(|record| record.completed_retirements)
                .unwrap_or_default();
            store.write_scope_owner_sync(&RuntimeScopeOwner {
                version: RUNTIME_SCOPE_RECORD_VERSION,
                authority: authority.clone(),
                current_retirement: Some(manifest.clone()),
                completed_retirements,
            })?;
            Ok(Self::retirement_receipt(
                authority,
                manifest,
                ScopeRetirementStatus::Begun,
            ))
        })
    }

    fn continue_scope_retirement<'a>(
        &'a self,
        scope_id: &'a str,
        delete_operation_id: &'a str,
        expected_scope_revision: u64,
        advance: ScopeRetirementAdvance,
    ) -> BoxFuture<'a, crate::error::Result<ScopeRetirementReceipt>> {
        let scope_id = scope_id.to_string();
        let delete_operation_id = delete_operation_id.to_string();
        self.run_scope_blocking(scope_id, move |store, scope_id| {
            let _scope = store.lock_scope(&scope_id)?;
            let _runtime_shards = store.lock_all_runtime_shards()?;
            store.ensure_active_deadline("runtime scope retirement advance after lock")?;
            let mut record = store
                .reconcile_scope_owner_sync(&scope_id)?
                .ok_or_else(|| {
                    echo_core::error::RuntimeStateError::NotFound(format!(
                        "runtime scope {scope_id} is absent"
                    ))
                })?;
            if let Some(receipt) = Self::completed_retirement(&record, &delete_operation_id) {
                return Ok(receipt);
            }
            let Some(mut manifest) = record.current_retirement.take() else {
                return Ok(Self::retirement_receipt(
                    record.authority.clone(),
                    ScopeRetirementManifest {
                        delete_operation_id,
                        payload_digest: String::new(),
                        expected_conversation_epoch: record
                            .authority
                            .conversation_epoch
                            .unwrap_or(0),
                        items: Vec::new(),
                        conversation_delete_receipt: None,
                    },
                    ScopeRetirementStatus::IdentityConflict,
                ));
            };
            if manifest.delete_operation_id != delete_operation_id {
                return Ok(Self::retirement_receipt(
                    record.authority,
                    manifest,
                    ScopeRetirementStatus::IdentityConflict,
                ));
            }
            if let ScopeRetirementAdvance::ConversationDeleted { receipt } = &advance {
                if receipt.operation_id != manifest.delete_operation_id
                    || receipt.payload_digest != manifest.payload_digest
                    || receipt.deleted_epoch != manifest.expected_conversation_epoch
                {
                    return Ok(Self::retirement_receipt(
                        record.authority,
                        manifest,
                        ScopeRetirementStatus::IdentityConflict,
                    ));
                }
                if let Some(status) = Self::conversation_delete_failure_status(receipt) {
                    let mut result = Self::retirement_receipt(
                        record.authority,
                        manifest,
                        status,
                    );
                    if status == ScopeRetirementStatus::ReceiptExpired {
                        result.retention_floor_epoch = receipt.retention_floor_epoch;
                    }
                    return Ok(result);
                }
            }
            if let ScopeRetirementAdvance::ConversationDeleted { receipt } = &advance
                && let Some(current_floor) = manifest
                    .conversation_delete_receipt
                    .as_ref()
                    .filter(|current| Self::same_conversation_delete_effect(current, receipt))
                    .map(|current| current.retention_floor_epoch)
            {
                if receipt.retention_floor_epoch > current_floor {
                    if let Some(current) = manifest.conversation_delete_receipt.as_mut() {
                        current.retention_floor_epoch = receipt.retention_floor_epoch;
                    }
                    record.authority.revision =
                        Self::next_revision(record.authority.revision, &scope_id)?;
                    record.current_retirement = Some(manifest.clone());
                    store.write_scope_owner_sync(&record)?;
                }
                return Ok(Self::retirement_receipt(
                    record.authority,
                    manifest,
                    ScopeRetirementStatus::InProgress,
                ));
            }
            match &advance {
                ScopeRetirementAdvance::ConversationDeleted { .. } => {}
                ScopeRetirementAdvance::GenerationDropped {
                    runtime_state_id,
                    pending_operation_id,
                } if manifest.items.iter().any(|item| {
                    item.runtime_state_id == *runtime_state_id
                        && item.pending_operation_id == *pending_operation_id
                        && item.status == ScopeRetirementItemStatus::DroppedByDelete
                }) => {
                    return Ok(Self::retirement_receipt(
                        record.authority,
                        manifest,
                        ScopeRetirementStatus::InProgress,
                    ));
                }
                _ => {}
            }
            if record.authority.revision != expected_scope_revision {
                return Ok(Self::retirement_receipt(
                    record.authority,
                    manifest,
                    ScopeRetirementStatus::RevisionConflict,
                ));
            }

            let mut changed_runtime: Option<RuntimeStateOwner> = None;
            let completing = matches!(&advance, ScopeRetirementAdvance::Complete);
            let can_complete;
            match advance {
                ScopeRetirementAdvance::ConversationDeleted { receipt } => {
                    if receipt.operation_id != manifest.delete_operation_id
                        || receipt.payload_digest != manifest.payload_digest
                        || receipt.deleted_epoch != manifest.expected_conversation_epoch
                        || !Self::successful_conversation_delete(&receipt)
                    {
                        return Ok(Self::retirement_receipt(
                            record.authority,
                            manifest,
                            ScopeRetirementStatus::IdentityConflict,
                        ));
                    }
                    if let Some(current) = manifest.conversation_delete_receipt.as_ref() {
                        if !Self::same_conversation_delete_effect(current, &receipt) {
                            return Ok(Self::retirement_receipt(
                                record.authority,
                                manifest,
                                ScopeRetirementStatus::IdentityConflict,
                            ));
                        }
                        return Ok(Self::retirement_receipt(
                            record.authority,
                            manifest,
                            ScopeRetirementStatus::InProgress,
                        ));
                    } else {
                        manifest.conversation_delete_receipt = Some(receipt);
                    }
                    can_complete = false;
                }
                ScopeRetirementAdvance::GenerationDropped {
                    runtime_state_id,
                    pending_operation_id,
                } => {
                    if manifest.conversation_delete_receipt.is_none() {
                        return Ok(Self::retirement_receipt(
                            record.authority,
                            manifest,
                            ScopeRetirementStatus::InProgress,
                        ));
                    }
                    let item = manifest
                        .items
                        .iter_mut()
                        .find(|item| item.runtime_state_id == runtime_state_id)
                        .ok_or_else(|| {
                            echo_core::error::RuntimeStateError::NotFound(format!(
                                "runtime state {runtime_state_id} is absent from retirement manifest"
                            ))
                        })?;
                    if item.pending_operation_id != pending_operation_id {
                        return Ok(Self::retirement_receipt(
                            record.authority,
                            manifest,
                            ScopeRetirementStatus::IdentityConflict,
                        ));
                    }
                    if item.status == ScopeRetirementItemStatus::DroppedByDelete {
                        return Ok(Self::retirement_receipt(
                            record.authority,
                            manifest,
                            ScopeRetirementStatus::InProgress,
                        ));
                    } else {
                        let owner = store
                            .read_runtime_owner_sync(&runtime_state_id)?
                            .ok_or_else(|| {
                                echo_core::error::RuntimeStateError::NotFound(format!(
                                    "runtime state {runtime_state_id} disappeared during retirement"
                                ))
                            })?;
                        if owner.scope_id != scope_id
                            || Self::owner_state_version(&owner)? != item.state_version
                        {
                            return Ok(Self::retirement_receipt(
                                record.authority,
                                manifest,
                                ScopeRetirementStatus::IdentityConflict,
                            ));
                        }
                        changed_runtime = Some(RuntimeStateOwner {
                            version: RUNTIME_STATE_RECORD_VERSION,
                            runtime_state_id: runtime_state_id.clone(),
                            scope_id: scope_id.clone(),
                            phase: RuntimeStatePhase::Deleting,
                            checkpoint: None,
                            state_version: Some(RuntimeStateVersion::Retired {
                                revision: Self::next_revision(
                                    Self::state_revision(&item.state_version),
                                    &runtime_state_id,
                                )?,
                                operation_id: delete_operation_id.clone(),
                            }),
                            conversation_epoch: record.authority.conversation_epoch,
                            scope_revision: 0,
                            scope_lifecycle: RuntimeScopeLifecycle::Retiring,
                            cas_expected_state_version: None,
                            cas_expected_scope_revision: 0,
                            transcript_ack_proof: None,
                        });
                        item.status = ScopeRetirementItemStatus::DroppedByDelete;
                    }
                    can_complete = false;
                }
                ScopeRetirementAdvance::Complete => {
                    can_complete = manifest.conversation_delete_receipt.is_some()
                        && manifest.items.iter().all(|item| {
                            item.status == ScopeRetirementItemStatus::DroppedByDelete
                        });
                }
            }
            if completing && !can_complete {
                return Ok(Self::retirement_receipt(
                    record.authority,
                    manifest,
                    ScopeRetirementStatus::InProgress,
                ));
            }
            let revision = Self::next_revision(record.authority.revision, &scope_id)?;
            if let Some(mut owner) = changed_runtime {
                owner.scope_revision = revision;
                store.write_runtime_owner_sync(&owner)?;
            }
            record.authority.revision = revision;
            if can_complete {
                record.authority.lifecycle = RuntimeScopeLifecycle::Tombstoned;
                let completed = Self::retirement_receipt(
                    record.authority.clone(),
                    manifest.clone(),
                    ScopeRetirementStatus::Completed,
                );
                record.completed_retirements.push(completed.clone());
                record.current_retirement = None;
                store.write_scope_owner_sync(&record)?;
                return Ok(completed);
            }
            record.current_retirement = Some(manifest.clone());
            store.write_scope_owner_sync(&record)?;
            Ok(Self::retirement_receipt(
                record.authority,
                manifest,
                ScopeRetirementStatus::InProgress,
            ))
        })
    }

    fn get_checkpoint<'a>(
        &'a self,
        conversation_id: &'a str,
    ) -> BoxFuture<'a, crate::error::Result<Option<AgentCheckpoint>>> {
        self.run_blocking(
            conversation_id.to_string(),
            move |store, conversation_id| {
                let _runtime = store.lock_runtime(&conversation_id)?;
                let Some(record) = store.read_runtime_owner_sync(&conversation_id)? else {
                    return Ok(None);
                };
                match record.phase {
                    RuntimeStatePhase::Active => Ok(record.checkpoint),
                    RuntimeStatePhase::Deleting => {
                        if record.version == LEGACY_RUNTIME_STATE_RECORD_VERSION {
                            let _removed = store.remove_runtime_record_sync(&conversation_id)?;
                        }
                        Ok(None)
                    }
                }
            },
        )
    }

    fn save_checkpoint<'a>(
        &'a self,
        checkpoint: &'a AgentCheckpoint,
    ) -> BoxFuture<'a, crate::error::Result<()>> {
        self.save_checkpoint_for_scope(&checkpoint.conversation_id, checkpoint)
    }

    fn save_checkpoint_for_scope<'a>(
        &'a self,
        scope_id: &'a str,
        checkpoint: &'a AgentCheckpoint,
    ) -> BoxFuture<'a, crate::error::Result<()>> {
        if let Err(error) = Self::validate_unmanaged_checkpoint(checkpoint) {
            return Box::pin(async move { Err(error) });
        }
        let checkpoint = checkpoint.clone();
        let runtime_state_id = checkpoint.conversation_id.clone();
        let scope_id = scope_id.to_string();
        self.run_blocking(runtime_state_id, move |store, runtime_state_id| {
            let _safe_scope = safe_segment(&scope_id)?;
            let _scope = store.lock_scope(&scope_id)?;
            let _runtime = store.lock_runtime(&runtime_state_id)?;
            if store.reconcile_scope_owner_sync(&scope_id)?.is_some() {
                return Err(Self::managed_state_error(format!(
                    "scope {scope_id} is revision managed"
                )));
            }
            if let Some(record) = store.read_runtime_owner_sync(&runtime_state_id)? {
                if record.state_version.is_some() {
                    return Err(Self::managed_state_error(format!(
                        "runtime state {runtime_state_id} is revision managed"
                    )));
                }
                match record.phase {
                    RuntimeStatePhase::Active if record.scope_id != scope_id => {
                        return Err(Self::invalid_state(format!(
                            "runtime state {runtime_state_id} already belongs to scope {}",
                            record.scope_id
                        )));
                    }
                    RuntimeStatePhase::Deleting => {
                        let _removed = store.remove_runtime_record_sync(&runtime_state_id)?;
                    }
                    RuntimeStatePhase::Active => {}
                }
            }
            store.write_runtime_owner_sync(&RuntimeStateOwner {
                version: LEGACY_RUNTIME_STATE_RECORD_VERSION,
                runtime_state_id: runtime_state_id.clone(),
                scope_id: scope_id.clone(),
                phase: RuntimeStatePhase::Active,
                checkpoint: Some(checkpoint),
                state_version: None,
                conversation_epoch: None,
                scope_revision: 0,
                scope_lifecycle: RuntimeScopeLifecycle::Active,
                cas_expected_state_version: None,
                cas_expected_scope_revision: 0,
                transcript_ack_proof: None,
            })?;
            store.repair_scope_index_best_effort(&scope_id);
            Ok(())
        })
    }

    fn runtime_state_ids<'a>(
        &'a self,
        scope_id: &'a str,
    ) -> BoxFuture<'a, crate::error::Result<Vec<String>>> {
        self.run_scope_blocking(scope_id.to_string(), move |store, scope_id| {
            let _scope = store.lock_scope(&scope_id)?;
            let _runtime_shards = store.lock_all_runtime_shards()?;
            let records = store.runtime_records_sync()?;
            let runtime_state_ids = Self::scope_runtime_state_ids(&scope_id, &records);
            if let Err(error) = store.reconcile_scope_index_sync(&scope_id, &records) {
                tracing::warn!(
                    %scope_id,
                    %error,
                    "runtime scope projection repair failed during authoritative enumeration"
                );
            }
            Ok(runtime_state_ids)
        })
    }

    fn clear_runtime_state<'a>(
        &'a self,
        scope_id: &'a str,
        runtime_state_id: &'a str,
    ) -> BoxFuture<'a, crate::error::Result<RuntimeStateClearReceipt>> {
        let runtime_state_id = runtime_state_id.to_string();
        let scope_id = scope_id.to_string();
        self.run_blocking(runtime_state_id, move |store, runtime_state_id| {
            let _safe_scope = safe_segment(&scope_id)?;
            let _scope = store.lock_scope(&scope_id)?;
            let _runtime = store.lock_runtime(&runtime_state_id)?;
            if store.reconcile_scope_owner_sync(&scope_id)?.is_some() {
                return Err(Self::managed_state_error(format!(
                    "scope {scope_id} is revision managed"
                )));
            }
            if store
                .read_runtime_owner_sync(&runtime_state_id)?
                .is_some_and(|owner| owner.state_version.is_some())
            {
                return Err(Self::managed_state_error(format!(
                    "runtime state {runtime_state_id} is revision managed"
                )));
            }
            let checkpoint_removed =
                store.mark_runtime_deleting_sync(&scope_id, &runtime_state_id)?;
            let _removed = store.remove_runtime_record_sync(&runtime_state_id)?;
            store.repair_scope_index_best_effort(&scope_id);
            Ok(RuntimeStateClearReceipt {
                scope_id,
                runtime_state_id,
                checkpoint_removed,
            })
        })
    }

    fn clear_runtime_state_scope<'a>(
        &'a self,
        scope_id: &'a str,
    ) -> BoxFuture<'a, crate::error::Result<RuntimeStateScopeClearReceipt>> {
        self.run_scope_blocking(scope_id.to_string(), move |store, scope_id| {
            let _scope = store.lock_scope(&scope_id)?;
            let _runtime_shards = store.lock_all_runtime_shards()?;
            if store.reconcile_scope_owner_sync(&scope_id)?.is_some() {
                return Err(Self::managed_state_error(format!(
                    "scope {scope_id} is revision managed"
                )));
            }
            let records = store.runtime_records_sync()?;
            let runtime_state_ids = records
                .iter()
                .filter(|record| record.scope_id == scope_id)
                .map(|record| record.runtime_state_id.clone())
                .collect::<Vec<_>>();
            for runtime_state_id in &runtime_state_ids {
                let _marked = store.mark_runtime_deleting_sync(&scope_id, runtime_state_id)?;
            }
            for runtime_state_id in &runtime_state_ids {
                let _removed = store.remove_runtime_record_sync(runtime_state_id)?;
            }
            store.repair_scope_index_best_effort(&scope_id);
            Ok(RuntimeStateScopeClearReceipt {
                scope_id,
                runtime_state_ids,
            })
        })
    }

    fn clear_conversation<'a>(
        &'a self,
        conversation_id: &'a str,
    ) -> BoxFuture<'a, crate::error::Result<()>> {
        Box::pin(async move {
            self.clear_runtime_state(conversation_id, conversation_id)
                .await
                .map(|_receipt| ())
        })
    }
}

/// Validate an id and encode its exact UTF-8 bytes as one safe path segment.
fn safe_segment(id: &str) -> Result<String, ReactError> {
    echo_core::utils::fs::encode_path_segment_identity(id).map_err(|error| {
        echo_core::error::RuntimeStateError::SerializationError(error.to_string()).into()
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use std::time::Duration;

    fn tmp_base() -> PathBuf {
        std::env::temp_dir().join(format!(
            "echo-file-runtime-state-{}-{}",
            std::process::id(),
            uuid::Uuid::new_v4()
        ))
    }

    fn checkpoint(runtime_state_id: &str, marker: &str) -> AgentCheckpoint {
        AgentCheckpoint {
            conversation_id: runtime_state_id.to_string(),
            messages_json: serde_json::json!([{"role": "user", "content": marker}]).to_string(),
            current_plan: None,
            active_skills: Vec::new(),
            blocked_reason: None,
            working_dir: None,
            timestamp: Utc::now(),
        }
    }

    fn managed_checkpoint(
        runtime_state_id: &str,
        marker: &str,
    ) -> crate::error::Result<AgentCheckpoint> {
        let mut checkpoint = checkpoint(runtime_state_id, marker);
        checkpoint.messages_json = AgentCheckpoint::serialize_payload(
            vec![crate::llm::types::Message::user(marker.to_string())],
            None,
        )?;
        Ok(checkpoint)
    }

    fn pending_checkpoint(
        runtime_state_id: &str,
        scope_id: &str,
    ) -> crate::error::Result<AgentCheckpoint> {
        pending_checkpoint_at_revision(runtime_state_id, scope_id, 1)
    }

    fn pending_checkpoint_at_revision(
        runtime_state_id: &str,
        scope_id: &str,
        base_runtime_revision: u64,
    ) -> crate::error::Result<AgentCheckpoint> {
        let messages = vec![crate::llm::types::Message::user("pending".to_string())];
        let projected = crate::memory::project_messages(scope_id, &messages)?;
        let batch = crate::memory::TranscriptProjectionBatch::prepare(
            scope_id,
            1,
            runtime_state_id,
            0,
            projected,
        )?;
        let cursor_before = super::super::TranscriptProjectionCheckpoint {
            generation_id: runtime_state_id.to_string(),
            next_ordinal: 0,
            projected: Vec::new(),
        };
        let projected = batch
            .items
            .iter()
            .map(|item| {
                Ok(super::super::TranscriptProjectionMessage {
                    ordinal: item.ordinal,
                    digest: crate::memory::transcript_projection_message_digest(&item.message)?,
                })
            })
            .collect::<crate::error::Result<Vec<_>>>()?;
        let cursor_after = super::super::TranscriptProjectionCheckpoint {
            generation_id: runtime_state_id.to_string(),
            next_ordinal: 1,
            projected,
        };
        Ok(AgentCheckpoint {
            conversation_id: runtime_state_id.to_string(),
            messages_json: AgentCheckpoint::serialize_managed_payload(
                messages,
                Some(cursor_before.clone()),
                Some(super::super::PendingTranscriptProjection {
                    batch,
                    cursor_before,
                    cursor_after,
                    base_runtime_revision,
                    prepared_at: Utc::now(),
                    attempt: 1,
                    last_attempt_class: None,
                    last_error: None,
                }),
            )?,
            current_plan: None,
            active_skills: Vec::new(),
            blocked_reason: None,
            working_dir: None,
            timestamp: Utc::now(),
        })
    }

    fn transcript_ack_request(
        checkpoint: &AgentCheckpoint,
        scope_revision: u64,
        state_revision: u64,
        status: crate::memory::TranscriptProjectionApplyStatus,
    ) -> crate::error::Result<RuntimeTranscriptAckRequest> {
        let payload = checkpoint.restore_managed_runtime_payload()?;
        let pending = payload.pending_transcript_projection.ok_or_else(|| {
            echo_core::error::RuntimeStateError::NotFound(
                "test checkpoint has no pending transcript projection".to_string(),
            )
        })?;
        let mut acknowledged = checkpoint.clone();
        acknowledged.messages_json = AgentCheckpoint::serialize_managed_payload(
            payload.messages,
            Some(pending.cursor_after.clone()),
            None,
        )?;
        acknowledged.timestamp = Utc::now();
        Ok(RuntimeTranscriptAckRequest {
            scope_id: pending.batch.conversation_id.clone(),
            runtime_state_id: pending.batch.generation_id.clone(),
            conversation_epoch: pending.batch.conversation_epoch,
            expected_scope_revision: scope_revision,
            expected_state_version: RuntimeStateExpectedVersion::Managed {
                revision: state_revision,
            },
            projection_receipt: crate::memory::TranscriptProjectionApplyReceipt {
                operation_id: pending.batch.operation_id,
                payload_digest: pending.batch.payload_digest,
                authority: crate::memory::ConversationProjectionAuthority {
                    conversation_id: pending.batch.conversation_id,
                    epoch: pending.batch.conversation_epoch,
                    revision: 1,
                    lifecycle: crate::memory::ConversationProjectionLifecycle::Live,
                    delete_receipt_retention_floor_epoch: 0,
                },
                status,
            },
            checkpoint: acknowledged,
        })
    }

    fn update_pending_checkpoint(
        checkpoint: &AgentCheckpoint,
        update: impl FnOnce(&mut super::super::PendingTranscriptProjection),
    ) -> crate::error::Result<AgentCheckpoint> {
        let payload = checkpoint.restore_managed_runtime_payload()?;
        let mut pending = payload.pending_transcript_projection.ok_or_else(|| {
            echo_core::error::RuntimeStateError::NotFound("pending debt".to_string())
        })?;
        update(&mut pending);
        let mut updated = checkpoint.clone();
        updated.messages_json = AgentCheckpoint::serialize_managed_payload(
            payload.messages,
            payload.transcript_projection,
            Some(pending),
        )?;
        updated.timestamp = Utc::now();
        Ok(updated)
    }

    fn retirement_manifest(
        scope_id: &str,
        epoch: u64,
    ) -> crate::error::Result<ScopeRetirementManifest> {
        let delete = crate::memory::ManagedConversationDelete::prepare(scope_id, epoch)?;
        Ok(ScopeRetirementManifest {
            delete_operation_id: delete.operation_id,
            payload_digest: delete.payload_digest,
            expected_conversation_epoch: epoch,
            items: Vec::new(),
            conversation_delete_receipt: None,
        })
    }

    #[tokio::test]
    async fn revisioned_runtime_state_contract_is_supported() -> crate::error::Result<()> {
        let tmp = tmp_base();
        let store = FileRuntimeStateStore::new(&tmp)?;

        assert_eq!(
            store.runtime_state_capability(),
            super::super::RuntimeStateCapability::RevisionedV1
        );
        assert!(store.load_scope_authority("scope-a").await?.is_none());

        for epoch in [0, MAX_CONVERSATION_EPOCH.saturating_add(1)] {
            assert!(
                store
                    .compare_and_save_checkpoint(RuntimeCheckpointCasRequest {
                        scope_id: "scope-a".to_string(),
                        runtime_state_id: "runtime-a".to_string(),
                        conversation_epoch: Some(epoch),
                        expected_scope_revision: 0,
                        expected_state_version: RuntimeStateExpectedVersion::Absent,
                        checkpoint: managed_checkpoint("runtime-a", "invalid-epoch")?,
                    })
                    .await
                    .is_err()
            );
        }
        assert!(store.load_scope_authority("scope-a").await?.is_none());

        let _ = std::fs::remove_dir_all(&tmp);
        Ok(())
    }

    #[tokio::test]
    async fn revisioned_cas_replays_and_fences_legacy_mutators() -> crate::error::Result<()> {
        let tmp = tmp_base();
        let store = FileRuntimeStateStore::new(&tmp)?;
        let initial = managed_checkpoint("runtime-a", "initial")?;
        let create = RuntimeCheckpointCasRequest {
            scope_id: "scope-a".to_string(),
            runtime_state_id: "runtime-a".to_string(),
            conversation_epoch: Some(1),
            expected_scope_revision: 0,
            expected_state_version: RuntimeStateExpectedVersion::Absent,
            checkpoint: initial.clone(),
        };

        let applied = store.compare_and_save_checkpoint(create.clone()).await?;
        assert_eq!(applied.status, RuntimeCheckpointCasStatus::Applied);
        assert_eq!(applied.scope.revision, 1);
        assert_eq!(
            applied.version,
            RuntimeStateVersion::Managed { revision: 1 }
        );
        let replayed = store.compare_and_save_checkpoint(create).await?;
        assert_eq!(replayed.status, RuntimeCheckpointCasStatus::AlreadyCurrent);
        assert_eq!(replayed.scope.revision, 1);
        let false_replay = store
            .compare_and_save_checkpoint(RuntimeCheckpointCasRequest {
                scope_id: "scope-a".to_string(),
                runtime_state_id: "runtime-a".to_string(),
                conversation_epoch: Some(1),
                expected_scope_revision: 99,
                expected_state_version: RuntimeStateExpectedVersion::Managed { revision: 99 },
                checkpoint: initial.clone(),
            })
            .await?;
        assert_eq!(
            false_replay.status,
            RuntimeCheckpointCasStatus::RevisionConflict
        );
        assert!(store.save_checkpoint(&initial).await.is_err());
        assert!(
            store
                .clear_runtime_state("scope-a", "runtime-a")
                .await
                .is_err()
        );

        let updated = managed_checkpoint("runtime-a", "updated")?;
        let stale = store
            .compare_and_save_checkpoint(RuntimeCheckpointCasRequest {
                scope_id: "scope-a".to_string(),
                runtime_state_id: "runtime-a".to_string(),
                conversation_epoch: Some(1),
                expected_scope_revision: 0,
                expected_state_version: RuntimeStateExpectedVersion::Managed { revision: 1 },
                checkpoint: updated.clone(),
            })
            .await?;
        assert_eq!(stale.status, RuntimeCheckpointCasStatus::RevisionConflict);
        let update = store
            .compare_and_save_checkpoint(RuntimeCheckpointCasRequest {
                scope_id: "scope-a".to_string(),
                runtime_state_id: "runtime-a".to_string(),
                conversation_epoch: Some(1),
                expected_scope_revision: 1,
                expected_state_version: RuntimeStateExpectedVersion::Managed { revision: 1 },
                checkpoint: updated,
            })
            .await?;
        assert_eq!(update.status, RuntimeCheckpointCasStatus::Applied);
        assert_eq!(update.version, RuntimeStateVersion::Managed { revision: 2 });

        let retired = store
            .retire_runtime_generation(RuntimeGenerationRetireRequest::prepare(
                "scope-a",
                "runtime-a",
                2,
                RuntimeStateExpectedVersion::Managed { revision: 2 },
            )?)
            .await?;
        assert_eq!(retired.status, RuntimeGenerationRetireStatus::Retired);
        assert_eq!(retired.scope.revision, 3);
        assert!(store.get_checkpoint("runtime-a").await?.is_none());
        let replayed = store
            .retire_runtime_generation(RuntimeGenerationRetireRequest::prepare(
                "scope-a",
                "runtime-a",
                2,
                RuntimeStateExpectedVersion::Managed { revision: 2 },
            )?)
            .await?;
        assert_eq!(
            replayed.status,
            RuntimeGenerationRetireStatus::AlreadyRetired
        );
        let false_replay = store
            .retire_runtime_generation(RuntimeGenerationRetireRequest::prepare(
                "scope-a",
                "runtime-a",
                1,
                RuntimeStateExpectedVersion::Managed { revision: 1 },
            )?)
            .await?;
        assert_eq!(false_replay.status, RuntimeGenerationRetireStatus::Conflict);

        drop(store);
        let restarted = FileRuntimeStateStore::new(&tmp)?;
        let snapshot = restarted
            .load_runtime_state("scope-a", "runtime-a")
            .await?
            .ok_or_else(|| ReactError::Other("retirement tombstone missing".to_string()))?;
        assert!(matches!(
            snapshot.version,
            RuntimeStateVersion::Retired { .. }
        ));
        assert!(snapshot.checkpoint.is_none());

        let legacy = checkpoint("runtime-b", "legacy");
        restarted
            .save_checkpoint_for_scope("scope-b", &legacy)
            .await?;
        let unmanaged = restarted
            .load_runtime_state("scope-b", "runtime-b")
            .await?
            .ok_or_else(|| ReactError::Other("unmanaged checkpoint missing".to_string()))?;
        let RuntimeStateVersion::Unmanaged { digest } = unmanaged.version else {
            return Err(ReactError::Other(
                "legacy checkpoint was not reported as unmanaged".to_string(),
            ));
        };
        let adopted = restarted
            .compare_and_save_checkpoint(RuntimeCheckpointCasRequest {
                scope_id: "scope-b".to_string(),
                runtime_state_id: "runtime-b".to_string(),
                conversation_epoch: Some(1),
                expected_scope_revision: 0,
                expected_state_version: RuntimeStateExpectedVersion::Unmanaged { digest },
                checkpoint: managed_checkpoint("runtime-b", "adopted")?,
            })
            .await?;
        assert_eq!(adopted.status, RuntimeCheckpointCasStatus::Applied);
        assert_eq!(
            adopted.version,
            RuntimeStateVersion::Managed { revision: 1 }
        );
        let _ = std::fs::remove_dir_all(&tmp);
        Ok(())
    }

    #[tokio::test]
    async fn pending_transcript_requires_proof_carrying_acknowledgement() -> crate::error::Result<()>
    {
        let tmp = tmp_base();
        let store = FileRuntimeStateStore::new(&tmp)?;
        let pending = pending_checkpoint("runtime-pending", "scope-pending")?;
        let created = store
            .compare_and_save_checkpoint(RuntimeCheckpointCasRequest {
                scope_id: "scope-pending".to_string(),
                runtime_state_id: "runtime-pending".to_string(),
                conversation_epoch: Some(1),
                expected_scope_revision: 0,
                expected_state_version: RuntimeStateExpectedVersion::Absent,
                checkpoint: pending.clone(),
            })
            .await?;

        let clear = transcript_ack_request(
            &pending,
            created.scope.revision,
            1,
            crate::memory::TranscriptProjectionApplyStatus::Applied,
        )?
        .checkpoint;
        assert!(
            store
                .compare_and_save_checkpoint(RuntimeCheckpointCasRequest {
                    scope_id: "scope-pending".to_string(),
                    runtime_state_id: "runtime-pending".to_string(),
                    conversation_epoch: Some(1),
                    expected_scope_revision: created.scope.revision,
                    expected_state_version: RuntimeStateExpectedVersion::Managed { revision: 1 },
                    checkpoint: clear,
                })
                .await
                .is_err()
        );
        assert!(
            store
                .compare_and_save_checkpoint(RuntimeCheckpointCasRequest {
                    scope_id: "scope-pending".to_string(),
                    runtime_state_id: "runtime-pending".to_string(),
                    conversation_epoch: Some(1),
                    expected_scope_revision: created.scope.revision,
                    expected_state_version: RuntimeStateExpectedVersion::Managed { revision: 1 },
                    checkpoint: pending_checkpoint("runtime-pending", "scope-pending")?,
                })
                .await
                .is_err()
        );

        let payload = pending.restore_managed_runtime_payload()?;
        let mut retry_debt = payload.pending_transcript_projection.ok_or_else(|| {
            echo_core::error::RuntimeStateError::NotFound("pending debt".to_string())
        })?;
        retry_debt.base_runtime_revision = 2;
        let mut invalid_retry = pending.clone();
        invalid_retry.messages_json = AgentCheckpoint::serialize_managed_payload(
            payload.messages.clone(),
            Some(retry_debt.cursor_before.clone()),
            Some(retry_debt.clone()),
        )?;
        assert!(
            store
                .compare_and_save_checkpoint(RuntimeCheckpointCasRequest {
                    scope_id: "scope-pending".to_string(),
                    runtime_state_id: "runtime-pending".to_string(),
                    conversation_epoch: Some(1),
                    expected_scope_revision: created.scope.revision,
                    expected_state_version: RuntimeStateExpectedVersion::Managed { revision: 1 },
                    checkpoint: invalid_retry,
                })
                .await
                .is_err()
        );
        retry_debt.last_attempt_class =
            Some(super::super::TranscriptProjectionAttemptClass::TransientNoCommit);
        retry_debt.last_error = Some("retryable".to_string());
        let mut recorded = pending.clone();
        recorded.messages_json = AgentCheckpoint::serialize_managed_payload(
            payload.messages.clone(),
            Some(retry_debt.cursor_before.clone()),
            Some(retry_debt.clone()),
        )?;
        recorded.timestamp = Utc::now();
        let mut jumped = recorded.clone();
        let mut jumped_debt = retry_debt.clone();
        jumped_debt.attempt = 3;
        jumped.messages_json = AgentCheckpoint::serialize_managed_payload(
            payload.messages.clone(),
            Some(jumped_debt.cursor_before.clone()),
            Some(jumped_debt),
        )?;
        assert!(
            store
                .compare_and_save_checkpoint(RuntimeCheckpointCasRequest {
                    scope_id: "scope-pending".to_string(),
                    runtime_state_id: "runtime-pending".to_string(),
                    conversation_epoch: Some(1),
                    expected_scope_revision: created.scope.revision,
                    expected_state_version: RuntimeStateExpectedVersion::Managed { revision: 1 },
                    checkpoint: jumped,
                })
                .await
                .is_err()
        );
        let mut metadata_rewrites = Vec::new();
        let mut changed_plan = recorded.clone();
        changed_plan.current_plan = Some("forged plan".to_string());
        metadata_rewrites.push(changed_plan);
        let mut changed_skills = recorded.clone();
        changed_skills.active_skills = vec!["forged-skill".to_string()];
        metadata_rewrites.push(changed_skills);
        let mut changed_block = recorded.clone();
        changed_block.blocked_reason = Some("forged block".to_string());
        metadata_rewrites.push(changed_block);
        let mut changed_workdir = recorded.clone();
        changed_workdir.working_dir = Some(PathBuf::from("/forged"));
        metadata_rewrites.push(changed_workdir);
        for checkpoint in metadata_rewrites {
            assert!(
                store
                    .compare_and_save_checkpoint(RuntimeCheckpointCasRequest {
                        scope_id: "scope-pending".to_string(),
                        runtime_state_id: "runtime-pending".to_string(),
                        conversation_epoch: Some(1),
                        expected_scope_revision: created.scope.revision,
                        expected_state_version: RuntimeStateExpectedVersion::Managed {
                            revision: 1,
                        },
                        checkpoint,
                    })
                    .await
                    .is_err()
            );
        }
        let failure = store
            .compare_and_save_checkpoint(RuntimeCheckpointCasRequest {
                scope_id: "scope-pending".to_string(),
                runtime_state_id: "runtime-pending".to_string(),
                conversation_epoch: Some(1),
                expected_scope_revision: created.scope.revision,
                expected_state_version: RuntimeStateExpectedVersion::Managed { revision: 1 },
                checkpoint: recorded.clone(),
            })
            .await?;
        assert_eq!(failure.status, RuntimeCheckpointCasStatus::Applied);
        let recorded_payload = recorded.restore_managed_runtime_payload()?;
        let mut dispatch_debt =
            recorded_payload
                .pending_transcript_projection
                .ok_or_else(|| {
                    echo_core::error::RuntimeStateError::NotFound("recorded debt".to_string())
                })?;
        dispatch_debt.attempt = 2;
        dispatch_debt.base_runtime_revision = 3;
        let mut carried = recorded.clone();
        carried.messages_json = AgentCheckpoint::serialize_managed_payload(
            recorded_payload.messages.clone(),
            Some(dispatch_debt.cursor_before.clone()),
            Some(dispatch_debt.clone()),
        )?;
        assert!(
            store
                .compare_and_save_checkpoint(RuntimeCheckpointCasRequest {
                    scope_id: "scope-pending".to_string(),
                    runtime_state_id: "runtime-pending".to_string(),
                    conversation_epoch: Some(1),
                    expected_scope_revision: failure.scope.revision,
                    expected_state_version: RuntimeStateExpectedVersion::Managed { revision: 2 },
                    checkpoint: carried,
                })
                .await
                .is_err()
        );
        dispatch_debt.last_attempt_class = None;
        dispatch_debt.last_error = None;
        let mut retried = recorded;
        retried.messages_json = AgentCheckpoint::serialize_managed_payload(
            recorded_payload.messages,
            Some(dispatch_debt.cursor_before.clone()),
            Some(dispatch_debt),
        )?;
        retried.timestamp = Utc::now();
        let retry = store
            .compare_and_save_checkpoint(RuntimeCheckpointCasRequest {
                scope_id: "scope-pending".to_string(),
                runtime_state_id: "runtime-pending".to_string(),
                conversation_epoch: Some(1),
                expected_scope_revision: failure.scope.revision,
                expected_state_version: RuntimeStateExpectedVersion::Managed { revision: 2 },
                checkpoint: retried.clone(),
            })
            .await?;
        assert_eq!(retry.status, RuntimeCheckpointCasStatus::Applied);

        let mut forged = transcript_ack_request(
            &retried,
            retry.scope.revision,
            3,
            crate::memory::TranscriptProjectionApplyStatus::Applied,
        )?;
        forged.projection_receipt.payload_digest = "forged".to_string();
        assert!(
            store
                .acknowledge_transcript_projection(forged)
                .await
                .is_err()
        );
        let unchanged = store
            .load_runtime_state("scope-pending", "runtime-pending")
            .await?
            .ok_or_else(|| {
                echo_core::error::RuntimeStateError::NotFound("runtime-pending".to_string())
            })?;
        assert_eq!(
            unchanged.version,
            RuntimeStateVersion::Managed { revision: 3 }
        );
        assert!(
            unchanged
                .checkpoint
                .as_ref()
                .ok_or_else(|| {
                    echo_core::error::RuntimeStateError::NotFound(
                        "runtime-pending checkpoint".to_string(),
                    )
                })?
                .restore_managed_runtime_payload()?
                .pending_transcript_projection
                .is_some()
        );

        let ack = transcript_ack_request(
            &retried,
            retry.scope.revision,
            3,
            crate::memory::TranscriptProjectionApplyStatus::Applied,
        )?;
        let mut forged_replay = ack.clone();
        forged_replay.projection_receipt.payload_digest = "forged-after-apply".to_string();
        let applied = store.acknowledge_transcript_projection(ack.clone()).await?;
        assert_eq!(applied.status, RuntimeCheckpointCasStatus::Applied);
        assert_eq!(
            store
                .acknowledge_transcript_projection(forged_replay)
                .await?
                .status,
            RuntimeCheckpointCasStatus::RevisionConflict
        );
        let replay = store.acknowledge_transcript_projection(ack).await?;
        assert_eq!(replay.status, RuntimeCheckpointCasStatus::AlreadyCurrent);
        assert_eq!(replay.scope, applied.scope);
        assert_eq!(replay.version, applied.version);

        let second = pending_checkpoint("runtime-pending-2", "scope-pending")?;
        let generic_checkpoint = managed_checkpoint("runtime-generic", "generic")?;
        let generic = store
            .compare_and_save_checkpoint(RuntimeCheckpointCasRequest {
                scope_id: "scope-pending".to_string(),
                runtime_state_id: "runtime-generic".to_string(),
                conversation_epoch: Some(1),
                expected_scope_revision: applied.scope.revision,
                expected_state_version: RuntimeStateExpectedVersion::Absent,
                checkpoint: generic_checkpoint.clone(),
            })
            .await?;
        let mut impersonation = transcript_ack_request(
            &second,
            applied.scope.revision,
            1,
            crate::memory::TranscriptProjectionApplyStatus::Applied,
        )?;
        impersonation.runtime_state_id = "runtime-generic".to_string();
        impersonation.checkpoint = generic_checkpoint;
        assert_eq!(
            store
                .acknowledge_transcript_projection(impersonation)
                .await?
                .status,
            RuntimeCheckpointCasStatus::RevisionConflict
        );
        let prepared = store
            .compare_and_save_checkpoint(RuntimeCheckpointCasRequest {
                scope_id: "scope-pending".to_string(),
                runtime_state_id: "runtime-pending-2".to_string(),
                conversation_epoch: Some(1),
                expected_scope_revision: generic.scope.revision,
                expected_state_version: RuntimeStateExpectedVersion::Absent,
                checkpoint: second.clone(),
            })
            .await?;
        let already_applied = store
            .acknowledge_transcript_projection(transcript_ack_request(
                &second,
                prepared.scope.revision,
                1,
                crate::memory::TranscriptProjectionApplyStatus::AlreadyApplied,
            )?)
            .await?;
        assert_eq!(already_applied.status, RuntimeCheckpointCasStatus::Applied);

        let owner = store
            .read_runtime_owner_sync("runtime-pending")?
            .ok_or_else(|| {
                echo_core::error::RuntimeStateError::NotFound("runtime-pending".to_string())
            })?;
        let mut unmanaged_proof = owner.clone();
        unmanaged_proof.state_version = Some(RuntimeStateVersion::Unmanaged {
            digest: "legacy".to_string(),
        });
        assert!(store.write_runtime_owner_sync(&unmanaged_proof).is_err());
        let mut uppercase_proof = owner;
        let proof = uppercase_proof
            .transcript_ack_proof
            .as_mut()
            .ok_or_else(|| {
                echo_core::error::RuntimeStateError::NotFound(
                    "runtime transcript acknowledgement proof".to_string(),
                )
            })?;
        proof.payload_digest = proof.payload_digest.to_ascii_uppercase();
        proof.operation_id = format!("transcript-projection-v1:{}", proof.payload_digest);
        let path = store.runtime_owner_path("runtime-pending")?;
        let raw = serde_json::to_vec_pretty(&uppercase_proof)
            .map_err(|error| FileRuntimeStateStore::invalid_state(error.to_string()))?;
        echo_core::utils::fs::atomic_write(&path, &raw)
            .map_err(FileRuntimeStateStore::to_react_err)?;
        assert!(
            store
                .load_runtime_state("scope-pending", "runtime-pending")
                .await
                .is_err()
        );
        assert_eq!(
            std::fs::read(path).map_err(FileRuntimeStateStore::to_react_err)?,
            raw
        );

        let _ = std::fs::remove_dir_all(&tmp);
        Ok(())
    }

    #[tokio::test]
    async fn legacy_raw_pending_is_rejected_without_side_effects() -> crate::error::Result<()> {
        let tmp = tmp_base();
        let store = FileRuntimeStateStore::new(&tmp)?;
        let pending = pending_checkpoint("legacy-pending", "legacy-scope")?;
        assert!(
            store
                .save_checkpoint_for_scope("legacy-scope", &pending)
                .await
                .is_err()
        );
        let checkpoint_only = managed_checkpoint("legacy-v2", "managed")?;
        store.save_checkpoint(&checkpoint_only).await?;
        assert!(!store.runtime_owner_path("legacy-pending")?.exists());
        let restored = store
            .get_checkpoint("legacy-v2")
            .await?
            .ok_or_else(|| echo_core::error::RuntimeStateError::NotFound("legacy-v2".to_string()))?
            .restore_messages()?;
        assert!(FileRuntimeStateStore::serialized_eq(
            &restored,
            &checkpoint_only.restore_messages()?
        )?);
        let cursor = super::super::TranscriptProjectionCheckpoint {
            generation_id: "legacy-cursor".to_string(),
            next_ordinal: 0,
            projected: Vec::new(),
        };
        let mut cursor_checkpoint = checkpoint("legacy-cursor", "cursor");
        cursor_checkpoint.messages_json = AgentCheckpoint::serialize_payload(
            vec![crate::llm::types::Message::user("cursor".to_string())],
            Some(cursor.clone()),
        )?;
        store.save_checkpoint(&cursor_checkpoint).await?;
        assert_eq!(
            store
                .get_checkpoint("legacy-cursor")
                .await?
                .ok_or_else(|| {
                    echo_core::error::RuntimeStateError::NotFound("legacy-cursor".to_string())
                })?
                .restore_transcript_projection()?,
            Some(cursor)
        );
        assert!(store.runtime_state_ids("legacy-scope").await?.is_empty());
        let _ = std::fs::remove_dir_all(&tmp);
        Ok(())
    }

    #[tokio::test]
    async fn corrupt_pending_base_revision_fails_closed() -> crate::error::Result<()> {
        let tmp = tmp_base();
        let store = FileRuntimeStateStore::new(&tmp)?;
        let pending = pending_checkpoint("runtime-base-corrupt", "scope-base-corrupt")?;
        store
            .compare_and_save_checkpoint(RuntimeCheckpointCasRequest {
                scope_id: "scope-base-corrupt".to_string(),
                runtime_state_id: "runtime-base-corrupt".to_string(),
                conversation_epoch: Some(1),
                expected_scope_revision: 0,
                expected_state_version: RuntimeStateExpectedVersion::Absent,
                checkpoint: pending,
            })
            .await?;
        let mut owner = store
            .read_runtime_owner_sync("runtime-base-corrupt")?
            .ok_or_else(|| {
                echo_core::error::RuntimeStateError::NotFound("runtime-base-corrupt".to_string())
            })?;
        let checkpoint = owner.checkpoint.as_mut().ok_or_else(|| {
            echo_core::error::RuntimeStateError::NotFound(
                "runtime-base-corrupt checkpoint".to_string(),
            )
        })?;
        let payload = checkpoint.restore_managed_runtime_payload()?;
        let mut debt = payload.pending_transcript_projection.ok_or_else(|| {
            echo_core::error::RuntimeStateError::NotFound("pending debt".to_string())
        })?;
        debt.base_runtime_revision = 99;
        checkpoint.messages_json = AgentCheckpoint::serialize_managed_payload(
            payload.messages,
            payload.transcript_projection,
            Some(debt),
        )?;
        let path = store.runtime_owner_path("runtime-base-corrupt")?;
        let raw = serde_json::to_vec_pretty(&owner)
            .map_err(|error| FileRuntimeStateStore::invalid_state(error.to_string()))?;
        echo_core::utils::fs::atomic_write(&path, &raw)
            .map_err(FileRuntimeStateStore::to_react_err)?;
        assert!(
            store
                .load_runtime_state("scope-base-corrupt", "runtime-base-corrupt")
                .await
                .is_err()
        );
        assert!(store.get_checkpoint("runtime-base-corrupt").await.is_err());
        assert!(
            store
                .load_scope_authority("scope-base-corrupt")
                .await
                .is_err()
        );
        assert_eq!(
            std::fs::read(&path).map_err(FileRuntimeStateStore::to_react_err)?,
            raw
        );
        let _ = std::fs::remove_dir_all(&tmp);
        Ok(())
    }

    #[tokio::test]
    async fn retry_terminal_failure_remains_durably_blocked() -> crate::error::Result<()> {
        let tmp = tmp_base();
        let store = FileRuntimeStateStore::new(&tmp)?;
        let initial = pending_checkpoint("runtime-terminal", "scope-terminal")?;
        let created = store
            .compare_and_save_checkpoint(RuntimeCheckpointCasRequest {
                scope_id: "scope-terminal".to_string(),
                runtime_state_id: "runtime-terminal".to_string(),
                conversation_epoch: Some(1),
                expected_scope_revision: 0,
                expected_state_version: RuntimeStateExpectedVersion::Absent,
                checkpoint: initial.clone(),
            })
            .await?;
        let failure = update_pending_checkpoint(&initial, |pending| {
            pending.base_runtime_revision = 2;
            pending.last_attempt_class =
                Some(super::super::TranscriptProjectionAttemptClass::DeadlineExceeded);
            pending.last_error = Some("attempt one deadline".to_string());
        })?;
        let failed = store
            .compare_and_save_checkpoint(RuntimeCheckpointCasRequest {
                scope_id: "scope-terminal".to_string(),
                runtime_state_id: "runtime-terminal".to_string(),
                conversation_epoch: Some(1),
                expected_scope_revision: created.scope.revision,
                expected_state_version: RuntimeStateExpectedVersion::Managed { revision: 1 },
                checkpoint: failure.clone(),
            })
            .await?;
        let dispatch = update_pending_checkpoint(&failure, |pending| {
            pending.attempt = 2;
            pending.base_runtime_revision = 3;
            pending.last_attempt_class = None;
            pending.last_error = None;
        })?;
        let dispatched = store
            .compare_and_save_checkpoint(RuntimeCheckpointCasRequest {
                scope_id: "scope-terminal".to_string(),
                runtime_state_id: "runtime-terminal".to_string(),
                conversation_epoch: Some(1),
                expected_scope_revision: failed.scope.revision,
                expected_state_version: RuntimeStateExpectedVersion::Managed { revision: 2 },
                checkpoint: dispatch.clone(),
            })
            .await?;
        let terminal = update_pending_checkpoint(&dispatch, |pending| {
            pending.base_runtime_revision = 4;
            pending.last_attempt_class =
                Some(super::super::TranscriptProjectionAttemptClass::SemanticConflict);
            pending.last_error = Some("attempt two semantic conflict".to_string());
        })?;
        let terminal_receipt = store
            .compare_and_save_checkpoint(RuntimeCheckpointCasRequest {
                scope_id: "scope-terminal".to_string(),
                runtime_state_id: "runtime-terminal".to_string(),
                conversation_epoch: Some(1),
                expected_scope_revision: dispatched.scope.revision,
                expected_state_version: RuntimeStateExpectedVersion::Managed { revision: 3 },
                checkpoint: terminal.clone(),
            })
            .await?;
        let loaded = store
            .load_runtime_state("scope-terminal", "runtime-terminal")
            .await?
            .ok_or_else(|| {
                echo_core::error::RuntimeStateError::NotFound("runtime-terminal".to_string())
            })?;
        let debt = loaded
            .checkpoint
            .ok_or_else(|| {
                echo_core::error::RuntimeStateError::NotFound(
                    "runtime-terminal checkpoint".to_string(),
                )
            })?
            .restore_managed_runtime_payload()?
            .pending_transcript_projection
            .ok_or_else(|| {
                echo_core::error::RuntimeStateError::NotFound("terminal debt".to_string())
            })?;
        assert_eq!(debt.attempt, 2);
        assert_eq!(
            debt.last_attempt_class,
            Some(super::super::TranscriptProjectionAttemptClass::SemanticConflict)
        );
        let retry = update_pending_checkpoint(&terminal, |pending| {
            pending.attempt = 3;
            pending.base_runtime_revision = 5;
            pending.last_attempt_class = None;
            pending.last_error = None;
        })?;
        assert!(
            store
                .compare_and_save_checkpoint(RuntimeCheckpointCasRequest {
                    scope_id: "scope-terminal".to_string(),
                    runtime_state_id: "runtime-terminal".to_string(),
                    conversation_epoch: Some(1),
                    expected_scope_revision: terminal_receipt.scope.revision,
                    expected_state_version: RuntimeStateExpectedVersion::Managed { revision: 4 },
                    checkpoint: retry,
                })
                .await
                .is_err()
        );
        let _ = std::fs::remove_dir_all(&tmp);
        Ok(())
    }

    #[tokio::test]
    async fn managed_deadline_expires_in_queue_without_side_effects() -> crate::error::Result<()> {
        let tmp = tmp_base();
        let store = FileRuntimeStateStore::new(&tmp)?;
        assert_eq!(
            store.persistence_call_capability(),
            crate::memory::PersistenceCallCapability::AbsoluteDeadlineV1
        );
        let blocker_store = store.clone();
        let (entered_tx, entered_rx) = tokio::sync::oneshot::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let blocker = tokio::spawn(async move {
            blocker_store
                .run_blocking("deadline-runtime".to_string(), move |_store, _| {
                    let _ignored = entered_tx.send(());
                    release_rx
                        .recv_timeout(Duration::from_secs(2))
                        .map_err(FileRuntimeStateStore::to_react_err)?;
                    Ok(())
                })
                .await
        });
        entered_rx
            .await
            .map_err(FileRuntimeStateStore::to_react_err)?;
        let context =
            crate::memory::PersistenceCallContext::with_timeout(Duration::from_millis(25))?;
        let queued_store = store.clone();
        let queued = tokio::spawn(async move {
            queued_store
                .compare_and_save_checkpoint_with_context(
                    context,
                    RuntimeCheckpointCasRequest {
                        scope_id: "deadline-scope".to_string(),
                        runtime_state_id: "deadline-runtime".to_string(),
                        conversation_epoch: Some(1),
                        expected_scope_revision: 0,
                        expected_state_version: RuntimeStateExpectedVersion::Absent,
                        checkpoint: managed_checkpoint("deadline-runtime", "deadline")?,
                    },
                )
                .await
        });
        tokio::time::sleep(Duration::from_millis(60)).await;
        release_tx
            .send(())
            .map_err(FileRuntimeStateStore::to_react_err)?;
        blocker
            .await
            .map_err(FileRuntimeStateStore::to_react_err)??;
        let error = queued
            .await
            .map_err(FileRuntimeStateStore::to_react_err)?
            .err()
            .ok_or_else(|| {
                FileRuntimeStateStore::invalid_state("expired runtime write was accepted")
            })?;
        assert!(matches!(
            error,
            ReactError::RuntimeState(error)
                if matches!(
                    error.as_ref(),
                    echo_core::error::RuntimeStateError::DeadlineExceeded(_)
                )
        ));
        assert!(
            store
                .load_runtime_state("deadline-scope", "deadline-runtime")
                .await?
                .is_none()
        );
        let _ = std::fs::remove_dir_all(&tmp);
        Ok(())
    }

    #[tokio::test]
    async fn invalid_runtime_state_requests_have_no_durable_side_effects()
    -> crate::error::Result<()> {
        let tmp = tmp_base();
        let store = FileRuntimeStateStore::new(&tmp)?;
        let applied = store
            .compare_and_save_checkpoint(RuntimeCheckpointCasRequest {
                scope_id: "scope-validation".to_string(),
                runtime_state_id: "runtime-validation".to_string(),
                conversation_epoch: Some(1),
                expected_scope_revision: 0,
                expected_state_version: RuntimeStateExpectedVersion::Absent,
                checkpoint: managed_checkpoint("runtime-validation", "original")?,
            })
            .await?;
        let authority_before = applied.scope;

        let mut invalid_retirement = RuntimeGenerationRetireRequest::prepare(
            "scope-validation",
            "runtime-validation",
            authority_before.revision,
            RuntimeStateExpectedVersion::Managed { revision: 1 },
        )?;
        invalid_retirement.payload_digest = "tampered".to_string();
        assert!(
            store
                .retire_runtime_generation(invalid_retirement)
                .await
                .is_err()
        );

        let delete = crate::memory::ManagedConversationDelete::prepare("scope-validation", 1)?;
        let mut invalid_scope_retirement = ScopeRetirementRequest::prepare(
            "scope-validation",
            authority_before.revision,
            &delete,
        )?;
        invalid_scope_retirement.payload_digest = "tampered".to_string();
        assert!(
            store
                .begin_scope_retirement(invalid_scope_retirement)
                .await
                .is_err()
        );

        let mut malformed_checkpoint = managed_checkpoint("runtime-validation", "invalid")?;
        malformed_checkpoint.messages_json = "{".to_string();
        assert!(
            store
                .compare_and_save_checkpoint(RuntimeCheckpointCasRequest {
                    scope_id: "scope-validation".to_string(),
                    runtime_state_id: "runtime-validation".to_string(),
                    conversation_epoch: Some(1),
                    expected_scope_revision: authority_before.revision,
                    expected_state_version: RuntimeStateExpectedVersion::Managed { revision: 1 },
                    checkpoint: malformed_checkpoint,
                })
                .await
                .is_err()
        );

        assert_eq!(
            store.load_scope_authority("scope-validation").await?,
            Some(authority_before)
        );
        let state = store
            .load_runtime_state("scope-validation", "runtime-validation")
            .await?
            .ok_or_else(|| FileRuntimeStateStore::to_react_err("runtime state disappeared"))?;
        assert_eq!(state.version, RuntimeStateVersion::Managed { revision: 1 });
        assert!(
            state
                .checkpoint
                .is_some_and(|checkpoint| checkpoint.messages_json.contains("original"))
        );
        let _ = std::fs::remove_dir_all(&tmp);
        Ok(())
    }

    #[tokio::test]
    async fn corrupt_runtime_scope_saga_fails_closed() -> crate::error::Result<()> {
        let tmp = tmp_base();
        let store = FileRuntimeStateStore::new(&tmp)?;
        let path = store.scope_owner_path("scope-corrupt")?;
        let active_with_manifest = RuntimeScopeOwner {
            version: RUNTIME_SCOPE_RECORD_VERSION,
            authority: RuntimeScopeAuthority {
                scope_id: "scope-corrupt".to_string(),
                conversation_epoch: Some(1),
                revision: 1,
                lifecycle: RuntimeScopeLifecycle::Active,
            },
            current_retirement: Some(retirement_manifest("scope-corrupt", 1)?),
            completed_retirements: Vec::new(),
        };
        let raw = serde_json::to_vec_pretty(&active_with_manifest)
            .map_err(FileRuntimeStateStore::to_react_err)?;
        echo_core::utils::fs::atomic_write(&path, &raw)
            .map_err(FileRuntimeStateStore::to_react_err)?;
        assert!(store.load_scope_authority("scope-corrupt").await.is_err());
        assert!(
            store
                .compare_and_save_checkpoint(RuntimeCheckpointCasRequest {
                    scope_id: "scope-corrupt".to_string(),
                    runtime_state_id: "runtime-corrupt".to_string(),
                    conversation_epoch: Some(1),
                    expected_scope_revision: 1,
                    expected_state_version: RuntimeStateExpectedVersion::Absent,
                    checkpoint: managed_checkpoint("runtime-corrupt", "must not persist")?,
                })
                .await
                .is_err()
        );
        assert!(!store.runtime_owner_path("runtime-corrupt")?.exists());

        let retiring_epoch_mismatch = RuntimeScopeOwner {
            version: RUNTIME_SCOPE_RECORD_VERSION,
            authority: RuntimeScopeAuthority {
                scope_id: "scope-corrupt".to_string(),
                conversation_epoch: Some(1),
                revision: 1,
                lifecycle: RuntimeScopeLifecycle::Retiring,
            },
            current_retirement: Some(retirement_manifest("scope-corrupt", 2)?),
            completed_retirements: Vec::new(),
        };
        let raw = serde_json::to_vec_pretty(&retiring_epoch_mismatch)
            .map_err(FileRuntimeStateStore::to_react_err)?;
        echo_core::utils::fs::atomic_write(&path, &raw)
            .map_err(FileRuntimeStateStore::to_react_err)?;
        assert!(store.load_scope_authority("scope-corrupt").await.is_err());

        let mut identity_mismatch = retirement_manifest("scope-corrupt", 1)?;
        identity_mismatch.payload_digest = "tampered".to_string();
        let retiring_identity_mismatch = RuntimeScopeOwner {
            version: RUNTIME_SCOPE_RECORD_VERSION,
            authority: RuntimeScopeAuthority {
                scope_id: "scope-corrupt".to_string(),
                conversation_epoch: Some(1),
                revision: 1,
                lifecycle: RuntimeScopeLifecycle::Retiring,
            },
            current_retirement: Some(identity_mismatch),
            completed_retirements: Vec::new(),
        };
        let raw = serde_json::to_vec_pretty(&retiring_identity_mismatch)
            .map_err(FileRuntimeStateStore::to_react_err)?;
        echo_core::utils::fs::atomic_write(&path, &raw)
            .map_err(FileRuntimeStateStore::to_react_err)?;
        assert!(store.load_scope_authority("scope-corrupt").await.is_err());

        let checkpoint = managed_checkpoint("runtime-false-drop", "still-active")?;
        let created = store
            .compare_and_save_checkpoint(RuntimeCheckpointCasRequest {
                scope_id: "scope-false-drop".to_string(),
                runtime_state_id: "runtime-false-drop".to_string(),
                conversation_epoch: Some(1),
                expected_scope_revision: 0,
                expected_state_version: RuntimeStateExpectedVersion::Absent,
                checkpoint,
            })
            .await?;
        let delete = crate::memory::ManagedConversationDelete::prepare("scope-false-drop", 1)?;
        let dropped_item = ScopeRetirementItem {
            runtime_state_id: "runtime-false-drop".to_string(),
            state_version: RuntimeStateVersion::Managed { revision: 1 },
            pending_operation_id: None,
            status: ScopeRetirementItemStatus::DroppedByDelete,
        };
        let manifest = ScopeRetirementManifest {
            delete_operation_id: delete.operation_id.clone(),
            payload_digest: delete.payload_digest.clone(),
            expected_conversation_epoch: 1,
            items: vec![dropped_item.clone()],
            conversation_delete_receipt: None,
        };
        let false_drop_path = store.scope_owner_path("scope-false-drop")?;
        let false_drop = RuntimeScopeOwner {
            version: RUNTIME_SCOPE_RECORD_VERSION,
            authority: RuntimeScopeAuthority {
                lifecycle: RuntimeScopeLifecycle::Retiring,
                revision: created.scope.revision.saturating_add(1),
                ..created.scope.clone()
            },
            current_retirement: Some(manifest.clone()),
            completed_retirements: Vec::new(),
        };
        let raw = serde_json::to_vec_pretty(&false_drop)
            .map_err(|error| FileRuntimeStateStore::invalid_state(error.to_string()))?;
        echo_core::utils::fs::atomic_write(&false_drop_path, &raw)
            .map_err(FileRuntimeStateStore::to_react_err)?;
        let runtime_path = store.runtime_owner_path("runtime-false-drop")?;
        let runtime_before =
            std::fs::read(&runtime_path).map_err(FileRuntimeStateStore::to_react_err)?;
        assert!(
            store
                .continue_scope_retirement(
                    "scope-false-drop",
                    &delete.operation_id,
                    false_drop.authority.revision,
                    ScopeRetirementAdvance::Complete,
                )
                .await
                .is_err()
        );
        assert_eq!(
            std::fs::read(&runtime_path).map_err(FileRuntimeStateStore::to_react_err)?,
            runtime_before
        );

        let mut duplicate = false_drop.clone();
        duplicate
            .current_retirement
            .as_mut()
            .ok_or_else(|| {
                echo_core::error::RuntimeStateError::NotFound(
                    "test retirement manifest".to_string(),
                )
            })?
            .items
            .push(dropped_item);
        let raw = serde_json::to_vec_pretty(&duplicate)
            .map_err(|error| FileRuntimeStateStore::invalid_state(error.to_string()))?;
        echo_core::utils::fs::atomic_write(&false_drop_path, &raw)
            .map_err(FileRuntimeStateStore::to_react_err)?;
        assert!(
            store
                .load_scope_authority("scope-false-drop")
                .await
                .is_err()
        );

        let mut forged_receipt = manifest;
        forged_receipt.conversation_delete_receipt =
            Some(crate::memory::ManagedConversationDeleteReceipt {
                operation_id: delete.operation_id,
                payload_digest: delete.payload_digest,
                deleted_epoch: 1,
                retention_floor_epoch: 0,
                status: crate::memory::ManagedConversationDeleteStatus::EpochConflict,
            });
        let forged = RuntimeScopeOwner {
            current_retirement: Some(forged_receipt),
            ..false_drop.clone()
        };
        let raw = serde_json::to_vec_pretty(&forged)
            .map_err(|error| FileRuntimeStateStore::invalid_state(error.to_string()))?;
        echo_core::utils::fs::atomic_write(&false_drop_path, &raw)
            .map_err(FileRuntimeStateStore::to_react_err)?;
        assert!(
            store
                .load_scope_authority("scope-false-drop")
                .await
                .is_err()
        );
        assert_eq!(
            std::fs::read(&runtime_path).map_err(FileRuntimeStateStore::to_react_err)?,
            runtime_before
        );

        let incomplete_manifest = retirement_manifest("scope-false-drop", 1)?;
        let incomplete_history = RuntimeScopeOwner {
            version: RUNTIME_SCOPE_RECORD_VERSION,
            authority: RuntimeScopeAuthority {
                scope_id: "scope-false-drop".to_string(),
                conversation_epoch: Some(1),
                revision: false_drop.authority.revision.saturating_add(1),
                lifecycle: RuntimeScopeLifecycle::Active,
            },
            current_retirement: None,
            completed_retirements: vec![ScopeRetirementReceipt {
                scope: RuntimeScopeAuthority {
                    scope_id: "scope-false-drop".to_string(),
                    conversation_epoch: Some(1),
                    revision: false_drop.authority.revision,
                    lifecycle: RuntimeScopeLifecycle::Tombstoned,
                },
                manifest: incomplete_manifest,
                dropped_operation_ids: Vec::new(),
                retention_floor_epoch: 0,
                status: ScopeRetirementStatus::InProgress,
            }],
        };
        let raw = serde_json::to_vec_pretty(&incomplete_history)
            .map_err(|error| FileRuntimeStateStore::invalid_state(error.to_string()))?;
        echo_core::utils::fs::atomic_write(&false_drop_path, &raw)
            .map_err(FileRuntimeStateStore::to_react_err)?;
        assert!(
            store
                .load_scope_authority("scope-false-drop")
                .await
                .is_err()
        );
        let _ = std::fs::remove_dir_all(&tmp);
        Ok(())
    }

    #[tokio::test]
    async fn runtime_state_scope_retirement_replays_and_isolates_new_incarnation()
    -> crate::error::Result<()> {
        let tmp = tmp_base();
        let store = FileRuntimeStateStore::new(&tmp)?;
        let first_request = RuntimeCheckpointCasRequest {
            scope_id: "scope-a".to_string(),
            runtime_state_id: "runtime-a1".to_string(),
            conversation_epoch: Some(10),
            expected_scope_revision: 0,
            expected_state_version: RuntimeStateExpectedVersion::Absent,
            checkpoint: managed_checkpoint("runtime-a1", "one")?,
        };
        let first = store
            .compare_and_save_checkpoint(first_request.clone())
            .await?;
        let second = store
            .compare_and_save_checkpoint(RuntimeCheckpointCasRequest {
                scope_id: "scope-a".to_string(),
                runtime_state_id: "runtime-a2".to_string(),
                conversation_epoch: Some(10),
                expected_scope_revision: first.scope.revision,
                expected_state_version: RuntimeStateExpectedVersion::Absent,
                checkpoint: managed_checkpoint("runtime-a2", "two")?,
            })
            .await?;
        assert_eq!(
            store
                .compare_and_save_checkpoint(first_request)
                .await?
                .status,
            RuntimeCheckpointCasStatus::AlreadyCurrent
        );
        let delete = crate::memory::ManagedConversationDelete::prepare("scope-a", 10)?;
        let request = ScopeRetirementRequest::prepare("scope-a", second.scope.revision, &delete)?;
        let begun = store.begin_scope_retirement(request.clone()).await?;
        assert_eq!(begun.status, ScopeRetirementStatus::Begun);
        assert_eq!(begun.manifest.items.len(), 2);
        assert_eq!(begun.retention_floor_epoch, 0);

        let premature_drop = store
            .continue_scope_retirement(
                "scope-a",
                &request.delete_operation_id,
                begun.scope.revision,
                ScopeRetirementAdvance::GenerationDropped {
                    runtime_state_id: "runtime-a1".to_string(),
                    pending_operation_id: None,
                },
            )
            .await?;
        assert_eq!(premature_drop.status, ScopeRetirementStatus::InProgress);
        assert_eq!(premature_drop.scope.revision, begun.scope.revision);
        assert_eq!(premature_drop.retention_floor_epoch, 0);
        assert!(store.get_checkpoint("runtime-a1").await?.is_some());

        let expired = store
            .continue_scope_retirement(
                "scope-a",
                &request.delete_operation_id,
                begun.scope.revision,
                ScopeRetirementAdvance::ConversationDeleted {
                    receipt: crate::memory::ManagedConversationDeleteReceipt {
                        operation_id: request.delete_operation_id.clone(),
                        payload_digest: request.payload_digest.clone(),
                        deleted_epoch: 10,
                        retention_floor_epoch: 11,
                        status: crate::memory::ManagedConversationDeleteStatus::ReceiptExpired,
                    },
                },
            )
            .await?;
        assert_eq!(expired.status, ScopeRetirementStatus::ReceiptExpired);
        assert_eq!(expired.retention_floor_epoch, 11);

        let conversation = store
            .continue_scope_retirement(
                "scope-a",
                &request.delete_operation_id,
                begun.scope.revision,
                ScopeRetirementAdvance::ConversationDeleted {
                    receipt: crate::memory::ManagedConversationDeleteReceipt {
                        operation_id: request.delete_operation_id.clone(),
                        payload_digest: request.payload_digest.clone(),
                        deleted_epoch: 10,
                        retention_floor_epoch: 1,
                        status: crate::memory::ManagedConversationDeleteStatus::Deleted,
                    },
                },
            )
            .await?;
        let conversation_replay = store
            .continue_scope_retirement(
                "scope-a",
                &request.delete_operation_id,
                begun.scope.revision,
                ScopeRetirementAdvance::ConversationDeleted {
                    receipt: crate::memory::ManagedConversationDeleteReceipt {
                        operation_id: request.delete_operation_id.clone(),
                        payload_digest: request.payload_digest.clone(),
                        deleted_epoch: 10,
                        retention_floor_epoch: 2,
                        status: crate::memory::ManagedConversationDeleteStatus::AlreadyDeleted,
                    },
                },
            )
            .await?;
        assert_eq!(
            conversation_replay.scope.revision,
            conversation.scope.revision.saturating_add(1)
        );
        assert_eq!(conversation_replay.retention_floor_epoch, 2);
        let dropped_first = store
            .continue_scope_retirement(
                "scope-a",
                &request.delete_operation_id,
                conversation_replay.scope.revision,
                ScopeRetirementAdvance::GenerationDropped {
                    runtime_state_id: "runtime-a1".to_string(),
                    pending_operation_id: None,
                },
            )
            .await?;
        let replayed = store
            .continue_scope_retirement(
                "scope-a",
                &request.delete_operation_id,
                conversation_replay.scope.revision,
                ScopeRetirementAdvance::GenerationDropped {
                    runtime_state_id: "runtime-a1".to_string(),
                    pending_operation_id: None,
                },
            )
            .await?;
        assert_eq!(replayed.scope.revision, dropped_first.scope.revision);
        let dropped_second = store
            .continue_scope_retirement(
                "scope-a",
                &request.delete_operation_id,
                dropped_first.scope.revision,
                ScopeRetirementAdvance::GenerationDropped {
                    runtime_state_id: "runtime-a2".to_string(),
                    pending_operation_id: None,
                },
            )
            .await?;
        let completed = store
            .continue_scope_retirement(
                "scope-a",
                &request.delete_operation_id,
                dropped_second.scope.revision,
                ScopeRetirementAdvance::Complete,
            )
            .await?;
        assert_eq!(completed.status, ScopeRetirementStatus::Completed);
        assert_eq!(completed.scope.lifecycle, RuntimeScopeLifecycle::Tombstoned);

        let old_epoch_reopen = store
            .compare_and_save_checkpoint(RuntimeCheckpointCasRequest {
                scope_id: "scope-a".to_string(),
                runtime_state_id: "runtime-old-epoch".to_string(),
                conversation_epoch: Some(10),
                expected_scope_revision: completed.scope.revision,
                expected_state_version: RuntimeStateExpectedVersion::Absent,
                checkpoint: managed_checkpoint("runtime-old-epoch", "stale")?,
            })
            .await?;
        assert_eq!(
            old_epoch_reopen.status,
            RuntimeCheckpointCasStatus::ScopeFenced
        );

        let next = store
            .compare_and_save_checkpoint(RuntimeCheckpointCasRequest {
                scope_id: "scope-a".to_string(),
                runtime_state_id: "runtime-a3".to_string(),
                conversation_epoch: Some(11),
                expected_scope_revision: completed.scope.revision,
                expected_state_version: RuntimeStateExpectedVersion::Absent,
                checkpoint: managed_checkpoint("runtime-a3", "three")?,
            })
            .await?;
        assert_eq!(next.status, RuntimeCheckpointCasStatus::Applied);
        assert_eq!(next.scope.lifecycle, RuntimeScopeLifecycle::Active);
        assert_eq!(next.scope.conversation_epoch, Some(11));
        let lost_ack = store.begin_scope_retirement(request.clone()).await?;
        assert_eq!(lost_ack.status, ScopeRetirementStatus::AlreadyCompleted);
        assert_eq!(lost_ack.scope, completed.scope);
        lost_ack.validate()?;

        let second_delete = crate::memory::ManagedConversationDelete::prepare("scope-a", 11)?;
        let second_request =
            ScopeRetirementRequest::prepare("scope-a", next.scope.revision, &second_delete)?;
        let second_begun = store.begin_scope_retirement(second_request.clone()).await?;
        assert_eq!(second_begun.status, ScopeRetirementStatus::Begun);
        assert_eq!(second_begun.manifest.items.len(), 1);
        let second_conversation = store
            .continue_scope_retirement(
                "scope-a",
                &second_request.delete_operation_id,
                second_begun.scope.revision,
                ScopeRetirementAdvance::ConversationDeleted {
                    receipt: crate::memory::ManagedConversationDeleteReceipt {
                        operation_id: second_request.delete_operation_id.clone(),
                        payload_digest: second_request.payload_digest.clone(),
                        deleted_epoch: 11,
                        retention_floor_epoch: 10,
                        status: crate::memory::ManagedConversationDeleteStatus::Deleted,
                    },
                },
            )
            .await?;
        let second_dropped = store
            .continue_scope_retirement(
                "scope-a",
                &second_request.delete_operation_id,
                second_conversation.scope.revision,
                ScopeRetirementAdvance::GenerationDropped {
                    runtime_state_id: "runtime-a3".to_string(),
                    pending_operation_id: None,
                },
            )
            .await?;
        let second_completed = store
            .continue_scope_retirement(
                "scope-a",
                &second_request.delete_operation_id,
                second_dropped.scope.revision,
                ScopeRetirementAdvance::Complete,
            )
            .await?;
        assert_eq!(second_completed.status, ScopeRetirementStatus::Completed);
        let authority_before_old_replay = store
            .load_scope_authority("scope-a")
            .await?
            .ok_or_else(|| FileRuntimeStateStore::to_react_err("scope authority disappeared"))?;
        let expired_old_receipt = store.begin_scope_retirement(request.clone()).await?;
        assert_eq!(
            expired_old_receipt.status,
            ScopeRetirementStatus::ReceiptExpired
        );
        assert_eq!(expired_old_receipt.retention_floor_epoch, 10);
        assert_eq!(expired_old_receipt.scope, completed.scope);
        expired_old_receipt.validate()?;
        assert_eq!(
            store.load_scope_authority("scope-a").await?,
            Some(authority_before_old_replay.clone())
        );

        let mut invalid_request = request;
        invalid_request.payload_digest = "different-digest".to_string();
        assert!(store.begin_scope_retirement(invalid_request).await.is_err());
        assert_eq!(
            store.load_scope_authority("scope-a").await?,
            Some(authority_before_old_replay)
        );
        let stale_delete = crate::memory::ManagedConversationDelete::prepare("scope-a", 9)?;
        let stale_delete = store
            .begin_scope_retirement(ScopeRetirementRequest::prepare(
                "scope-a",
                second_completed.scope.revision,
                &stale_delete,
            )?)
            .await?;
        assert_eq!(stale_delete.status, ScopeRetirementStatus::EpochConflict);
        assert_eq!(
            store
                .load_scope_authority("scope-a")
                .await?
                .map(|scope| scope.conversation_epoch),
            Some(Some(11))
        );
        let _ = std::fs::remove_dir_all(&tmp);
        Ok(())
    }

    #[tokio::test]
    async fn owner_first_crash_cut_repairs_scope_revision() -> crate::error::Result<()> {
        let tmp = tmp_base();
        let store = FileRuntimeStateStore::new(&tmp)?;
        let applied = store
            .compare_and_save_checkpoint(RuntimeCheckpointCasRequest {
                scope_id: "scope-a".to_string(),
                runtime_state_id: "runtime-a".to_string(),
                conversation_epoch: Some(1),
                expected_scope_revision: 0,
                expected_state_version: RuntimeStateExpectedVersion::Absent,
                checkpoint: managed_checkpoint("runtime-a", "one")?,
            })
            .await?;
        assert_eq!(applied.scope.revision, 1);

        let mut owner = store
            .read_runtime_owner_sync("runtime-a")?
            .ok_or_else(|| ReactError::Other("runtime owner missing".to_string()))?;
        owner.state_version = Some(RuntimeStateVersion::Managed { revision: 2 });
        owner.scope_revision = 2;
        owner.checkpoint = Some(managed_checkpoint("runtime-a", "two")?);
        store.write_runtime_owner_sync(&owner)?;
        drop(store);

        let restarted = FileRuntimeStateStore::new(&tmp)?;
        let authority = restarted
            .load_scope_authority("scope-a")
            .await?
            .ok_or_else(|| ReactError::Other("scope authority missing".to_string()))?;
        assert_eq!(authority.revision, 2);
        let snapshot = restarted
            .load_runtime_state("scope-a", "runtime-a")
            .await?
            .ok_or_else(|| ReactError::Other("runtime state missing".to_string()))?;
        assert_eq!(
            snapshot.version,
            RuntimeStateVersion::Managed { revision: 2 }
        );
        assert_eq!(
            snapshot
                .checkpoint
                .map(|checkpoint| checkpoint.messages_json),
            Some(managed_checkpoint("runtime-a", "two")?.messages_json)
        );
        let _ = std::fs::remove_dir_all(&tmp);
        Ok(())
    }

    #[tokio::test]
    async fn legacy_deleting_cut_is_absent_to_revisioned_authority() -> crate::error::Result<()> {
        let tmp = tmp_base();
        let store = FileRuntimeStateStore::new(&tmp)?;
        store
            .save_checkpoint_for_scope("scope-a", &checkpoint("legacy-cut", "legacy"))
            .await?;
        assert!(store.mark_runtime_deleting_sync("scope-a", "legacy-cut")?);
        let legacy_path = store.runtime_owner_path("legacy-cut")?;
        drop(store);

        let restarted = FileRuntimeStateStore::new(&tmp)?;
        let applied = restarted
            .compare_and_save_checkpoint(RuntimeCheckpointCasRequest {
                scope_id: "scope-a".to_string(),
                runtime_state_id: "runtime-new".to_string(),
                conversation_epoch: Some(1),
                expected_scope_revision: 0,
                expected_state_version: RuntimeStateExpectedVersion::Absent,
                checkpoint: managed_checkpoint("runtime-new", "new")?,
            })
            .await?;
        assert_eq!(applied.status, RuntimeCheckpointCasStatus::Applied);
        assert!(
            restarted
                .load_runtime_state("scope-a", "legacy-cut")
                .await?
                .is_none()
        );
        assert!(!legacy_path.exists());
        assert_eq!(
            restarted.runtime_state_ids("scope-a").await?,
            vec!["runtime-new".to_string()]
        );
        let _ = std::fs::remove_dir_all(&tmp);
        Ok(())
    }

    #[tokio::test]
    async fn authoritative_scan_never_unlinks_a_legacy_reclaim() -> crate::error::Result<()> {
        let tmp = tmp_base();
        let store = FileRuntimeStateStore::new(&tmp)?;
        store
            .save_checkpoint_for_scope("scope-a", &checkpoint("shared", "legacy"))
            .await?;
        assert!(store.mark_runtime_deleting_sync("scope-a", "shared")?);
        let owner_path = store.runtime_owner_path("shared")?;
        assert!(store.runtime_records_sync()?.is_empty());
        assert!(owner_path.exists());

        store
            .save_checkpoint_for_scope("scope-b", &checkpoint("shared", "reclaimed"))
            .await?;
        let scanned = store.runtime_records_sync()?;
        assert_eq!(scanned.len(), 1);
        assert_eq!(
            scanned.first().map(|owner| owner.scope_id.as_str()),
            Some("scope-b")
        );
        assert_eq!(
            store
                .get_checkpoint("shared")
                .await?
                .map(|checkpoint| checkpoint.messages_json),
            Some(checkpoint("shared", "reclaimed").messages_json)
        );
        let _ = std::fs::remove_dir_all(&tmp);
        Ok(())
    }

    #[tokio::test]
    async fn generation_retirement_waits_for_pending_projection() -> crate::error::Result<()> {
        let tmp = tmp_base();
        let store = FileRuntimeStateStore::new(&tmp)?;
        let initial = store
            .compare_and_save_checkpoint(RuntimeCheckpointCasRequest {
                scope_id: "scope-p".to_string(),
                runtime_state_id: "runtime-p".to_string(),
                conversation_epoch: Some(1),
                expected_scope_revision: 0,
                expected_state_version: RuntimeStateExpectedVersion::Absent,
                checkpoint: managed_checkpoint("runtime-p", "initial")?,
            })
            .await?;
        let pending = store
            .compare_and_save_checkpoint(RuntimeCheckpointCasRequest {
                scope_id: "scope-p".to_string(),
                runtime_state_id: "runtime-p".to_string(),
                conversation_epoch: Some(1),
                expected_scope_revision: initial.scope.revision,
                expected_state_version: RuntimeStateExpectedVersion::Managed { revision: 1 },
                checkpoint: pending_checkpoint_at_revision("runtime-p", "scope-p", 2)?,
            })
            .await?;
        let retirement = store
            .retire_runtime_generation(RuntimeGenerationRetireRequest::prepare(
                "scope-p",
                "runtime-p",
                pending.scope.revision,
                RuntimeStateExpectedVersion::Managed { revision: 2 },
            )?)
            .await?;
        assert_eq!(
            retirement.status,
            RuntimeGenerationRetireStatus::PendingProjection
        );
        assert!(store.get_checkpoint("runtime-p").await?.is_some());
        let _ = std::fs::remove_dir_all(&tmp);
        Ok(())
    }

    #[tokio::test]
    async fn concurrent_generation_writers_cannot_overwrite_each_other() -> crate::error::Result<()>
    {
        let tmp = tmp_base();
        let first_store = FileRuntimeStateStore::new(&tmp)?;
        let second_store = FileRuntimeStateStore::new(&tmp)?;
        let barrier = Arc::new(tokio::sync::Barrier::new(3));
        let first_barrier = Arc::clone(&barrier);
        let first = tokio::spawn(async move {
            first_barrier.wait().await;
            first_store
                .compare_and_save_checkpoint(RuntimeCheckpointCasRequest {
                    scope_id: "scope-c".to_string(),
                    runtime_state_id: "runtime-c".to_string(),
                    conversation_epoch: Some(1),
                    expected_scope_revision: 0,
                    expected_state_version: RuntimeStateExpectedVersion::Absent,
                    checkpoint: managed_checkpoint("runtime-c", "first")?,
                })
                .await
        });
        let second_barrier = Arc::clone(&barrier);
        let second = tokio::spawn(async move {
            second_barrier.wait().await;
            second_store
                .compare_and_save_checkpoint(RuntimeCheckpointCasRequest {
                    scope_id: "scope-c".to_string(),
                    runtime_state_id: "runtime-c".to_string(),
                    conversation_epoch: Some(1),
                    expected_scope_revision: 0,
                    expected_state_version: RuntimeStateExpectedVersion::Absent,
                    checkpoint: managed_checkpoint("runtime-c", "second")?,
                })
                .await
        });
        barrier.wait().await;
        let first = first.await.map_err(FileRuntimeStateStore::to_react_err)??;
        let second = second
            .await
            .map_err(FileRuntimeStateStore::to_react_err)??;
        let statuses = [first.status, second.status];
        assert_eq!(
            statuses
                .iter()
                .filter(|status| **status == RuntimeCheckpointCasStatus::Applied)
                .count(),
            1
        );
        assert_eq!(
            statuses
                .iter()
                .filter(|status| **status == RuntimeCheckpointCasStatus::RevisionConflict)
                .count(),
            1
        );
        let authority = FileRuntimeStateStore::new(&tmp)?
            .load_scope_authority("scope-c")
            .await?
            .ok_or_else(|| ReactError::Other("concurrent scope missing".to_string()))?;
        assert_eq!(authority.revision, 1);
        let _ = std::fs::remove_dir_all(&tmp);
        Ok(())
    }

    #[tokio::test]
    async fn file_runtime_state_lifecycle() -> crate::error::Result<()> {
        let tmp = tmp_base();
        let store = FileRuntimeStateStore::new(&tmp)?;

        let checkpoint = AgentCheckpoint {
            conversation_id: "conv-1".to_string(),
            messages_json: "[]".to_string(),
            current_plan: Some("plan".to_string()),
            active_skills: vec!["coding".to_string()],
            blocked_reason: None,
            working_dir: None,
            timestamp: Utc::now(),
        };
        store.save_checkpoint(&checkpoint).await?;
        let cp = store
            .get_checkpoint("conv-1")
            .await?
            .ok_or_else(|| ReactError::Other("checkpoint missing after save".to_string()))?;
        assert_eq!(cp.active_skills, vec!["coding"]);

        store.clear_conversation("conv-1").await?;
        assert!(store.get_checkpoint("conv-1").await?.is_none());

        // clear on a never-existing conversation is a no-op.
        store.clear_conversation("never").await?;

        let _ = std::fs::remove_dir_all(&tmp);
        Ok(())
    }

    #[tokio::test]
    async fn scoped_incarnations_survive_restart_and_reset_is_sender_local()
    -> crate::error::Result<()> {
        let tmp = tmp_base();
        let store = FileRuntimeStateStore::new(&tmp)?;
        store
            .save_checkpoint_for_scope("alice", &checkpoint("alice-1", "alice one"))
            .await?;
        store
            .save_checkpoint_for_scope("alice", &checkpoint("alice-2", "alice two"))
            .await?;
        store
            .save_checkpoint_for_scope("bob", &checkpoint("bob-1", "bob one"))
            .await?;
        assert!(
            store
                .save_checkpoint_for_scope("bob", &checkpoint("alice-2", "wrong owner"))
                .await
                .is_err()
        );
        drop(store);

        let restarted = FileRuntimeStateStore::new(&tmp)?;
        assert_eq!(
            restarted.runtime_state_ids("alice").await?,
            vec!["alice-1".to_string(), "alice-2".to_string()]
        );
        assert!(
            restarted
                .clear_runtime_state("alice", "bob-1")
                .await
                .is_err()
        );
        assert!(
            restarted
                .clear_runtime_state("bob-1", "bob-1")
                .await
                .is_err()
        );
        assert!(
            restarted
                .clear_runtime_state_scope("bob-1")
                .await?
                .runtime_state_ids
                .is_empty()
        );
        assert!(restarted.clear_conversation("bob-1").await.is_err());
        assert!(restarted.get_checkpoint("bob-1").await?.is_some());
        let reset = restarted.clear_runtime_state("alice", "alice-1").await?;
        assert!(reset.checkpoint_removed);
        assert_eq!(
            restarted.runtime_state_ids("alice").await?,
            vec!["alice-2".to_string()]
        );
        assert!(restarted.get_checkpoint("alice-1").await?.is_none());
        assert!(restarted.get_checkpoint("alice-2").await?.is_some());
        assert_eq!(
            restarted
                .get_checkpoint("alice-2")
                .await?
                .map(|checkpoint| checkpoint.messages_json),
            Some(checkpoint("alice-2", "alice two").messages_json)
        );
        assert!(restarted.get_checkpoint("bob-1").await?.is_some());
        assert_eq!(
            restarted.runtime_state_ids("bob").await?,
            vec!["bob-1".to_string()]
        );

        restarted
            .save_checkpoint_for_scope("scope-a", &checkpoint("scope-a-1", "owned by a"))
            .await?;
        restarted
            .save_checkpoint_for_scope("scope-b", &checkpoint("scope-a", "owned by b"))
            .await?;
        let same_name = restarted.clear_runtime_state_scope("scope-a").await?;
        assert_eq!(same_name.runtime_state_ids, vec!["scope-a-1".to_string()]);
        assert!(restarted.get_checkpoint("scope-a-1").await?.is_none());
        assert!(restarted.get_checkpoint("scope-a").await?.is_some());
        assert_eq!(
            restarted.runtime_state_ids("scope-b").await?,
            vec!["scope-a".to_string()]
        );
        let _ = std::fs::remove_dir_all(&tmp);
        Ok(())
    }

    #[tokio::test]
    async fn concurrent_scopes_cannot_claim_one_runtime_identity() -> crate::error::Result<()> {
        let tmp = tmp_base();
        let store = FileRuntimeStateStore::new(&tmp)?;
        let barrier = std::sync::Arc::new(tokio::sync::Barrier::new(3));
        let first_store = store.clone();
        let first_barrier = std::sync::Arc::clone(&barrier);
        let first = tokio::spawn(async move {
            first_barrier.wait().await;
            first_store
                .save_checkpoint_for_scope("scope-a", &checkpoint("shared-runtime", "a"))
                .await
        });
        // A separately constructed handle for the same canonical root must
        // share the fixed process authority, not create per-ID lock files.
        let second_store = FileRuntimeStateStore::new(&tmp)?;
        let second_barrier = std::sync::Arc::clone(&barrier);
        let second = tokio::spawn(async move {
            second_barrier.wait().await;
            second_store
                .save_checkpoint_for_scope("scope-b", &checkpoint("shared-runtime", "b"))
                .await
        });
        barrier.wait().await;
        let first = first.await.map_err(FileRuntimeStateStore::to_react_err)?;
        let second = second.await.map_err(FileRuntimeStateStore::to_react_err)?;
        assert_ne!(first.is_ok(), second.is_ok());

        let scope_a = store.runtime_state_ids("scope-a").await?;
        let scope_b = store.runtime_state_ids("scope-b").await?;
        assert_eq!(scope_a.len().saturating_add(scope_b.len()), 1);
        assert!(
            scope_a.first().map(String::as_str) == Some("shared-runtime")
                || scope_b.first().map(String::as_str) == Some("shared-runtime")
        );
        assert!(store.get_checkpoint("shared-runtime").await?.is_some());
        assert!(!tmp.join("runtime_state").join("_locks").exists());
        let _ = std::fs::remove_dir_all(&tmp);
        Ok(())
    }

    #[tokio::test]
    async fn reset_keeps_transcript_and_product_delete_retires_all_incarnations()
    -> crate::error::Result<()> {
        use crate::memory::{
            ConversationStore, EnsureConversationProjectionRequest, FileConversationStore,
            NewConversation,
        };

        let tmp = tmp_base();
        let runtime = FileRuntimeStateStore::new(&tmp)?;
        let conversations = FileConversationStore::new(&tmp)?;
        for conversation_id in ["alice", "bob"] {
            conversations
                .ensure_conversation(NewConversation {
                    conversation_id: conversation_id.to_string(),
                    user_id: "default".to_string(),
                    agent_type: None,
                    title: None,
                })
                .await?;
            let messages = crate::memory::project_messages(
                conversation_id,
                &[crate::llm::types::Message::user(format!(
                    "{conversation_id} transcript"
                ))],
            )?;
            conversations
                .save_messages(conversation_id, &messages)
                .await?;
        }
        runtime
            .save_checkpoint_for_scope("alice", &checkpoint("alice-1", "one"))
            .await?;
        runtime
            .save_checkpoint_for_scope("alice", &checkpoint("alice-2", "two"))
            .await?;
        runtime
            .save_checkpoint_for_scope("bob", &checkpoint("bob-1", "bob"))
            .await?;

        for runtime_state_id in ["alice-1", "bob-1"] {
            conversations
                .ensure_conversation(NewConversation {
                    conversation_id: runtime_state_id.to_string(),
                    user_id: "default".to_string(),
                    agent_type: None,
                    title: None,
                })
                .await?;
            let incarnation_messages = crate::memory::project_messages(
                runtime_state_id,
                &[crate::llm::types::Message::user(format!(
                    "{runtime_state_id} incarnation-only transcript"
                ))],
            )?;
            conversations
                .save_messages(runtime_state_id, &incarnation_messages)
                .await?;
        }

        assert!(
            super::super::clear_persisted_runtime_incarnation(
                &conversations,
                &runtime,
                "alice",
                "bob-1",
            )
            .await
            .is_err()
        );
        assert!(conversations.get_conversation("bob-1").await?.is_some());

        super::super::clear_persisted_runtime_incarnation(
            &conversations,
            &runtime,
            "alice",
            "alice-1",
        )
        .await?;
        assert_eq!(conversations.count_messages("alice").await?, 1);
        assert!(conversations.get_conversation("alice-1").await?.is_none());

        let authority = conversations
            .ensure_projection_epoch(EnsureConversationProjectionRequest {
                conversation: NewConversation {
                    conversation_id: "alice".to_string(),
                    user_id: "default".to_string(),
                    agent_type: None,
                    title: None,
                },
                expected_tombstone_epoch: None,
            })
            .await?;
        let delete =
            crate::memory::ManagedConversationDelete::prepare("alice", authority.authority.epoch)?;
        let deleted =
            super::super::delete_persisted_conversation_managed(&conversations, &runtime, delete)
                .await?;
        assert_eq!(deleted.runtime_state_ids, vec!["alice-2".to_string()]);
        assert!(conversations.get_conversation("alice").await?.is_none());
        assert!(runtime.get_checkpoint("alice-1").await?.is_none());
        assert!(runtime.get_checkpoint("alice-2").await?.is_none());
        assert_eq!(
            runtime.runtime_state_ids("alice").await?,
            vec!["alice-1".to_string(), "alice-2".to_string()]
        );
        assert!(conversations.get_conversation("bob").await?.is_some());
        assert!(conversations.get_conversation("bob-1").await?.is_some());
        assert!(runtime.get_checkpoint("bob-1").await?.is_some());

        drop(conversations);
        let _ = std::fs::remove_dir_all(&tmp);
        Ok(())
    }

    #[tokio::test]
    async fn every_phase_cut_recovers_and_preserves_exact_owner() -> crate::error::Result<()> {
        let tmp = tmp_base();
        let store = FileRuntimeStateStore::new(&tmp)?;
        let active = checkpoint("active-cut", "active");
        store.write_runtime_owner_sync(&RuntimeStateOwner {
            version: LEGACY_RUNTIME_STATE_RECORD_VERSION,
            runtime_state_id: "active-cut".to_string(),
            scope_id: "scope-a".to_string(),
            phase: RuntimeStatePhase::Active,
            checkpoint: Some(active),
            state_version: None,
            conversation_epoch: None,
            scope_revision: 0,
            scope_lifecycle: RuntimeScopeLifecycle::Active,
            cas_expected_state_version: None,
            cas_expected_scope_revision: 0,
            transcript_ack_proof: None,
        })?;
        drop(store);

        let restarted = FileRuntimeStateStore::new(&tmp)?;
        assert_eq!(
            restarted.runtime_state_ids("scope-a").await?,
            vec!["active-cut".to_string()]
        );
        assert!(
            restarted
                .save_checkpoint_for_scope("scope-b", &checkpoint("active-cut", "wrong"))
                .await
                .is_err()
        );

        restarted
            .save_checkpoint_for_scope("scope-a", &checkpoint("deleting-cut", "delete"))
            .await?;
        restarted.mark_runtime_deleting_sync("scope-a", "deleting-cut")?;
        drop(restarted);
        let after_delete_mark = FileRuntimeStateStore::new(&tmp)?;
        assert!(
            after_delete_mark
                .get_checkpoint("deleting-cut")
                .await?
                .is_none()
        );
        assert_eq!(
            after_delete_mark.runtime_state_ids("scope-a").await?,
            vec!["active-cut".to_string()]
        );
        after_delete_mark
            .save_checkpoint_for_scope("scope-b", &checkpoint("deleting-cut", "reclaimed"))
            .await?;

        after_delete_mark
            .save_checkpoint_for_scope("scope-a", &checkpoint("deleting-claim", "delete"))
            .await?;
        after_delete_mark.mark_runtime_deleting_sync("scope-a", "deleting-claim")?;
        drop(after_delete_mark);
        let after_deleting_claim = FileRuntimeStateStore::new(&tmp)?;
        after_deleting_claim
            .save_checkpoint_for_scope("scope-b", &checkpoint("deleting-claim", "direct reclaim"))
            .await?;

        after_deleting_claim
            .save_checkpoint_for_scope("scope-a", &checkpoint("unlinked-cut", "unlink"))
            .await?;
        assert!(after_deleting_claim.remove_runtime_record_sync("unlinked-cut")?);
        drop(after_deleting_claim);
        let after_unlink = FileRuntimeStateStore::new(&tmp)?;
        assert_eq!(
            after_unlink.runtime_state_ids("scope-a").await?,
            vec!["active-cut".to_string()]
        );
        after_unlink
            .save_checkpoint_for_scope("scope-b", &checkpoint("unlinked-cut", "new owner"))
            .await?;
        assert!(after_unlink.get_checkpoint("active-cut").await?.is_some());
        assert!(after_unlink.get_checkpoint("deleting-cut").await?.is_some());
        assert!(
            after_unlink
                .get_checkpoint("deleting-claim")
                .await?
                .is_some()
        );
        assert!(after_unlink.get_checkpoint("unlinked-cut").await?.is_some());
        let _ = std::fs::remove_dir_all(&tmp);
        Ok(())
    }

    #[tokio::test]
    async fn corrupt_scope_projection_never_overrides_owner_authority() -> crate::error::Result<()>
    {
        let tmp = tmp_base();
        let store = FileRuntimeStateStore::new(&tmp)?;
        store
            .save_checkpoint_for_scope("scope-a", &checkpoint("runtime-a", "a"))
            .await?;
        let projection = store.scope_index_path("scope-a")?;
        echo_core::utils::fs::atomic_write(&projection, b"{ corrupt projection")
            .map_err(FileRuntimeStateStore::to_react_err)?;

        store
            .save_checkpoint_for_scope("scope-a", &checkpoint("runtime-b", "b"))
            .await?;
        assert_eq!(
            store.runtime_state_ids("scope-a").await?,
            vec!["runtime-a".to_string(), "runtime-b".to_string()]
        );

        echo_core::utils::fs::atomic_write(&projection, b"{ corrupt projection")
            .map_err(FileRuntimeStateStore::to_react_err)?;
        assert!(
            store
                .clear_runtime_state("scope-a", "runtime-a")
                .await?
                .checkpoint_removed
        );
        assert_eq!(
            store.runtime_state_ids("scope-a").await?,
            vec!["runtime-b".to_string()]
        );
        let _ = std::fs::remove_dir_all(&tmp);
        Ok(())
    }

    #[tokio::test]
    async fn unwritable_scope_projection_cannot_block_enumeration_or_delete()
    -> crate::error::Result<()> {
        let tmp = tmp_base();
        let store = FileRuntimeStateStore::new(&tmp)?;
        store
            .save_checkpoint_for_scope("scope-a", &checkpoint("runtime-a", "a"))
            .await?;
        let projection = store.scope_index_path("scope-a")?;
        let _removed =
            remove_file_durable(&projection).map_err(FileRuntimeStateStore::to_react_err)?;
        std::fs::create_dir(&projection).map_err(FileRuntimeStateStore::to_react_err)?;

        assert_eq!(
            store.runtime_state_ids("scope-a").await?,
            vec!["runtime-a".to_string()]
        );
        assert!(
            store
                .clear_runtime_state("scope-a", "runtime-a")
                .await?
                .checkpoint_removed
        );
        assert!(store.runtime_state_ids("scope-a").await?.is_empty());
        assert!(store.get_checkpoint("runtime-a").await?.is_none());
        let _ = std::fs::remove_dir_all(&tmp);
        Ok(())
    }

    #[tokio::test]
    async fn corrupt_checkpoint_surfaces_as_error() -> crate::error::Result<()> {
        let tmp = tmp_base();
        let store = FileRuntimeStateStore::new(&tmp)?;
        let path = store.checkpoint_path("c1")?;
        let parent = path.parent().ok_or_else(|| {
            ReactError::Other("checkpoint path has no parent directory".to_string())
        })?;
        std::fs::create_dir_all(parent).map_err(|error| ReactError::Other(error.to_string()))?;
        std::fs::write(path, b"{ not valid json")
            .map_err(|error| ReactError::Other(error.to_string()))?;
        let err = store
            .get_checkpoint("c1")
            .await
            .err()
            .ok_or_else(|| ReactError::Other("corrupt checkpoint was accepted".to_string()))?;
        assert!(err.to_string().contains("parse"));
        let _ = std::fs::remove_dir_all(&tmp);
        Ok(())
    }

    #[tokio::test]
    async fn path_traversal_conversation_id_is_rejected() -> crate::error::Result<()> {
        let tmp = tmp_base();
        let store = FileRuntimeStateStore::new(&tmp)?;
        let err = store
            .get_checkpoint("../escape")
            .await
            .err()
            .ok_or_else(|| ReactError::Other("unsafe conversation id was accepted".to_string()))?;
        assert!(err.to_string().contains("path segment") || err.to_string().contains("unsafe"));
        assert!(
            store
                .save_checkpoint_for_scope("../scope", &checkpoint("safe-runtime", "state"))
                .await
                .is_err()
        );
        assert!(
            store
                .clear_runtime_state("../scope", "safe-runtime")
                .await
                .is_err()
        );
        // No directory was created outside base.
        let parent = tmp
            .parent()
            .ok_or_else(|| ReactError::Other("temporary path has no parent".to_string()))?;
        assert!(!parent.join("escape").exists());
        let _ = std::fs::remove_dir_all(&tmp);
        Ok(())
    }

    #[tokio::test]
    async fn exact_utf8_ids_do_not_alias_on_case_folding_filesystems() -> crate::error::Result<()> {
        let tmp = tmp_base();
        let store = FileRuntimeStateStore::new(&tmp)?;
        let upper = checkpoint("A", "upper");
        let lower = checkpoint("a", "lower");
        let composed = checkpoint("é", "composed");
        let decomposed = checkpoint("e\u{301}", "decomposed");
        let paths = [
            store.checkpoint_path(&upper.conversation_id)?,
            store.checkpoint_path(&lower.conversation_id)?,
            store.checkpoint_path(&composed.conversation_id)?,
            store.checkpoint_path(&decomposed.conversation_id)?,
        ];
        let unique = paths.iter().collect::<std::collections::HashSet<_>>();
        assert_eq!(unique.len(), paths.len());

        tokio::try_join!(
            store.save_checkpoint(&upper),
            store.save_checkpoint(&lower),
            store.save_checkpoint(&composed),
            store.save_checkpoint(&decomposed),
        )?;
        for expected in [&upper, &lower, &composed, &decomposed] {
            let loaded = store
                .get_checkpoint(&expected.conversation_id)
                .await?
                .ok_or_else(|| ReactError::Other("aliased checkpoint is missing".to_string()))?;
            assert_eq!(loaded.conversation_id, expected.conversation_id);
            assert_eq!(loaded.messages_json, expected.messages_json);
        }
        let _ = std::fs::remove_dir_all(&tmp);
        Ok(())
    }

    #[tokio::test]
    async fn aborted_caller_does_not_cancel_accepted_checkpoint_write() -> crate::error::Result<()>
    {
        let tmp = tmp_base();
        let store = FileRuntimeStateStore::new(&tmp)?;
        let caller_store = store.clone();
        let checkpoint = checkpoint("abort", "committed");
        let checkpoint_for_write = checkpoint.clone();
        let (entered_tx, entered_rx) = tokio::sync::oneshot::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let caller = tokio::spawn(async move {
            caller_store
                .run_blocking("abort".to_string(), move |store, _| {
                    let _ignored = entered_tx.send(());
                    release_rx
                        .recv_timeout(Duration::from_secs(2))
                        .map_err(|error| {
                            ReactError::Other(format!("release checkpoint write: {error}"))
                        })?;
                    store.save_checkpoint_sync(&checkpoint_for_write)
                })
                .await
        });
        entered_rx
            .await
            .map_err(FileRuntimeStateStore::to_react_err)?;
        caller.abort();
        release_tx
            .send(())
            .map_err(FileRuntimeStateStore::to_react_err)?;

        let loaded = tokio::time::timeout(
            Duration::from_secs(2),
            store.get_checkpoint(&checkpoint.conversation_id),
        )
        .await
        .map_err(|_| ReactError::Other("accepted checkpoint write did not settle".to_string()))??
        .ok_or_else(|| ReactError::Other("accepted checkpoint write disappeared".to_string()))?;
        assert_eq!(loaded.messages_json, checkpoint.messages_json);
        let _ = std::fs::remove_dir_all(&tmp);
        Ok(())
    }

    #[tokio::test(flavor = "current_thread")]
    async fn save_clear_save_preserves_exact_fifo_after_caller_abort() -> crate::error::Result<()> {
        let tmp = tmp_base();
        let store = FileRuntimeStateStore::new(&tmp)?;
        let first_checkpoint = checkpoint("aba", "first");
        let final_checkpoint = checkpoint("aba", "final");
        let first_store = store.clone();
        let first_for_operation = first_checkpoint.clone();
        let (entered_tx, entered_rx) = tokio::sync::oneshot::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let first = tokio::spawn(async move {
            first_store
                .run_blocking("aba".to_string(), move |store, _| {
                    let _ignored = entered_tx.send(());
                    release_rx
                        .recv_timeout(Duration::from_secs(2))
                        .map_err(FileRuntimeStateStore::to_react_err)?;
                    store.save_checkpoint_sync(&first_for_operation)
                })
                .await
        });
        entered_rx
            .await
            .map_err(FileRuntimeStateStore::to_react_err)?;
        first.abort();

        let clear_store = store.clone();
        let clear = tokio::spawn(async move { clear_store.clear_conversation("aba").await });
        tokio::task::yield_now().await;
        assert!(!clear.is_finished());
        let final_store = store.clone();
        let final_for_operation = final_checkpoint.clone();
        let final_save =
            tokio::spawn(async move { final_store.save_checkpoint(&final_for_operation).await });
        tokio::task::yield_now().await;
        assert!(!clear.is_finished());
        assert!(!final_save.is_finished());

        release_tx
            .send(())
            .map_err(FileRuntimeStateStore::to_react_err)?;
        clear.await.map_err(FileRuntimeStateStore::to_react_err)??;
        final_save
            .await
            .map_err(FileRuntimeStateStore::to_react_err)??;
        let loaded = store
            .get_checkpoint("aba")
            .await?
            .ok_or_else(|| ReactError::Other("final checkpoint is missing".to_string()))?;
        assert_eq!(loaded.messages_json, final_checkpoint.messages_json);
        for _ in 0..16 {
            tokio::task::yield_now().await;
        }
        let stable = store
            .get_checkpoint("aba")
            .await?
            .ok_or_else(|| ReactError::Other("stable checkpoint is missing".to_string()))?;
        assert_eq!(stable.messages_json, final_checkpoint.messages_json);
        let _ = std::fs::remove_dir_all(&tmp);
        Ok(())
    }

    #[tokio::test(flavor = "current_thread")]
    async fn corrupt_read_and_clear_are_ordered_for_one_conversation() -> crate::error::Result<()> {
        let tmp = tmp_base();
        let store = FileRuntimeStateStore::new(&tmp)?;
        let path = store.checkpoint_path("corrupt")?;
        let parent = path.parent().ok_or_else(|| {
            ReactError::Other("checkpoint path has no parent directory".to_string())
        })?;
        std::fs::create_dir_all(parent).map_err(FileRuntimeStateStore::to_react_err)?;
        std::fs::write(&path, b"{ invalid json").map_err(FileRuntimeStateStore::to_react_err)?;

        let blocker_store = store.clone();
        let (entered_tx, entered_rx) = tokio::sync::oneshot::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let blocker = tokio::spawn(async move {
            blocker_store
                .run_blocking("corrupt".to_string(), move |_, _| {
                    let _ignored = entered_tx.send(());
                    release_rx
                        .recv_timeout(Duration::from_secs(2))
                        .map_err(FileRuntimeStateStore::to_react_err)
                })
                .await
        });
        entered_rx
            .await
            .map_err(FileRuntimeStateStore::to_react_err)?;
        let read_store = store.clone();
        let read = tokio::spawn(async move { read_store.get_checkpoint("corrupt").await });
        tokio::task::yield_now().await;
        let clear_store = store.clone();
        let clear = tokio::spawn(async move { clear_store.clear_conversation("corrupt").await });
        tokio::task::yield_now().await;
        release_tx
            .send(())
            .map_err(FileRuntimeStateStore::to_react_err)?;

        blocker
            .await
            .map_err(FileRuntimeStateStore::to_react_err)??;
        let read_error = read
            .await
            .map_err(FileRuntimeStateStore::to_react_err)?
            .err()
            .ok_or_else(|| ReactError::Other("corrupt checkpoint was accepted".to_string()))?;
        assert!(read_error.to_string().contains("parse"));
        let clear_error = clear
            .await
            .map_err(FileRuntimeStateStore::to_react_err)?
            .err()
            .ok_or_else(|| ReactError::Other("corrupt clear was accepted".to_string()))?;
        assert!(clear_error.to_string().contains("parse"));
        assert!(store.get_checkpoint("corrupt").await.is_err());
        let _removed = remove_file_durable(&path).map_err(FileRuntimeStateStore::to_react_err)?;
        let _ = std::fs::remove_dir_all(&tmp);
        Ok(())
    }

    #[tokio::test]
    async fn different_conversations_can_use_distinct_blocking_slots() -> crate::error::Result<()> {
        let tmp = tmp_base();
        let store = FileRuntimeStateStore::new(&tmp)?;
        let first_store = store.clone();
        let second_store = store.clone();
        let (first_entered_tx, first_entered_rx) = tokio::sync::oneshot::channel();
        let (second_entered_tx, second_entered_rx) = tokio::sync::oneshot::channel();
        let (first_release_tx, first_release_rx) = std::sync::mpsc::channel();
        let (second_release_tx, second_release_rx) = std::sync::mpsc::channel();
        let first = tokio::spawn(async move {
            first_store
                .run_blocking("first".to_string(), move |_, _| {
                    let _ignored = first_entered_tx.send(());
                    first_release_rx
                        .recv_timeout(Duration::from_secs(2))
                        .map_err(FileRuntimeStateStore::to_react_err)
                })
                .await
        });
        let second = tokio::spawn(async move {
            second_store
                .run_blocking("second".to_string(), move |_, _| {
                    let _ignored = second_entered_tx.send(());
                    second_release_rx
                        .recv_timeout(Duration::from_secs(2))
                        .map_err(FileRuntimeStateStore::to_react_err)
                })
                .await
        });
        tokio::time::timeout(Duration::from_secs(2), async {
            first_entered_rx
                .await
                .map_err(FileRuntimeStateStore::to_react_err)?;
            second_entered_rx
                .await
                .map_err(FileRuntimeStateStore::to_react_err)
        })
        .await
        .map_err(|_| ReactError::Other("distinct conversations were serialized".to_string()))??;
        first_release_tx
            .send(())
            .map_err(FileRuntimeStateStore::to_react_err)?;
        second_release_tx
            .send(())
            .map_err(FileRuntimeStateStore::to_react_err)?;
        first.await.map_err(FileRuntimeStateStore::to_react_err)??;
        second
            .await
            .map_err(FileRuntimeStateStore::to_react_err)??;
        let _ = std::fs::remove_dir_all(&tmp);
        Ok(())
    }
}
