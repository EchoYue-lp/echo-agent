//! SQLite-backed [`RuntimeStateStore`] implementation.

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
use crate::error::{Result, RuntimeStateError};
use echo_core::utils::blocking::{
    BlockingFileOperationKey, BlockingFileOperationScope, run_keyed_file_operation,
};
use futures::future::BoxFuture;
use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::path::Path;

const MAX_CONVERSATION_EPOCH: u64 = i64::MAX as u64;

#[derive(Clone, Debug, Serialize, Deserialize)]
struct SqliteRuntimeScopeRecord {
    authority: RuntimeScopeAuthority,
    current_retirement: Option<ScopeRetirementManifest>,
    completed_retirements: Vec<ScopeRetirementReceipt>,
}

#[derive(Clone, Debug)]
struct SqliteRuntimeStateRecord {
    runtime_state_id: String,
    scope_id: String,
    version: RuntimeStateVersion,
    conversation_epoch: Option<u64>,
    scope_revision: u64,
    scope_lifecycle: RuntimeScopeLifecycle,
    cas_expected_state_version: Option<RuntimeStateExpectedVersion>,
    cas_expected_scope_revision: u64,
    transcript_ack_proof: Option<SqliteRuntimeTranscriptAckProof>,
    checkpoint: Option<AgentCheckpoint>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
struct SqliteRuntimeTranscriptAckProof {
    operation_id: String,
    payload_digest: String,
}

/// SQLite-backed runtime checkpoint store.
#[derive(Clone)]
pub struct SqliteRuntimeStateStore {
    path: std::path::PathBuf,
    call_context: Option<crate::memory::PersistenceCallContext>,
}

impl SqliteRuntimeStateStore {
    /// Create a SQLite state store at `path`.
    pub fn new(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref().to_path_buf();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|error| {
                RuntimeStateError::Io(format!("failed to create directory: {error}"))
            })?;
        }
        let mut store = Self {
            path,
            call_context: None,
        };
        store.init_tables()?;
        store.path = std::fs::canonicalize(&store.path).map_err(|error| {
            RuntimeStateError::Io(format!("failed to canonicalize SQLite store: {error}"))
        })?;
        Ok(store)
    }

    fn open_conn(&self) -> Result<Connection> {
        let connection = Connection::open(&self.path).map_err(|error| {
            crate::error::ReactError::from(RuntimeStateError::Io(format!(
                "failed to open SQLite connection: {error}"
            )))
        })?;
        connection
            .busy_timeout(std::time::Duration::from_secs(5))
            .map_err(|error| {
                RuntimeStateError::Io(format!("failed to configure SQLite busy timeout: {error}"))
            })?;
        Ok(connection)
    }

    /// Initialize the checkpoint table.
    pub fn init_tables(&self) -> Result<()> {
        let conn = self.open_conn()?;
        conn.execute_batch(
            r#"
            CREATE TABLE IF NOT EXISTS agent_checkpoints (
                conversation_id TEXT PRIMARY KEY,
                messages_json   TEXT NOT NULL,
                current_plan    TEXT,
                active_skills   TEXT NOT NULL,
                blocked_reason  TEXT,
                working_dir     TEXT,
                timestamp       TEXT NOT NULL
            );
            CREATE TABLE IF NOT EXISTS runtime_state_scopes (
                scope_id         TEXT NOT NULL,
                runtime_state_id TEXT NOT NULL,
                PRIMARY KEY (scope_id, runtime_state_id)
            );
            CREATE INDEX IF NOT EXISTS idx_runtime_state_scopes_runtime
                ON runtime_state_scopes(runtime_state_id);
            CREATE UNIQUE INDEX IF NOT EXISTS uniq_runtime_state_scope_owner
                ON runtime_state_scopes(runtime_state_id);
            CREATE TABLE IF NOT EXISTS runtime_state_authorities (
                scope_id                      TEXT PRIMARY KEY,
                authority_json                TEXT NOT NULL,
                current_retirement_json       TEXT,
                completed_retirements_json    TEXT NOT NULL
            );
            CREATE TABLE IF NOT EXISTS runtime_state_versions (
                runtime_state_id       TEXT PRIMARY KEY,
                scope_id               TEXT NOT NULL,
                state_version_json      TEXT NOT NULL,
                conversation_epoch      INTEGER,
                scope_revision          TEXT NOT NULL,
                scope_lifecycle_json    TEXT NOT NULL,
                cas_expected_state_version_json TEXT,
                cas_expected_scope_revision TEXT,
                transcript_ack_proof_json TEXT
            );
            CREATE UNIQUE INDEX IF NOT EXISTS uniq_runtime_state_version_owner
                ON runtime_state_versions(runtime_state_id);
            CREATE INDEX IF NOT EXISTS idx_runtime_state_versions_scope
                ON runtime_state_versions(scope_id);
            "#,
        )
        .map_err(|error| RuntimeStateError::Io(format!("failed to init tables: {error}")))?;

        // Databases created before working-directory restoration lack this
        // column. SQLite has no `ADD COLUMN IF NOT EXISTS`, so only the exact
        // duplicate-column outcome is accepted.
        if let Err(error) = conn.execute(
            "ALTER TABLE agent_checkpoints ADD COLUMN working_dir TEXT",
            [],
        ) {
            let message = error.to_string();
            if !message.contains("duplicate column name") {
                return Err(RuntimeStateError::Io(format!(
                    "failed to add working_dir column: {message}"
                ))
                .into());
            }
        }
        for statement in [
            "ALTER TABLE runtime_state_versions ADD COLUMN cas_expected_state_version_json TEXT",
            "ALTER TABLE runtime_state_versions ADD COLUMN cas_expected_scope_revision TEXT",
            "ALTER TABLE runtime_state_versions ADD COLUMN transcript_ack_proof_json TEXT",
        ] {
            if let Err(error) = conn.execute(statement, []) {
                let message = error.to_string();
                if !message.contains("duplicate column name") {
                    return Err(RuntimeStateError::Io(format!(
                        "failed to migrate runtime-state CAS metadata: {message}"
                    ))
                    .into());
                }
            }
        }
        Ok(())
    }

    fn immediate_transaction(conn: &mut Connection) -> Result<rusqlite::Transaction<'_>> {
        conn.transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|error| {
                RuntimeStateError::Io(format!(
                    "failed to begin immediate runtime-state transaction: {error}"
                ))
                .into()
            })
    }

    fn managed_state_error(message: impl Into<String>) -> crate::error::ReactError {
        RuntimeStateError::ManagedStateRequiresCas(message.into()).into()
    }

    fn with_call_context(&self, context: crate::memory::PersistenceCallContext) -> Self {
        let mut store = self.clone();
        store.call_context = Some(context);
        store
    }

    fn ensure_call_deadline(
        context: crate::memory::PersistenceCallContext,
        operation: &str,
    ) -> Result<()> {
        context.ensure_not_expired().map_err(|error| match error {
            echo_core::error::MemoryError::DeadlineExceeded(_) => {
                RuntimeStateError::DeadlineExceeded(operation.to_string()).into()
            }
            other => RuntimeStateError::SerializationError(format!(
                "invalid persistence deadline for {operation}: {other}"
            ))
            .into(),
        })
    }

    fn ensure_active_deadline(&self, operation: &str) -> Result<()> {
        self.call_context.map_or(Ok(()), |context| {
            Self::ensure_call_deadline(context, operation)
        })
    }

    fn next_revision(current: u64, identity: &str) -> Result<u64> {
        current
            .checked_add(1)
            .ok_or_else(|| RuntimeStateError::RevisionExhausted(identity.to_string()).into())
    }

    fn checkpoint_digest(checkpoint: &AgentCheckpoint) -> Result<String> {
        let encoded = serde_json::to_vec(checkpoint).map_err(|error| {
            RuntimeStateError::SerializationError(format!(
                "failed to serialize checkpoint digest: {error}"
            ))
        })?;
        Ok(format!("{:x}", Sha256::digest(encoded)))
    }

    fn serialized_eq<T: Serialize>(left: &T, right: &T) -> Result<bool> {
        let left = serde_json::to_vec(left).map_err(|error| {
            RuntimeStateError::SerializationError(format!(
                "failed to serialize runtime payload: {error}"
            ))
        })?;
        let right = serde_json::to_vec(right).map_err(|error| {
            RuntimeStateError::SerializationError(format!(
                "failed to serialize runtime payload: {error}"
            ))
        })?;
        Ok(left == right)
    }

    fn validate_unmanaged_checkpoint(checkpoint: &AgentCheckpoint) -> Result<()> {
        let payload = checkpoint.restore_managed_runtime_payload()?;
        if payload.pending_transcript_projection.is_some() {
            return Err(Self::managed_state_error(
                "pending checkpoint payload requires compare-and-save",
            ));
        }
        Ok(())
    }

    fn encode<T: Serialize>(value: &T, label: &str) -> Result<String> {
        serde_json::to_string(value).map_err(|error| {
            RuntimeStateError::SerializationError(format!("failed to serialize {label}: {error}"))
                .into()
        })
    }

    fn decode<T: serde::de::DeserializeOwned>(value: &str, label: &str) -> Result<T> {
        serde_json::from_str(value).map_err(|error| {
            RuntimeStateError::SerializationError(format!("failed to deserialize {label}: {error}"))
                .into()
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

    fn validate_checkpoint_cas_request(request: &RuntimeCheckpointCasRequest) -> Result<()> {
        if request.scope_id.trim().is_empty() || request.runtime_state_id.trim().is_empty() {
            return Err(RuntimeStateError::SerializationError(
                "checkpoint CAS scope and runtime identities must not be empty".to_string(),
            )
            .into());
        }
        if request.checkpoint.conversation_id != request.runtime_state_id {
            return Err(RuntimeStateError::SerializationError(
                "checkpoint identity does not match CAS runtime identity".to_string(),
            )
            .into());
        }
        if !Self::epoch_is_valid(request.conversation_epoch) {
            return Err(RuntimeStateError::SerializationError(
                "conversation epoch is outside the supported range".to_string(),
            )
            .into());
        }
        let payload = request.checkpoint.restore_managed_runtime_payload()?;
        if let Some(pending) = payload.pending_transcript_projection.as_ref()
            && (pending.batch.conversation_id != request.scope_id
                || Some(pending.batch.conversation_epoch) != request.conversation_epoch
                || payload.transcript_projection.as_ref() != Some(&pending.cursor_before))
        {
            return Err(RuntimeStateError::SerializationError(
                "pending transcript projection does not match CAS scope, epoch, and cursor"
                    .to_string(),
            )
            .into());
        }
        Ok(())
    }

    fn validate_pending_transition(
        state: Option<&SqliteRuntimeStateRecord>,
        checkpoint: &AgentCheckpoint,
    ) -> Result<()> {
        let next = checkpoint.restore_managed_runtime_payload()?;
        let current = match state {
            Some(state) if matches!(state.version, RuntimeStateVersion::Managed { .. }) => Some(
                state
                    .checkpoint
                    .as_ref()
                    .ok_or_else(|| {
                        RuntimeStateError::SerializationError(format!(
                            "managed runtime state {} lost its checkpoint",
                            state.runtime_state_id
                        ))
                    })?
                    .restore_managed_runtime_payload()?,
            ),
            _ => None,
        };
        let current_revision = state
            .map(|state| Self::state_revision(&state.version))
            .unwrap_or(0);
        let resulting_revision =
            Self::next_revision(current_revision, &checkpoint.conversation_id)?;
        if let (Some(state), Some(current_pending)) = (
            state,
            current
                .as_ref()
                .and_then(|payload| payload.pending_transcript_projection.as_ref()),
        ) && current_pending.base_runtime_revision != Self::state_revision(&state.version)
        {
            return Err(RuntimeStateError::SerializationError(
                "current pending transcript revision does not match runtime authority".to_string(),
            )
            .into());
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
            (None, Some(_)) => Err(RuntimeStateError::SerializationError(
                "new pending transcript revision does not match resulting runtime revision"
                    .to_string(),
            )
            .into()),
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
                    && state.is_some_and(|state| {
                        state.checkpoint.as_ref().is_some_and(|current_checkpoint| {
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
        state: &SqliteRuntimeStateRecord,
        request: &RuntimeTranscriptAckRequest,
    ) -> Result<()> {
        let checkpoint = state.checkpoint.as_ref().ok_or_else(|| {
            RuntimeStateError::SerializationError(format!(
                "managed runtime state {} lost its checkpoint",
                state.runtime_state_id
            ))
        })?;
        let current = checkpoint.restore_managed_runtime_payload()?;
        let pending = current.pending_transcript_projection.ok_or_else(|| {
            Self::managed_state_error(
                "runtime transcript acknowledgement has no current pending projection",
            )
        })?;
        if pending.base_runtime_revision != Self::state_revision(&state.version) {
            return Err(RuntimeStateError::SerializationError(
                "current pending transcript revision does not match runtime authority".to_string(),
            )
            .into());
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
        state: &SqliteRuntimeStateRecord,
        request: &RuntimeCheckpointCasRequest,
    ) -> Result<bool> {
        Ok(matches!(state.version, RuntimeStateVersion::Managed { .. })
            && state.cas_expected_state_version.as_ref() == Some(&request.expected_state_version)
            && state.cas_expected_scope_revision == request.expected_scope_revision
            && request.expected_scope_revision.checked_add(1) == Some(state.scope_revision)
            && Self::checkpoint_is_current(state, &request.checkpoint)?)
    }

    fn is_direct_retirement_replay(
        state: &SqliteRuntimeStateRecord,
        request: &RuntimeGenerationRetireRequest,
    ) -> bool {
        matches!(
            &state.version,
            RuntimeStateVersion::Retired { operation_id, .. }
                if operation_id == &request.operation_id
        ) && state.cas_expected_state_version.as_ref() == Some(&request.expected_state_version)
            && state.cas_expected_scope_revision == request.expected_scope_revision
            && request.expected_scope_revision.checked_add(1) == Some(state.scope_revision)
    }

    fn load_checkpoint_on_connection(
        conn: &Connection,
        conversation_id: &str,
    ) -> Result<Option<AgentCheckpoint>> {
        let row = conn
            .query_row(
                "SELECT messages_json, current_plan, active_skills, blocked_reason, working_dir, timestamp
                 FROM agent_checkpoints WHERE conversation_id = ?1",
                params![conversation_id],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, Option<String>>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, Option<String>>(3)?,
                        row.get::<_, Option<String>>(4)?,
                        row.get::<_, String>(5)?,
                    ))
                },
            )
            .optional()
            .map_err(|error| {
                RuntimeStateError::Io(format!("failed to query checkpoint: {error}"))
            })?;
        let Some((
            messages_json,
            current_plan,
            active_skills_json,
            blocked_reason,
            working_dir,
            timestamp,
        )) = row
        else {
            return Ok(None);
        };
        let active_skills = serde_json::from_str(&active_skills_json).map_err(|error| {
            RuntimeStateError::SerializationError(format!(
                "invalid checkpoint active_skills: {error}"
            ))
        })?;
        let timestamp = chrono::DateTime::parse_from_rfc3339(&timestamp)
            .map_err(|error| {
                RuntimeStateError::SerializationError(format!(
                    "invalid checkpoint timestamp: {error}"
                ))
            })?
            .with_timezone(&chrono::Utc);
        Ok(Some(AgentCheckpoint {
            conversation_id: conversation_id.to_string(),
            messages_json,
            current_plan,
            active_skills,
            blocked_reason,
            working_dir: working_dir.map(std::path::PathBuf::from),
            timestamp,
        }))
    }

    fn load_scope_record_on_connection(
        conn: &Connection,
        scope_id: &str,
    ) -> Result<Option<SqliteRuntimeScopeRecord>> {
        let row = conn
            .query_row(
                "SELECT authority_json, current_retirement_json, completed_retirements_json
                 FROM runtime_state_authorities WHERE scope_id = ?1",
                params![scope_id],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, Option<String>>(1)?,
                        row.get::<_, String>(2)?,
                    ))
                },
            )
            .optional()
            .map_err(|error| {
                RuntimeStateError::Io(format!("failed to query runtime scope: {error}"))
            })?;
        let Some((authority, current, completed)) = row else {
            return Ok(None);
        };
        let authority: RuntimeScopeAuthority = Self::decode(&authority, "runtime scope")?;
        if authority.scope_id != scope_id || authority.revision == 0 {
            return Err(RuntimeStateError::SerializationError(format!(
                "runtime scope authority mismatch for {scope_id}"
            ))
            .into());
        }
        if !Self::epoch_is_valid(authority.conversation_epoch) {
            return Err(RuntimeStateError::SerializationError(format!(
                "runtime scope {scope_id} has an invalid conversation epoch"
            ))
            .into());
        }
        let current_retirement = current
            .as_deref()
            .map(|value| Self::decode(value, "scope retirement manifest"))
            .transpose()?;
        let record = SqliteRuntimeScopeRecord {
            authority,
            current_retirement,
            completed_retirements: Self::decode(&completed, "completed scope retirements")?,
        };
        Self::validate_scope_record(&record)?;
        Self::validate_retirement_tombstones_on_connection(conn, &record)?;
        Ok(Some(record))
    }

    fn save_scope_record_on_connection(
        conn: &Connection,
        record: &SqliteRuntimeScopeRecord,
    ) -> Result<()> {
        Self::validate_scope_record(record)?;
        Self::validate_retirement_tombstones_on_connection(conn, record)?;
        conn.execute(
            "INSERT INTO runtime_state_authorities
             (scope_id, authority_json, current_retirement_json, completed_retirements_json)
             VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(scope_id) DO UPDATE SET
                 authority_json = excluded.authority_json,
                 current_retirement_json = excluded.current_retirement_json,
                 completed_retirements_json = excluded.completed_retirements_json",
            params![
                &record.authority.scope_id,
                Self::encode(&record.authority, "runtime scope")?,
                record
                    .current_retirement
                    .as_ref()
                    .map(|manifest| Self::encode(manifest, "scope retirement manifest"))
                    .transpose()?,
                Self::encode(&record.completed_retirements, "completed scope retirements")?,
            ],
        )
        .map_err(|error| RuntimeStateError::Io(format!("failed to save runtime scope: {error}")))?;
        Ok(())
    }

    fn validate_scope_record(record: &SqliteRuntimeScopeRecord) -> Result<()> {
        match (
            record.authority.lifecycle,
            record.current_retirement.as_ref(),
        ) {
            (RuntimeScopeLifecycle::Retiring, Some(manifest)) => {
                if record.authority.conversation_epoch != Some(manifest.expected_conversation_epoch)
                {
                    return Err(RuntimeStateError::SerializationError(
                        "retiring runtime scope manifest epoch does not match its authority"
                            .to_string(),
                    )
                    .into());
                }
                manifest.validate(&record.authority.scope_id)?;
            }
            (RuntimeScopeLifecycle::Retiring, None) => {
                return Err(RuntimeStateError::SerializationError(
                    "retiring runtime scope is missing its manifest".to_string(),
                )
                .into());
            }
            (RuntimeScopeLifecycle::Active | RuntimeScopeLifecycle::Tombstoned, Some(_)) => {
                return Err(RuntimeStateError::SerializationError(
                    "non-retiring runtime scope must not retain a retirement manifest".to_string(),
                )
                .into());
            }
            (RuntimeScopeLifecycle::Active | RuntimeScopeLifecycle::Tombstoned, None) => {}
        }
        for receipt in &record.completed_retirements {
            if receipt.scope.scope_id != record.authority.scope_id {
                return Err(RuntimeStateError::SerializationError(
                    "completed retirement receipt belongs to a different scope".to_string(),
                )
                .into());
            }
            if receipt.status != ScopeRetirementStatus::Completed
                || receipt.scope.lifecycle != RuntimeScopeLifecycle::Tombstoned
                || receipt.scope.conversation_epoch
                    != Some(receipt.manifest.expected_conversation_epoch)
            {
                return Err(RuntimeStateError::SerializationError(
                    "completed retirement history contains a non-completed receipt".to_string(),
                )
                .into());
            }
            receipt.validate()?;
        }
        Ok(())
    }

    fn validate_retirement_tombstones_on_connection(
        conn: &Connection,
        record: &SqliteRuntimeScopeRecord,
    ) -> Result<()> {
        let manifests = record.current_retirement.iter().chain(
            record
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
                let state = Self::load_state_record_on_connection(conn, &item.runtime_state_id)?
                    .ok_or_else(|| {
                        RuntimeStateError::SerializationError(format!(
                            "dropped runtime state {} is missing its retirement tombstone",
                            item.runtime_state_id
                        ))
                    })?;
                if state.scope_id != record.authority.scope_id
                    || !matches!(
                        state.version,
                        RuntimeStateVersion::Retired { ref operation_id, .. }
                            if operation_id == &manifest.delete_operation_id
                    )
                {
                    return Err(RuntimeStateError::SerializationError(format!(
                        "dropped runtime state {} lacks the matching retirement tombstone",
                        item.runtime_state_id
                    ))
                    .into());
                }
            }
        }
        Ok(())
    }

    fn load_state_record_on_connection(
        conn: &Connection,
        runtime_state_id: &str,
    ) -> Result<Option<SqliteRuntimeStateRecord>> {
        let managed = conn
            .query_row(
                "SELECT scope_id, state_version_json, conversation_epoch, scope_revision,
                        scope_lifecycle_json, cas_expected_state_version_json,
                        cas_expected_scope_revision, transcript_ack_proof_json
                 FROM runtime_state_versions WHERE runtime_state_id = ?1",
                params![runtime_state_id],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, Option<u64>>(2)?,
                        row.get::<_, String>(3)?,
                        row.get::<_, String>(4)?,
                        row.get::<_, Option<String>>(5)?,
                        row.get::<_, Option<String>>(6)?,
                        row.get::<_, Option<String>>(7)?,
                    ))
                },
            )
            .optional()
            .map_err(|error| {
                RuntimeStateError::Io(format!("failed to query runtime version: {error}"))
            })?;
        if let Some((
            scope_id,
            version,
            epoch,
            scope_revision,
            lifecycle,
            cas_expected_state_version,
            cas_expected_scope_revision,
            transcript_ack_proof,
        )) = managed
        {
            let version: RuntimeStateVersion = Self::decode(&version, "runtime state version")?;
            let scope_revision = scope_revision.parse::<u64>().map_err(|error| {
                RuntimeStateError::SerializationError(format!(
                    "invalid runtime scope revision: {error}"
                ))
            })?;
            if version == RuntimeStateVersion::Absent {
                return Err(RuntimeStateError::SerializationError(format!(
                    "runtime state {runtime_state_id} persisted an absent version"
                ))
                .into());
            }
            if !Self::epoch_is_valid(epoch) {
                return Err(RuntimeStateError::SerializationError(format!(
                    "runtime state {runtime_state_id} has an invalid conversation epoch"
                ))
                .into());
            }
            let checkpoint = Self::load_checkpoint_on_connection(conn, runtime_state_id)?;
            if matches!(version, RuntimeStateVersion::Retired { .. }) && checkpoint.is_some() {
                return Err(RuntimeStateError::SerializationError(format!(
                    "retired runtime state {runtime_state_id} retained a checkpoint"
                ))
                .into());
            }
            if matches!(version, RuntimeStateVersion::Managed { .. }) && checkpoint.is_none() {
                return Err(RuntimeStateError::SerializationError(format!(
                    "managed runtime state {runtime_state_id} lost its checkpoint"
                ))
                .into());
            }
            let record = SqliteRuntimeStateRecord {
                runtime_state_id: runtime_state_id.to_string(),
                scope_id,
                version,
                conversation_epoch: epoch,
                scope_revision,
                scope_lifecycle: Self::decode(&lifecycle, "runtime scope lifecycle")?,
                cas_expected_state_version: cas_expected_state_version
                    .as_deref()
                    .map(|value| Self::decode(value, "runtime CAS predecessor"))
                    .transpose()?,
                cas_expected_scope_revision: cas_expected_scope_revision
                    .as_deref()
                    .map(|value| {
                        value.parse::<u64>().map_err(|error| {
                            crate::error::ReactError::from(RuntimeStateError::SerializationError(
                                format!("invalid runtime CAS scope predecessor: {error}"),
                            ))
                        })
                    })
                    .transpose()?
                    .unwrap_or(0),
                transcript_ack_proof: transcript_ack_proof
                    .as_deref()
                    .map(|value| Self::decode(value, "runtime transcript acknowledgement proof"))
                    .transpose()?,
                checkpoint,
            };
            Self::validate_state_record(&record)?;
            return Ok(Some(record));
        }
        let scope_id = conn
            .query_row(
                "SELECT scope_id FROM runtime_state_scopes WHERE runtime_state_id = ?1",
                params![runtime_state_id],
                |row| row.get::<_, String>(0),
            )
            .optional()
            .map_err(|error| {
                RuntimeStateError::Io(format!("failed to query legacy runtime owner: {error}"))
            })?;
        let checkpoint = Self::load_checkpoint_on_connection(conn, runtime_state_id)?;
        let Some(checkpoint) = checkpoint else {
            return Ok(None);
        };
        let scope_id = scope_id.unwrap_or_else(|| runtime_state_id.to_string());
        let record = SqliteRuntimeStateRecord {
            runtime_state_id: runtime_state_id.to_string(),
            scope_id,
            version: RuntimeStateVersion::Unmanaged {
                digest: Self::checkpoint_digest(&checkpoint)?,
            },
            conversation_epoch: None,
            scope_revision: 0,
            scope_lifecycle: RuntimeScopeLifecycle::Active,
            cas_expected_state_version: None,
            cas_expected_scope_revision: 0,
            transcript_ack_proof: None,
            checkpoint: Some(checkpoint),
        };
        Self::validate_state_record(&record)?;
        Ok(Some(record))
    }

    fn save_state_record_on_connection(
        conn: &Connection,
        record: &SqliteRuntimeStateRecord,
    ) -> Result<()> {
        Self::validate_state_record(record)?;
        conn.execute(
            "INSERT INTO runtime_state_scopes (scope_id, runtime_state_id) VALUES (?1, ?2)
             ON CONFLICT(scope_id, runtime_state_id) DO NOTHING",
            params![&record.scope_id, &record.runtime_state_id],
        )
        .map_err(|error| {
            RuntimeStateError::Io(format!("failed to bind managed runtime scope: {error}"))
        })?;
        conn.execute(
            "INSERT INTO runtime_state_versions
             (runtime_state_id, scope_id, state_version_json, conversation_epoch, scope_revision,
              scope_lifecycle_json, cas_expected_state_version_json, cas_expected_scope_revision,
              transcript_ack_proof_json)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
             ON CONFLICT(runtime_state_id) DO UPDATE SET
                 scope_id = excluded.scope_id,
                 state_version_json = excluded.state_version_json,
                 conversation_epoch = excluded.conversation_epoch,
                 scope_revision = excluded.scope_revision,
                 scope_lifecycle_json = excluded.scope_lifecycle_json,
                 cas_expected_state_version_json = excluded.cas_expected_state_version_json,
                 cas_expected_scope_revision = excluded.cas_expected_scope_revision,
                 transcript_ack_proof_json = excluded.transcript_ack_proof_json",
            params![
                &record.runtime_state_id,
                &record.scope_id,
                Self::encode(&record.version, "runtime state version")?,
                record.conversation_epoch,
                record.scope_revision.to_string(),
                Self::encode(&record.scope_lifecycle, "runtime scope lifecycle")?,
                record
                    .cas_expected_state_version
                    .as_ref()
                    .map(|version| Self::encode(version, "runtime CAS predecessor"))
                    .transpose()?,
                record.cas_expected_scope_revision.to_string(),
                record
                    .transcript_ack_proof
                    .as_ref()
                    .map(|proof| Self::encode(proof, "runtime transcript acknowledgement proof"))
                    .transpose()?,
            ],
        )
        .map_err(|error| {
            RuntimeStateError::Io(format!("failed to save managed runtime version: {error}"))
        })?;
        if let Some(checkpoint) = record.checkpoint.as_ref() {
            Self::save_checkpoint_on_connection(conn, checkpoint)?;
        } else {
            conn.execute(
                "DELETE FROM agent_checkpoints WHERE conversation_id = ?1",
                params![&record.runtime_state_id],
            )
            .map_err(|error| {
                RuntimeStateError::Io(format!("failed to clear retired checkpoint: {error}"))
            })?;
        }
        Ok(())
    }

    fn validate_state_record(record: &SqliteRuntimeStateRecord) -> Result<()> {
        match &record.version {
            RuntimeStateVersion::Unmanaged { .. } => {
                let checkpoint = record.checkpoint.as_ref().ok_or_else(|| {
                    RuntimeStateError::SerializationError(
                        "unmanaged runtime state lost its checkpoint".to_string(),
                    )
                })?;
                Self::validate_unmanaged_checkpoint(checkpoint)?;
            }
            RuntimeStateVersion::Managed { revision } => {
                let checkpoint = record.checkpoint.as_ref().ok_or_else(|| {
                    RuntimeStateError::SerializationError(
                        "managed runtime state lost its checkpoint".to_string(),
                    )
                })?;
                let payload = checkpoint.restore_managed_runtime_payload()?;
                if payload
                    .pending_transcript_projection
                    .as_ref()
                    .is_some_and(|pending| pending.base_runtime_revision != *revision)
                {
                    return Err(RuntimeStateError::SerializationError(
                        "pending transcript base revision does not match managed state".to_string(),
                    )
                    .into());
                }
            }
            RuntimeStateVersion::Retired { .. } => {
                if record.checkpoint.is_some() || record.transcript_ack_proof.is_some() {
                    return Err(RuntimeStateError::SerializationError(
                        "retired runtime state retained checkpoint or acknowledgement proof"
                            .to_string(),
                    )
                    .into());
                }
            }
            RuntimeStateVersion::Absent => {
                return Err(RuntimeStateError::SerializationError(
                    "runtime state persisted an absent version".to_string(),
                )
                .into());
            }
        }
        if let Some(proof) = record.transcript_ack_proof.as_ref() {
            let payload = record
                .checkpoint
                .as_ref()
                .ok_or_else(|| {
                    RuntimeStateError::SerializationError(
                        "acknowledged runtime state is missing checkpoint".to_string(),
                    )
                })?
                .restore_managed_runtime_payload()?;
            if proof.payload_digest.len() != 64
                || !proof
                    .payload_digest
                    .bytes()
                    .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
                || proof.operation_id
                    != format!("transcript-projection-v1:{}", proof.payload_digest)
                || !matches!(record.version, RuntimeStateVersion::Managed { .. })
                || payload.pending_transcript_projection.is_some()
                || payload.transcript_projection.is_none()
            {
                return Err(RuntimeStateError::SerializationError(
                    "runtime transcript acknowledgement proof is corrupt".to_string(),
                )
                .into());
            }
        }
        Ok(())
    }

    fn scope_state_records_on_connection(
        conn: &Connection,
        scope_id: &str,
    ) -> Result<Vec<SqliteRuntimeStateRecord>> {
        let mut statement = conn
            .prepare(
                "SELECT runtime_state_id FROM runtime_state_scopes
                 WHERE scope_id = ?1 ORDER BY runtime_state_id",
            )
            .map_err(|error| {
                RuntimeStateError::Io(format!("failed to prepare runtime scope query: {error}"))
            })?;
        let rows = statement
            .query_map(params![scope_id], |row| row.get::<_, String>(0))
            .map_err(|error| {
                RuntimeStateError::Io(format!("failed to query runtime scope: {error}"))
            })?;
        let mut records = Vec::new();
        for row in rows {
            let runtime_state_id = row.map_err(|error| {
                RuntimeStateError::Io(format!("failed to read runtime scope: {error}"))
            })?;
            let record = Self::load_state_record_on_connection(conn, &runtime_state_id)?
                .ok_or_else(|| {
                    RuntimeStateError::SerializationError(format!(
                        "runtime scope {scope_id} references missing state {runtime_state_id}"
                    ))
                })?;
            if record.scope_id != scope_id {
                return Err(RuntimeStateError::SerializationError(format!(
                    "runtime state {runtime_state_id} has conflicting scope ownership"
                ))
                .into());
            }
            records.push(record);
        }
        if !records
            .iter()
            .any(|record| record.runtime_state_id == scope_id)
        {
            let owner = conn
                .query_row(
                    "SELECT scope_id FROM runtime_state_scopes WHERE runtime_state_id = ?1",
                    params![scope_id],
                    |row| row.get::<_, String>(0),
                )
                .optional()
                .map_err(|error| {
                    RuntimeStateError::Io(format!(
                        "failed to inspect same-id legacy runtime state: {error}"
                    ))
                })?;
            if owner.is_none()
                && let Some(record) = Self::load_state_record_on_connection(conn, scope_id)?
            {
                records.push(record);
                records.sort_by(|left, right| left.runtime_state_id.cmp(&right.runtime_state_id));
            }
        }
        Ok(records)
    }

    fn authority_or_unmanaged(
        scope_id: &str,
        scope: Option<&SqliteRuntimeScopeRecord>,
        states: &[SqliteRuntimeStateRecord],
    ) -> RuntimeScopeAuthority {
        scope
            .map(|record| record.authority.clone())
            .or_else(|| {
                states
                    .iter()
                    .max_by_key(|state| state.scope_revision)
                    .map(|state| RuntimeScopeAuthority {
                        scope_id: scope_id.to_string(),
                        conversation_epoch: state.conversation_epoch,
                        revision: state.scope_revision,
                        lifecycle: state.scope_lifecycle,
                    })
            })
            .unwrap_or(RuntimeScopeAuthority {
                scope_id: scope_id.to_string(),
                conversation_epoch: None,
                revision: 0,
                lifecycle: RuntimeScopeLifecycle::Active,
            })
    }

    fn pending_operation_id(record: &SqliteRuntimeStateRecord) -> Result<Option<String>> {
        let Some(checkpoint) = record.checkpoint.as_ref() else {
            return Ok(None);
        };
        Ok(checkpoint
            .restore_managed_runtime_payload()?
            .pending_transcript_projection
            .map(|pending| pending.batch.operation_id))
    }

    fn checkpoint_is_current(
        record: &SqliteRuntimeStateRecord,
        checkpoint: &AgentCheckpoint,
    ) -> Result<bool> {
        match record.checkpoint.as_ref() {
            Some(current) => {
                Ok(Self::checkpoint_digest(current)? == Self::checkpoint_digest(checkpoint)?)
            }
            None => Ok(false),
        }
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
        record: &SqliteRuntimeScopeRecord,
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

    /// Delete a conversation checkpoint synchronously.
    pub fn clear_conversation_sync(&self, conversation_id: &str) -> Result<()> {
        let mut conn = self.open_conn()?;
        let transaction = Self::immediate_transaction(&mut conn)?;
        if Self::load_scope_record_on_connection(&transaction, conversation_id)?.is_some()
            || Self::load_state_record_on_connection(&transaction, conversation_id)?.is_some_and(
                |state| !matches!(state.version, RuntimeStateVersion::Unmanaged { .. }),
            )
        {
            return Err(Self::managed_state_error(format!(
                "runtime state {conversation_id} is revision managed"
            )));
        }
        let owner = transaction
            .query_row(
                "SELECT scope_id FROM runtime_state_scopes WHERE runtime_state_id = ?1",
                params![conversation_id],
                |row| row.get::<_, String>(0),
            )
            .map(Some)
            .or_else(|error| match error {
                rusqlite::Error::QueryReturnedNoRows => Ok(None),
                error => Err(error),
            })
            .map_err(|error| {
                RuntimeStateError::Io(format!("failed to inspect checkpoint owner: {error}"))
            })?;
        if let Some(owner) = owner.as_deref()
            && owner != conversation_id
        {
            return Err(RuntimeStateError::SerializationError(format!(
                "runtime state {conversation_id} belongs to scope {owner}, not {conversation_id}"
            ))
            .into());
        }
        transaction
            .execute(
                "DELETE FROM agent_checkpoints WHERE conversation_id = ?1",
                params![conversation_id],
            )
            .map_err(|error| {
                RuntimeStateError::Io(format!("failed to clear checkpoint: {error}"))
            })?;
        transaction
            .execute(
                "DELETE FROM runtime_state_scopes WHERE runtime_state_id = ?1",
                params![conversation_id],
            )
            .map_err(|error| {
                RuntimeStateError::Io(format!("failed to clear checkpoint scope binding: {error}"))
            })?;
        transaction.commit().map_err(|error| {
            RuntimeStateError::Io(format!("failed to commit checkpoint clear: {error}"))
        })?;
        Ok(())
    }

    fn save_checkpoint_on_connection(
        conn: &Connection,
        checkpoint: &AgentCheckpoint,
    ) -> Result<()> {
        let active_skills = serde_json::to_string(&checkpoint.active_skills).map_err(|error| {
            RuntimeStateError::SerializationError(format!(
                "failed to serialize checkpoint active_skills: {error}"
            ))
        })?;
        conn.execute(
            r#"
            INSERT INTO agent_checkpoints (conversation_id, messages_json, current_plan, active_skills, blocked_reason, working_dir, timestamp)
            VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
            ON CONFLICT(conversation_id) DO UPDATE SET
                messages_json = excluded.messages_json,
                current_plan = excluded.current_plan,
                active_skills = excluded.active_skills,
                blocked_reason = excluded.blocked_reason,
                working_dir = excluded.working_dir,
                timestamp = excluded.timestamp
            "#,
            params![
                &checkpoint.conversation_id,
                &checkpoint.messages_json,
                checkpoint.current_plan.as_deref(),
                active_skills,
                checkpoint.blocked_reason.as_deref(),
                checkpoint.working_dir.as_ref().and_then(|path| path.to_str()),
                crate::utils::time::to_local(checkpoint.timestamp).to_rfc3339(),
            ],
        )
        .map_err(|error| {
            RuntimeStateError::Io(format!("failed to save checkpoint: {error}"))
        })?;
        Ok(())
    }

    fn run_blocking<'a, T, F>(
        &'a self,
        scope: BlockingFileOperationScope,
        operation: F,
    ) -> BoxFuture<'a, Result<T>>
    where
        T: Send + 'static,
        F: FnOnce(Self) -> Result<T> + Send + 'static,
    {
        let store = self.clone();
        Box::pin(async move {
            store.ensure_active_deadline("SQLite runtime-state admission")?;
            let key =
                BlockingFileOperationKey::new("runtime-state-sqlite", store.path.clone(), scope);
            run_keyed_file_operation(key, move || {
                store.ensure_active_deadline("SQLite runtime-state durable work")?;
                operation(store)
            })
            .await
            .map_err(|error| RuntimeStateError::Io(error.to_string()))?
        })
    }

    fn entity_scope(value: &str) -> BlockingFileOperationScope {
        BlockingFileOperationScope::Entity(echo_core::utils::fs::encode_utf8_path_identity(value))
    }

    fn collection_scope(value: &str) -> BlockingFileOperationScope {
        BlockingFileOperationScope::Collection(echo_core::utils::fs::encode_utf8_path_identity(
            value,
        ))
    }
}

impl RuntimeStateStore for SqliteRuntimeStateStore {
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
    ) -> BoxFuture<'a, Result<Option<ManagedRuntimeStateSnapshot>>> {
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
    ) -> BoxFuture<'a, Result<Option<RuntimeScopeAuthority>>> {
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
    ) -> BoxFuture<'a, Result<RuntimeCheckpointCasReceipt>> {
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
    ) -> BoxFuture<'a, Result<RuntimeCheckpointCasReceipt>> {
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
    ) -> BoxFuture<'a, Result<RuntimeGenerationRetireReceipt>> {
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
    ) -> BoxFuture<'a, Result<ScopeRetirementReceipt>> {
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
    ) -> BoxFuture<'a, Result<ScopeRetirementReceipt>> {
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
    ) -> BoxFuture<'a, Result<Option<ManagedRuntimeStateSnapshot>>> {
        let scope_id = scope_id.to_string();
        let runtime_state_id = runtime_state_id.to_string();
        self.run_blocking(Self::entity_scope(&runtime_state_id), move |store| {
            let mut conn = store.open_conn()?;
            store.ensure_active_deadline("SQLite runtime-state load before transaction")?;
            let transaction = Self::immediate_transaction(&mut conn)?;
            store.ensure_active_deadline("SQLite runtime-state load after transaction lock")?;
            let Some(state) =
                Self::load_state_record_on_connection(&transaction, &runtime_state_id)?
            else {
                return Ok(None);
            };
            if state.scope_id != scope_id {
                return Err(RuntimeStateError::SerializationError(format!(
                    "runtime state {runtime_state_id} belongs to scope {}, not {scope_id}",
                    state.scope_id
                ))
                .into());
            }
            let scope_record = Self::load_scope_record_on_connection(&transaction, &scope_id)?;
            let scope = Self::authority_or_unmanaged(
                &scope_id,
                scope_record.as_ref(),
                std::slice::from_ref(&state),
            );
            let snapshot = ManagedRuntimeStateSnapshot {
                scope,
                runtime_state_id,
                version: state.version,
                checkpoint: state.checkpoint,
            };
            transaction.commit().map_err(|error| {
                RuntimeStateError::Io(format!("failed to commit runtime-state load: {error}"))
            })?;
            Ok(Some(snapshot))
        })
    }

    fn load_scope_authority<'a>(
        &'a self,
        scope_id: &'a str,
    ) -> BoxFuture<'a, Result<Option<RuntimeScopeAuthority>>> {
        let scope_id = scope_id.to_string();
        self.run_blocking(Self::collection_scope(&scope_id), move |store| {
            let mut conn = store.open_conn()?;
            store.ensure_active_deadline("SQLite runtime scope load before transaction")?;
            let transaction = Self::immediate_transaction(&mut conn)?;
            store.ensure_active_deadline("SQLite runtime scope load after transaction lock")?;
            let authority = if let Some(record) =
                Self::load_scope_record_on_connection(&transaction, &scope_id)?
            {
                Some(record.authority)
            } else {
                let states = Self::scope_state_records_on_connection(&transaction, &scope_id)?;
                (!states.is_empty()).then(|| Self::authority_or_unmanaged(&scope_id, None, &states))
            };
            transaction.commit().map_err(|error| {
                RuntimeStateError::Io(format!("failed to commit runtime scope load: {error}"))
            })?;
            Ok(authority)
        })
    }

    fn compare_and_save_checkpoint<'a>(
        &'a self,
        request: RuntimeCheckpointCasRequest,
    ) -> BoxFuture<'a, Result<RuntimeCheckpointCasReceipt>> {
        if let Err(error) = Self::validate_checkpoint_cas_request(&request) {
            return Box::pin(async move { Err(error) });
        }
        self.run_blocking(Self::collection_scope(&request.scope_id), move |store| {
            let mut conn = store.open_conn()?;
            store.ensure_active_deadline("SQLite runtime checkpoint CAS before transaction")?;
            let transaction = Self::immediate_transaction(&mut conn)?;
            store.ensure_active_deadline("SQLite runtime checkpoint CAS after transaction lock")?;
            let mut scope_record =
                Self::load_scope_record_on_connection(&transaction, &request.scope_id)?;
            let state =
                Self::load_state_record_on_connection(&transaction, &request.runtime_state_id)?;
            if let Some(state) = state.as_ref()
                && state.scope_id != request.scope_id
            {
                return Err(RuntimeStateError::SerializationError(format!(
                    "runtime state {} already belongs to scope {}",
                    request.runtime_state_id, state.scope_id
                ))
                .into());
            }
            let states = state.iter().cloned().collect::<Vec<_>>();
            let scope =
                Self::authority_or_unmanaged(&request.scope_id, scope_record.as_ref(), &states);
            let current_version = state.as_ref().map(|state| state.version.clone());
            if matches!(current_version, Some(RuntimeStateVersion::Retired { .. })) {
                return Ok(RuntimeCheckpointCasReceipt {
                    scope,
                    runtime_state_id: request.runtime_state_id,
                    version: current_version
                        .clone()
                        .unwrap_or(RuntimeStateVersion::Absent),
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
                return Ok(RuntimeCheckpointCasReceipt {
                    scope,
                    runtime_state_id: request.runtime_state_id,
                    version: current_version.unwrap_or(RuntimeStateVersion::Absent),
                    status: RuntimeCheckpointCasStatus::ScopeFenced,
                });
            }
            if let Some(state) = state.as_ref()
                && Self::is_direct_cas_replay(state, &request)?
            {
                return Ok(RuntimeCheckpointCasReceipt {
                    scope,
                    runtime_state_id: request.runtime_state_id,
                    version: state.version.clone(),
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
                    runtime_state_id: request.runtime_state_id,
                    version: current_version.unwrap_or(RuntimeStateVersion::Absent),
                    status: RuntimeCheckpointCasStatus::RevisionConflict,
                });
            }
            Self::validate_pending_transition(state.as_ref(), &request.checkpoint)?;
            let state_revision = Self::next_revision(
                current_version
                    .as_ref()
                    .map(Self::state_revision)
                    .unwrap_or(0),
                &request.runtime_state_id,
            )?;
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
            Self::save_state_record_on_connection(
                &transaction,
                &SqliteRuntimeStateRecord {
                    runtime_state_id: request.runtime_state_id.clone(),
                    scope_id: request.scope_id.clone(),
                    version: version.clone(),
                    conversation_epoch: request.conversation_epoch,
                    scope_revision,
                    scope_lifecycle: RuntimeScopeLifecycle::Active,
                    cas_expected_state_version: Some(request.expected_state_version),
                    cas_expected_scope_revision: request.expected_scope_revision,
                    transcript_ack_proof: None,
                    checkpoint: Some(request.checkpoint),
                },
            )?;
            Self::save_scope_record_on_connection(
                &transaction,
                &SqliteRuntimeScopeRecord {
                    authority: authority.clone(),
                    current_retirement: None,
                    completed_retirements: scope_record
                        .take()
                        .map(|record| record.completed_retirements)
                        .unwrap_or_default(),
                },
            )?;
            transaction.commit().map_err(|error| {
                RuntimeStateError::Io(format!("failed to commit runtime checkpoint CAS: {error}"))
            })?;
            Ok(RuntimeCheckpointCasReceipt {
                scope: authority,
                runtime_state_id: request.runtime_state_id,
                version,
                status: RuntimeCheckpointCasStatus::Applied,
            })
        })
    }

    fn acknowledge_transcript_projection<'a>(
        &'a self,
        request: RuntimeTranscriptAckRequest,
    ) -> BoxFuture<'a, Result<RuntimeCheckpointCasReceipt>> {
        if let Err(error) = request.validate() {
            return Box::pin(async move { Err(error) });
        }
        self.run_blocking(Self::collection_scope(&request.scope_id), move |store| {
            let mut conn = store.open_conn()?;
            store.ensure_active_deadline(
                "SQLite runtime transcript acknowledgement before transaction",
            )?;
            let transaction = Self::immediate_transaction(&mut conn)?;
            store.ensure_active_deadline(
                "SQLite runtime transcript acknowledgement after transaction lock",
            )?;
            let mut scope_record =
                Self::load_scope_record_on_connection(&transaction, &request.scope_id)?;
            let state =
                Self::load_state_record_on_connection(&transaction, &request.runtime_state_id)?;
            if let Some(state) = state.as_ref()
                && state.scope_id != request.scope_id
            {
                return Err(RuntimeStateError::SerializationError(format!(
                    "runtime state {} belongs to scope {}, not {}",
                    request.runtime_state_id, state.scope_id, request.scope_id
                ))
                .into());
            }
            let states = state.iter().cloned().collect::<Vec<_>>();
            let scope =
                Self::authority_or_unmanaged(&request.scope_id, scope_record.as_ref(), &states);
            let current_version = state.as_ref().map(|state| state.version.clone());
            if matches!(current_version, Some(RuntimeStateVersion::Retired { .. })) {
                return Ok(RuntimeCheckpointCasReceipt {
                    scope,
                    runtime_state_id: request.runtime_state_id,
                    version: current_version.unwrap_or(RuntimeStateVersion::Absent),
                    status: RuntimeCheckpointCasStatus::GenerationRetired,
                });
            }
            if scope.lifecycle != RuntimeScopeLifecycle::Active
                || scope.conversation_epoch != Some(request.conversation_epoch)
            {
                return Ok(RuntimeCheckpointCasReceipt {
                    scope,
                    runtime_state_id: request.runtime_state_id,
                    version: current_version.unwrap_or(RuntimeStateVersion::Absent),
                    status: RuntimeCheckpointCasStatus::ScopeFenced,
                });
            }
            if let Some(state) = state.as_ref()
                && state.cas_expected_state_version.as_ref()
                    == Some(&request.expected_state_version)
                && state.cas_expected_scope_revision == request.expected_scope_revision
                && request.expected_scope_revision.checked_add(1) == Some(state.scope_revision)
                && state.transcript_ack_proof.as_ref()
                    == Some(&SqliteRuntimeTranscriptAckProof {
                        operation_id: request.projection_receipt.operation_id.clone(),
                        payload_digest: request.projection_receipt.payload_digest.clone(),
                    })
                && Self::checkpoint_is_current(state, &request.checkpoint)?
            {
                return Ok(RuntimeCheckpointCasReceipt {
                    scope,
                    runtime_state_id: request.runtime_state_id,
                    version: state.version.clone(),
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
                    runtime_state_id: request.runtime_state_id,
                    version: current_version.unwrap_or(RuntimeStateVersion::Absent),
                    status: RuntimeCheckpointCasStatus::RevisionConflict,
                });
            }
            let state = state.as_ref().ok_or_else(|| {
                RuntimeStateError::NotFound(format!(
                    "runtime state {} is absent",
                    request.runtime_state_id
                ))
            })?;
            Self::validate_transcript_ack(state, &request)?;
            let state_revision = Self::next_revision(
                current_version
                    .as_ref()
                    .map(Self::state_revision)
                    .unwrap_or(0),
                &request.runtime_state_id,
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
            Self::save_state_record_on_connection(
                &transaction,
                &SqliteRuntimeStateRecord {
                    runtime_state_id: request.runtime_state_id.clone(),
                    scope_id: request.scope_id.clone(),
                    version: version.clone(),
                    conversation_epoch: Some(request.conversation_epoch),
                    scope_revision,
                    scope_lifecycle: RuntimeScopeLifecycle::Active,
                    cas_expected_state_version: Some(request.expected_state_version),
                    cas_expected_scope_revision: request.expected_scope_revision,
                    transcript_ack_proof: Some(SqliteRuntimeTranscriptAckProof {
                        operation_id: request.projection_receipt.operation_id,
                        payload_digest: request.projection_receipt.payload_digest,
                    }),
                    checkpoint: Some(request.checkpoint),
                },
            )?;
            Self::save_scope_record_on_connection(
                &transaction,
                &SqliteRuntimeScopeRecord {
                    authority: authority.clone(),
                    current_retirement: None,
                    completed_retirements: scope_record
                        .take()
                        .map(|record| record.completed_retirements)
                        .unwrap_or_default(),
                },
            )?;
            transaction.commit().map_err(|error| {
                RuntimeStateError::Io(format!(
                    "failed to commit runtime transcript acknowledgement: {error}"
                ))
            })?;
            Ok(RuntimeCheckpointCasReceipt {
                scope: authority,
                runtime_state_id: request.runtime_state_id,
                version,
                status: RuntimeCheckpointCasStatus::Applied,
            })
        })
    }

    fn retire_runtime_generation<'a>(
        &'a self,
        request: RuntimeGenerationRetireRequest,
    ) -> BoxFuture<'a, Result<RuntimeGenerationRetireReceipt>> {
        if let Err(error) = request.validate() {
            return Box::pin(async move { Err(error) });
        }
        self.run_blocking(Self::collection_scope(&request.scope_id), move |store| {
            let mut conn = store.open_conn()?;
            store.ensure_active_deadline(
                "SQLite runtime generation retirement before transaction",
            )?;
            let transaction = Self::immediate_transaction(&mut conn)?;
            store.ensure_active_deadline(
                "SQLite runtime generation retirement after transaction lock",
            )?;
            let mut scope_record =
                Self::load_scope_record_on_connection(&transaction, &request.scope_id)?;
            let state =
                Self::load_state_record_on_connection(&transaction, &request.runtime_state_id)?;
            if let Some(state) = state.as_ref()
                && state.scope_id != request.scope_id
            {
                return Err(RuntimeStateError::SerializationError(format!(
                    "runtime state {} belongs to scope {}",
                    request.runtime_state_id, state.scope_id
                ))
                .into());
            }
            let states = state.iter().cloned().collect::<Vec<_>>();
            let scope =
                Self::authority_or_unmanaged(&request.scope_id, scope_record.as_ref(), &states);
            let current_version = state.as_ref().map(|state| state.version.clone());
            if matches!(current_version, Some(RuntimeStateVersion::Retired { .. })) {
                let is_replay = state
                    .as_ref()
                    .is_some_and(|state| Self::is_direct_retirement_replay(state, &request));
                return Ok(RuntimeGenerationRetireReceipt {
                    scope,
                    runtime_state_id: request.runtime_state_id,
                    version: current_version
                        .clone()
                        .unwrap_or(RuntimeStateVersion::Absent),
                    status: if is_replay {
                        RuntimeGenerationRetireStatus::AlreadyRetired
                    } else {
                        RuntimeGenerationRetireStatus::Conflict
                    },
                });
            }
            if scope.lifecycle != RuntimeScopeLifecycle::Active {
                return Ok(RuntimeGenerationRetireReceipt {
                    scope,
                    runtime_state_id: request.runtime_state_id,
                    version: current_version.unwrap_or(RuntimeStateVersion::Absent),
                    status: RuntimeGenerationRetireStatus::ScopeFenced,
                });
            }
            if let Some(state) = state.as_ref()
                && Self::pending_operation_id(state)?.is_some()
            {
                return Ok(RuntimeGenerationRetireReceipt {
                    scope,
                    runtime_state_id: request.runtime_state_id,
                    version: state.version.clone(),
                    status: RuntimeGenerationRetireStatus::PendingProjection,
                });
            }
            if scope.revision != request.expected_scope_revision
                || !Self::state_matches_expected(
                    current_version.as_ref(),
                    &request.expected_state_version,
                )
            {
                return Ok(RuntimeGenerationRetireReceipt {
                    scope,
                    runtime_state_id: request.runtime_state_id,
                    version: current_version.unwrap_or(RuntimeStateVersion::Absent),
                    status: RuntimeGenerationRetireStatus::Conflict,
                });
            }
            let state_revision = Self::next_revision(
                current_version
                    .as_ref()
                    .map(Self::state_revision)
                    .unwrap_or(0),
                &request.runtime_state_id,
            )?;
            let scope_revision = Self::next_revision(scope.revision, &request.scope_id)?;
            let version = RuntimeStateVersion::Retired {
                revision: state_revision,
                operation_id: request.operation_id,
            };
            Self::save_state_record_on_connection(
                &transaction,
                &SqliteRuntimeStateRecord {
                    runtime_state_id: request.runtime_state_id.clone(),
                    scope_id: request.scope_id.clone(),
                    version: version.clone(),
                    conversation_epoch: scope.conversation_epoch,
                    scope_revision,
                    scope_lifecycle: RuntimeScopeLifecycle::Active,
                    cas_expected_state_version: Some(request.expected_state_version),
                    cas_expected_scope_revision: request.expected_scope_revision,
                    transcript_ack_proof: None,
                    checkpoint: None,
                },
            )?;
            let mut record = scope_record.take().unwrap_or(SqliteRuntimeScopeRecord {
                authority: scope,
                current_retirement: None,
                completed_retirements: Vec::new(),
            });
            record.authority.revision = scope_revision;
            Self::save_scope_record_on_connection(&transaction, &record)?;
            transaction.commit().map_err(|error| {
                RuntimeStateError::Io(format!("failed to commit runtime retirement: {error}"))
            })?;
            Ok(RuntimeGenerationRetireReceipt {
                scope: record.authority,
                runtime_state_id: request.runtime_state_id,
                version,
                status: RuntimeGenerationRetireStatus::Retired,
            })
        })
    }

    fn begin_scope_retirement<'a>(
        &'a self,
        request: ScopeRetirementRequest,
    ) -> BoxFuture<'a, Result<ScopeRetirementReceipt>> {
        if let Err(error) = request.validate() {
            return Box::pin(async move { Err(error) });
        }
        self.run_blocking(Self::collection_scope(&request.scope_id), move |store| {
            let mut conn = store.open_conn()?;
            store.ensure_active_deadline("SQLite scope retirement begin before transaction")?;
            let transaction = Self::immediate_transaction(&mut conn)?;
            store.ensure_active_deadline("SQLite scope retirement begin after transaction lock")?;
            let mut record =
                Self::load_scope_record_on_connection(&transaction, &request.scope_id)?;
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
            let states = Self::scope_state_records_on_connection(&transaction, &request.scope_id)?;
            let authority =
                Self::authority_or_unmanaged(&request.scope_id, record.as_ref(), &states);
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
            for state in &states {
                if matches!(state.version, RuntimeStateVersion::Retired { .. }) {
                    continue;
                }
                items.push(ScopeRetirementItem {
                    runtime_state_id: state.runtime_state_id.clone(),
                    state_version: state.version.clone(),
                    pending_operation_id: Self::pending_operation_id(state)?,
                    status: ScopeRetirementItemStatus::Pending,
                });
            }
            let manifest = ScopeRetirementManifest {
                delete_operation_id: request.delete_operation_id,
                payload_digest: request.payload_digest,
                expected_conversation_epoch: request.expected_conversation_epoch,
                items,
                conversation_delete_receipt: None,
            };
            let authority = RuntimeScopeAuthority {
                scope_id: request.scope_id.clone(),
                conversation_epoch: Some(request.expected_conversation_epoch),
                revision: Self::next_revision(authority.revision, &request.scope_id)?,
                lifecycle: RuntimeScopeLifecycle::Retiring,
            };
            let record = SqliteRuntimeScopeRecord {
                authority: authority.clone(),
                current_retirement: Some(manifest.clone()),
                completed_retirements: record
                    .take()
                    .map(|record| record.completed_retirements)
                    .unwrap_or_default(),
            };
            Self::save_scope_record_on_connection(&transaction, &record)?;
            transaction.commit().map_err(|error| {
                RuntimeStateError::Io(format!("failed to begin scope retirement: {error}"))
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
    ) -> BoxFuture<'a, Result<ScopeRetirementReceipt>> {
        let scope_id = scope_id.to_string();
        let delete_operation_id = delete_operation_id.to_string();
        self.run_blocking(Self::collection_scope(&scope_id), move |store| {
            let mut conn = store.open_conn()?;
            store.ensure_active_deadline("SQLite scope retirement advance before transaction")?;
            let transaction = Self::immediate_transaction(&mut conn)?;
            store.ensure_active_deadline("SQLite scope retirement advance after transaction lock")?;
            let mut record = Self::load_scope_record_on_connection(&transaction, &scope_id)?
                .ok_or_else(|| {
                    RuntimeStateError::NotFound(format!("runtime scope {scope_id} is absent"))
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
                    Self::save_scope_record_on_connection(&transaction, &record)?;
                    transaction.commit().map_err(|error| {
                        RuntimeStateError::Io(format!(
                            "failed to update scope-retirement retention floor: {error}"
                        ))
                    })?;
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

            let completing = matches!(&advance, ScopeRetirementAdvance::Complete);
            let mut changed_state: Option<SqliteRuntimeStateRecord> = None;
            let can_complete = match advance {
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
                    if let Some(current) = manifest.conversation_delete_receipt.as_ref()
                        && !Self::same_conversation_delete_effect(current, &receipt)
                    {
                        return Ok(Self::retirement_receipt(
                            record.authority,
                            manifest,
                            ScopeRetirementStatus::IdentityConflict,
                        ));
                    }
                    manifest.conversation_delete_receipt = Some(receipt);
                    false
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
                            RuntimeStateError::NotFound(format!(
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
                    let state = Self::load_state_record_on_connection(
                        &transaction,
                        &runtime_state_id,
                    )?
                    .ok_or_else(|| {
                        RuntimeStateError::NotFound(format!(
                            "runtime state {runtime_state_id} disappeared during retirement"
                        ))
                    })?;
                    if state.scope_id != scope_id || state.version != item.state_version {
                        return Ok(Self::retirement_receipt(
                            record.authority,
                            manifest,
                            ScopeRetirementStatus::IdentityConflict,
                        ));
                    }
                    changed_state = Some(SqliteRuntimeStateRecord {
                        runtime_state_id: runtime_state_id.clone(),
                        scope_id: scope_id.clone(),
                        version: RuntimeStateVersion::Retired {
                            revision: Self::next_revision(
                                Self::state_revision(&item.state_version),
                                &runtime_state_id,
                            )?,
                            operation_id: delete_operation_id.clone(),
                        },
                        conversation_epoch: record.authority.conversation_epoch,
                        scope_revision: 0,
                        scope_lifecycle: RuntimeScopeLifecycle::Retiring,
                        cas_expected_state_version: None,
                        cas_expected_scope_revision: 0,
                        transcript_ack_proof: None,
                        checkpoint: None,
                    });
                    item.status = ScopeRetirementItemStatus::DroppedByDelete;
                    false
                }
                ScopeRetirementAdvance::Complete => {
                    manifest.conversation_delete_receipt.is_some()
                        && manifest.items.iter().all(|item| {
                            item.status == ScopeRetirementItemStatus::DroppedByDelete
                        })
                }
            };
            if completing && !can_complete {
                return Ok(Self::retirement_receipt(
                    record.authority,
                    manifest,
                    ScopeRetirementStatus::InProgress,
                ));
            }
            let revision = Self::next_revision(record.authority.revision, &scope_id)?;
            if let Some(mut state) = changed_state {
                state.scope_revision = revision;
                Self::save_state_record_on_connection(&transaction, &state)?;
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
                Self::save_scope_record_on_connection(&transaction, &record)?;
                transaction.commit().map_err(|error| {
                    RuntimeStateError::Io(format!(
                        "failed to complete scope retirement: {error}"
                    ))
                })?;
                return Ok(completed);
            }
            record.current_retirement = Some(manifest.clone());
            Self::save_scope_record_on_connection(&transaction, &record)?;
            transaction.commit().map_err(|error| {
                RuntimeStateError::Io(format!("failed to continue scope retirement: {error}"))
            })?;
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
    ) -> BoxFuture<'a, Result<Option<AgentCheckpoint>>> {
        let conversation_id = conversation_id.to_string();
        self.run_blocking(Self::entity_scope(&conversation_id), move |store| {
            let conn = store.open_conn()?;
            Ok(
                Self::load_state_record_on_connection(&conn, &conversation_id)?
                    .and_then(|record| record.checkpoint),
            )
        })
    }

    fn save_checkpoint<'a>(&'a self, checkpoint: &'a AgentCheckpoint) -> BoxFuture<'a, Result<()>> {
        self.save_checkpoint_for_scope(&checkpoint.conversation_id, checkpoint)
    }

    fn save_checkpoint_for_scope<'a>(
        &'a self,
        scope_id: &'a str,
        checkpoint: &'a AgentCheckpoint,
    ) -> BoxFuture<'a, Result<()>> {
        if let Err(error) = Self::validate_unmanaged_checkpoint(checkpoint) {
            return Box::pin(async move { Err(error) });
        }
        let scope_id = scope_id.to_string();
        let checkpoint = checkpoint.clone();
        self.run_blocking(
            Self::entity_scope(&checkpoint.conversation_id),
            move |store| {
                let mut conn = store.open_conn()?;
                let transaction = Self::immediate_transaction(&mut conn)?;
                if Self::load_scope_record_on_connection(&transaction, &scope_id)?.is_some()
                    || Self::load_state_record_on_connection(
                        &transaction,
                        &checkpoint.conversation_id,
                    )?
                    .is_some_and(|state| {
                        !matches!(state.version, RuntimeStateVersion::Unmanaged { .. })
                    })
                {
                    return Err(Self::managed_state_error(format!(
                        "runtime state {} is revision managed",
                        checkpoint.conversation_id
                    )));
                }
                transaction
                .execute(
                    "INSERT INTO runtime_state_scopes (scope_id, runtime_state_id) VALUES (?1, ?2)
                     ON CONFLICT(scope_id, runtime_state_id) DO NOTHING",
                    params![&scope_id, &checkpoint.conversation_id],
                )
                .map_err(|error| {
                    RuntimeStateError::Io(format!("failed to bind checkpoint scope: {error}"))
                })?;
                Self::save_checkpoint_on_connection(&transaction, &checkpoint)?;
                transaction.commit().map_err(|error| {
                    RuntimeStateError::Io(format!(
                        "failed to commit checkpoint transaction: {error}"
                    ))
                })?;
                Ok(())
            },
        )
    }

    fn runtime_state_ids<'a>(&'a self, scope_id: &'a str) -> BoxFuture<'a, Result<Vec<String>>> {
        let scope_id = scope_id.to_string();
        self.run_blocking(Self::collection_scope(&scope_id), move |store| {
            let conn = store.open_conn()?;
            let mut statement = conn
                .prepare(
                    "SELECT runtime_state_id FROM runtime_state_scopes WHERE scope_id = ?1 ORDER BY runtime_state_id",
                )
                .map_err(|error| {
                    RuntimeStateError::Io(format!("failed to prepare scope query: {error}"))
                })?;
            let rows = statement
                .query_map(params![&scope_id], |row| row.get::<_, String>(0))
                .map_err(|error| {
                    RuntimeStateError::Io(format!("failed to query checkpoint scope: {error}"))
                })?;
            let mut ids = Vec::new();
            for row in rows {
                ids.push(row.map_err(|error| {
                    RuntimeStateError::Io(format!("failed to read checkpoint scope: {error}"))
                })?);
            }
            Ok(ids)
        })
    }

    fn clear_runtime_state<'a>(
        &'a self,
        scope_id: &'a str,
        runtime_state_id: &'a str,
    ) -> BoxFuture<'a, Result<RuntimeStateClearReceipt>> {
        let scope_id = scope_id.to_string();
        let runtime_state_id = runtime_state_id.to_string();
        self.run_blocking(Self::entity_scope(&runtime_state_id), move |store| {
            let mut conn = store.open_conn()?;
            let transaction = Self::immediate_transaction(&mut conn)?;
            if Self::load_scope_record_on_connection(&transaction, &scope_id)?.is_some()
                || Self::load_state_record_on_connection(&transaction, &runtime_state_id)?
                    .is_some_and(|state| {
                        !matches!(state.version, RuntimeStateVersion::Unmanaged { .. })
                    })
            {
                return Err(Self::managed_state_error(format!(
                    "runtime state {runtime_state_id} is revision managed"
                )));
            }
            let owner = transaction
                .query_row(
                    "SELECT scope_id FROM runtime_state_scopes WHERE runtime_state_id = ?1",
                    params![&runtime_state_id],
                    |row| row.get::<_, String>(0),
                )
                .map(Some)
                .or_else(|error| match error {
                    rusqlite::Error::QueryReturnedNoRows => Ok(None),
                    error => Err(error),
                })
                .map_err(|error| {
                    RuntimeStateError::Io(format!("failed to inspect runtime scope: {error}"))
                })?;
            if let Some(owner) = owner.as_deref()
                && owner != scope_id
            {
                return Err(RuntimeStateError::SerializationError(format!(
                    "runtime state {runtime_state_id} belongs to scope {owner}, not {scope_id}"
                ))
                .into());
            }
            let indexed = owner.as_deref() == Some(scope_id.as_str());
            let legacy = owner.is_none() && scope_id == runtime_state_id;
            let checkpoint_removed = if indexed || legacy {
                transaction
                    .execute(
                        "DELETE FROM agent_checkpoints WHERE conversation_id = ?1",
                        params![&runtime_state_id],
                    )
                    .map_err(|error| {
                        RuntimeStateError::Io(format!("failed to delete checkpoint: {error}"))
                    })?
                    != 0
            } else {
                false
            };
            if indexed {
                transaction
                    .execute(
                        "DELETE FROM runtime_state_scopes WHERE scope_id = ?1 AND runtime_state_id = ?2",
                        params![&scope_id, &runtime_state_id],
                    )
                    .map_err(|error| {
                        RuntimeStateError::Io(format!("failed to delete scope binding: {error}"))
                    })?;
            }
            transaction.commit().map_err(|error| {
                RuntimeStateError::Io(format!("failed to commit runtime clear: {error}"))
            })?;
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
    ) -> BoxFuture<'a, Result<RuntimeStateScopeClearReceipt>> {
        let scope_id = scope_id.to_string();
        self.run_blocking(Self::collection_scope(&scope_id), move |store| {
            let mut conn = store.open_conn()?;
            let transaction = Self::immediate_transaction(&mut conn)?;
            if Self::load_scope_record_on_connection(&transaction, &scope_id)?.is_some() {
                return Err(Self::managed_state_error(format!(
                    "runtime scope {scope_id} is revision managed"
                )));
            }
            if Self::scope_state_records_on_connection(&transaction, &scope_id)?
                .iter()
                .any(|state| !matches!(state.version, RuntimeStateVersion::Unmanaged { .. }))
            {
                return Err(Self::managed_state_error(format!(
                    "runtime scope {scope_id} contains revision-managed state"
                )));
            }
            let mut runtime_state_ids = {
                let mut statement = transaction
                    .prepare(
                        "SELECT runtime_state_id FROM runtime_state_scopes WHERE scope_id = ?1 ORDER BY runtime_state_id",
                    )
                    .map_err(|error| {
                        RuntimeStateError::Io(format!("failed to prepare scope clear: {error}"))
                    })?;
                let rows = statement
                    .query_map(params![&scope_id], |row| row.get::<_, String>(0))
                    .map_err(|error| {
                        RuntimeStateError::Io(format!("failed to query scope clear: {error}"))
                    })?;
                let mut ids = Vec::new();
                for row in rows {
                    ids.push(row.map_err(|error| {
                        RuntimeStateError::Io(format!("failed to read scope clear: {error}"))
                    })?);
                }
                ids
            };
            let legacy_owner = transaction
                .query_row(
                    "SELECT scope_id FROM runtime_state_scopes WHERE runtime_state_id = ?1",
                    params![&scope_id],
                    |row| row.get::<_, String>(0),
                )
                .map(Some)
                .or_else(|error| match error {
                    rusqlite::Error::QueryReturnedNoRows => Ok(None),
                    error => Err(error),
                })
                .map_err(|error| {
                    RuntimeStateError::Io(format!("failed to inspect legacy owner: {error}"))
                })?;
            // A foreign scope may legitimately own a runtime ID equal to this
            // scope's name. That only disables the legacy same-ID fallback; it
            // must not block deletion of rows actually owned by `scope_id`.
            let legacy_exists = legacy_owner.is_none()
                && transaction
                .query_row(
                    "SELECT 1 FROM agent_checkpoints WHERE conversation_id = ?1",
                    params![&scope_id],
                    |_row| Ok(()),
                )
                .map(|()| true)
                .or_else(|error| match error {
                    rusqlite::Error::QueryReturnedNoRows => Ok(false),
                    error => Err(error),
                })
                .map_err(|error| {
                    RuntimeStateError::Io(format!("failed to inspect legacy checkpoint: {error}"))
                })?;
            if legacy_exists
                && !runtime_state_ids
                    .iter()
                    .any(|runtime_id| runtime_id == &scope_id)
            {
                runtime_state_ids.push(scope_id.to_string());
                runtime_state_ids.sort();
            }
            for runtime_state_id in &runtime_state_ids {
                transaction
                    .execute(
                        "DELETE FROM agent_checkpoints WHERE conversation_id = ?1",
                        params![runtime_state_id],
                    )
                    .map_err(|error| {
                        RuntimeStateError::Io(format!("failed to delete scope checkpoint: {error}"))
                    })?;
            }
            transaction
                .execute(
                    "DELETE FROM runtime_state_scopes WHERE scope_id = ?1",
                    params![&scope_id],
                )
                .map_err(|error| {
                    RuntimeStateError::Io(format!("failed to delete runtime scope: {error}"))
                })?;
            transaction.commit().map_err(|error| {
                RuntimeStateError::Io(format!("failed to commit scope clear: {error}"))
            })?;
            Ok(RuntimeStateScopeClearReceipt {
                scope_id,
                runtime_state_ids,
            })
        })
    }

    fn clear_conversation<'a>(&'a self, conversation_id: &'a str) -> BoxFuture<'a, Result<()>> {
        Box::pin(async move {
            self.clear_runtime_state(conversation_id, conversation_id)
                .await
                .map(|_receipt| ())
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use std::time::Duration;

    fn checkpoint(runtime_state_id: &str, marker: &str) -> Result<AgentCheckpoint> {
        Ok(AgentCheckpoint {
            conversation_id: runtime_state_id.to_string(),
            messages_json: serde_json::to_string(&vec![crate::llm::types::Message::user(
                marker.to_string(),
            )])
            .map_err(|error| RuntimeStateError::SerializationError(error.to_string()))?,
            current_plan: None,
            active_skills: Vec::new(),
            blocked_reason: None,
            working_dir: None,
            timestamp: Utc::now(),
        })
    }

    fn managed_checkpoint(runtime_state_id: &str, marker: &str) -> Result<AgentCheckpoint> {
        let mut checkpoint = checkpoint(runtime_state_id, marker)?;
        checkpoint.messages_json = AgentCheckpoint::serialize_payload(
            vec![crate::llm::types::Message::user(marker.to_string())],
            None,
        )?;
        Ok(checkpoint)
    }

    fn pending_checkpoint(runtime_state_id: &str, scope_id: &str) -> Result<AgentCheckpoint> {
        pending_checkpoint_at_revision(runtime_state_id, scope_id, 1)
    }

    fn pending_checkpoint_at_revision(
        runtime_state_id: &str,
        scope_id: &str,
        base_runtime_revision: u64,
    ) -> Result<AgentCheckpoint> {
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
            .collect::<Result<Vec<_>>>()?;
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
    ) -> Result<RuntimeTranscriptAckRequest> {
        let payload = checkpoint.restore_managed_runtime_payload()?;
        let pending = payload.pending_transcript_projection.ok_or_else(|| {
            RuntimeStateError::NotFound(
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
    ) -> Result<AgentCheckpoint> {
        let payload = checkpoint.restore_managed_runtime_payload()?;
        let mut pending = payload
            .pending_transcript_projection
            .ok_or_else(|| RuntimeStateError::NotFound("pending debt".to_string()))?;
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

    fn retirement_manifest(scope_id: &str, epoch: u64) -> Result<ScopeRetirementManifest> {
        let delete = crate::memory::ManagedConversationDelete::prepare(scope_id, epoch)?;
        Ok(ScopeRetirementManifest {
            delete_operation_id: delete.operation_id,
            payload_digest: delete.payload_digest,
            expected_conversation_epoch: epoch,
            items: Vec::new(),
            conversation_delete_receipt: None,
        })
    }

    fn inject_scope_record(
        store: &SqliteRuntimeStateStore,
        authority: &RuntimeScopeAuthority,
        manifest: &ScopeRetirementManifest,
    ) -> Result<()> {
        let connection = store.open_conn()?;
        connection
            .execute(
                "INSERT OR REPLACE INTO runtime_state_authorities
                 (scope_id, authority_json, current_retirement_json, completed_retirements_json)
                 VALUES (?1, ?2, ?3, ?4)",
                params![
                    &authority.scope_id,
                    serde_json::to_string(authority).map_err(|error| {
                        RuntimeStateError::SerializationError(error.to_string())
                    })?,
                    serde_json::to_string(manifest).map_err(|error| {
                        RuntimeStateError::SerializationError(error.to_string())
                    })?,
                    "[]",
                ],
            )
            .map_err(|error| RuntimeStateError::Io(error.to_string()))?;
        Ok(())
    }

    #[tokio::test]
    async fn revisioned_runtime_state_contract_is_supported() -> Result<()> {
        let path = std::env::temp_dir().join(format!(
            "echo-state-revisioned-{}-{}.sqlite",
            std::process::id(),
            uuid::Uuid::new_v4()
        ));
        let store = SqliteRuntimeStateStore::new(&path)?;

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
                        checkpoint: checkpoint("runtime-a", "invalid-epoch")?,
                    })
                    .await
                    .is_err()
            );
        }
        assert!(store.load_scope_authority("scope-a").await?.is_none());

        let _ = std::fs::remove_file(path);
        Ok(())
    }

    #[tokio::test]
    async fn revisioned_cas_replays_and_fences_legacy_mutators() -> Result<()> {
        let path = std::env::temp_dir().join(format!(
            "echo-state-cas-{}-{}.sqlite",
            std::process::id(),
            uuid::Uuid::new_v4()
        ));
        let store = SqliteRuntimeStateStore::new(&path)?;
        let initial = checkpoint("runtime-a", "initial")?;
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
        assert_eq!(
            applied.version,
            RuntimeStateVersion::Managed { revision: 1 }
        );
        assert_eq!(
            store.compare_and_save_checkpoint(create).await?.status,
            RuntimeCheckpointCasStatus::AlreadyCurrent
        );
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
        let stale = store
            .compare_and_save_checkpoint(RuntimeCheckpointCasRequest {
                scope_id: "scope-a".to_string(),
                runtime_state_id: "runtime-a".to_string(),
                conversation_epoch: Some(1),
                expected_scope_revision: 0,
                expected_state_version: RuntimeStateExpectedVersion::Managed { revision: 1 },
                checkpoint: checkpoint("runtime-a", "stale")?,
            })
            .await?;
        assert_eq!(stale.status, RuntimeCheckpointCasStatus::RevisionConflict);
        assert!(store.save_checkpoint(&initial).await.is_err());
        assert!(store.clear_conversation_sync("runtime-a").is_err());
        assert!(
            store
                .clear_runtime_state("scope-a", "runtime-a")
                .await
                .is_err()
        );

        let updated = store
            .compare_and_save_checkpoint(RuntimeCheckpointCasRequest {
                scope_id: "scope-a".to_string(),
                runtime_state_id: "runtime-a".to_string(),
                conversation_epoch: Some(1),
                expected_scope_revision: 1,
                expected_state_version: RuntimeStateExpectedVersion::Managed { revision: 1 },
                checkpoint: checkpoint("runtime-a", "updated")?,
            })
            .await?;
        assert_eq!(updated.status, RuntimeCheckpointCasStatus::Applied);
        let retired = store
            .retire_runtime_generation(RuntimeGenerationRetireRequest::prepare(
                "scope-a",
                "runtime-a",
                updated.scope.revision,
                RuntimeStateExpectedVersion::Managed { revision: 2 },
            )?)
            .await?;
        assert_eq!(retired.status, RuntimeGenerationRetireStatus::Retired);
        assert!(store.get_checkpoint("runtime-a").await?.is_none());
        drop(store);

        let restarted = SqliteRuntimeStateStore::new(&path)?;
        let snapshot = restarted
            .load_runtime_state("scope-a", "runtime-a")
            .await?
            .ok_or_else(|| RuntimeStateError::NotFound("retirement tombstone".to_string()))?;
        assert!(matches!(
            snapshot.version,
            RuntimeStateVersion::Retired { .. }
        ));
        assert_eq!(
            restarted
                .retire_runtime_generation(RuntimeGenerationRetireRequest::prepare(
                    "scope-a",
                    "runtime-a",
                    updated.scope.revision,
                    RuntimeStateExpectedVersion::Managed { revision: 2 },
                )?)
                .await?
                .status,
            RuntimeGenerationRetireStatus::AlreadyRetired
        );
        assert_eq!(
            restarted
                .retire_runtime_generation(RuntimeGenerationRetireRequest::prepare(
                    "scope-a",
                    "runtime-a",
                    1,
                    RuntimeStateExpectedVersion::Managed { revision: 1 },
                )?)
                .await?
                .status,
            RuntimeGenerationRetireStatus::Conflict
        );

        let legacy = AgentCheckpoint {
            conversation_id: "runtime-b".to_string(),
            messages_json: "[]".to_string(),
            current_plan: None,
            active_skills: Vec::new(),
            blocked_reason: None,
            working_dir: None,
            timestamp: Utc::now(),
        };
        restarted
            .save_checkpoint_for_scope("scope-b", &legacy)
            .await?;
        let unmanaged = restarted
            .load_runtime_state("scope-b", "runtime-b")
            .await?
            .ok_or_else(|| RuntimeStateError::NotFound("unmanaged checkpoint".to_string()))?;
        let RuntimeStateVersion::Unmanaged { digest } = unmanaged.version else {
            return Err(RuntimeStateError::SerializationError(
                "legacy checkpoint was not reported as unmanaged".to_string(),
            )
            .into());
        };
        let adopted = restarted
            .compare_and_save_checkpoint(RuntimeCheckpointCasRequest {
                scope_id: "scope-b".to_string(),
                runtime_state_id: "runtime-b".to_string(),
                conversation_epoch: Some(1),
                expected_scope_revision: 0,
                expected_state_version: RuntimeStateExpectedVersion::Unmanaged { digest },
                checkpoint: checkpoint("runtime-b", "adopted")?,
            })
            .await?;
        assert_eq!(adopted.status, RuntimeCheckpointCasStatus::Applied);
        assert_eq!(
            adopted.version,
            RuntimeStateVersion::Managed { revision: 1 }
        );
        let _ = std::fs::remove_file(path);
        Ok(())
    }

    #[tokio::test]
    async fn pending_transcript_requires_proof_carrying_acknowledgement() -> Result<()> {
        let path = std::env::temp_dir().join(format!(
            "echo-state-pending-ack-{}-{}.sqlite",
            std::process::id(),
            uuid::Uuid::new_v4()
        ));
        let store = SqliteRuntimeStateStore::new(&path)?;
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
        let mut retry_debt = payload
            .pending_transcript_projection
            .ok_or_else(|| RuntimeStateError::NotFound("pending debt".to_string()))?;
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
        changed_workdir.working_dir = Some(std::path::PathBuf::from("/forged"));
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
        let mut dispatch_debt = recorded_payload
            .pending_transcript_projection
            .ok_or_else(|| RuntimeStateError::NotFound("recorded debt".to_string()))?;
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
            .ok_or_else(|| RuntimeStateError::NotFound("runtime-pending".to_string()))?;
        assert_eq!(
            unchanged.version,
            RuntimeStateVersion::Managed { revision: 3 }
        );
        assert!(
            unchanged
                .checkpoint
                .as_ref()
                .ok_or_else(|| {
                    RuntimeStateError::NotFound("runtime-pending checkpoint".to_string())
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
        let generic_checkpoint = checkpoint("runtime-generic", "generic")?;
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

        let connection = store.open_conn()?;
        let proof_json = connection
            .query_row(
                "SELECT transcript_ack_proof_json FROM runtime_state_versions
                 WHERE runtime_state_id = ?1",
                params!["runtime-pending"],
                |row| row.get::<_, String>(0),
            )
            .map_err(|error| RuntimeStateError::Io(error.to_string()))?;
        let mut proof: SqliteRuntimeTranscriptAckProof =
            SqliteRuntimeStateStore::decode(&proof_json, "test acknowledgement proof")?;
        let original_proof = proof.clone();
        proof.payload_digest = proof.payload_digest.to_ascii_uppercase();
        proof.operation_id = format!("transcript-projection-v1:{}", proof.payload_digest);
        connection
            .execute(
                "UPDATE runtime_state_versions SET transcript_ack_proof_json = ?1
                 WHERE runtime_state_id = ?2",
                params![
                    SqliteRuntimeStateStore::encode(&proof, "test acknowledgement proof")?,
                    "runtime-pending"
                ],
            )
            .map_err(|error| RuntimeStateError::Io(error.to_string()))?;
        assert!(
            store
                .load_runtime_state("scope-pending", "runtime-pending")
                .await
                .is_err()
        );
        connection
            .execute(
                "UPDATE runtime_state_versions
                 SET state_version_json = ?1, transcript_ack_proof_json = ?2
                 WHERE runtime_state_id = ?3",
                params![
                    SqliteRuntimeStateStore::encode(
                        &RuntimeStateVersion::Unmanaged {
                            digest: "legacy".to_string()
                        },
                        "test unmanaged version"
                    )?,
                    SqliteRuntimeStateStore::encode(&original_proof, "test acknowledgement proof")?,
                    "runtime-pending"
                ],
            )
            .map_err(|error| RuntimeStateError::Io(error.to_string()))?;
        assert!(
            store
                .load_runtime_state("scope-pending", "runtime-pending")
                .await
                .is_err()
        );

        let _ = std::fs::remove_file(path);
        Ok(())
    }

    #[tokio::test]
    async fn legacy_raw_pending_is_rejected_without_side_effects() -> Result<()> {
        let path = std::env::temp_dir().join(format!(
            "echo-state-legacy-pending-{}-{}.sqlite",
            std::process::id(),
            uuid::Uuid::new_v4()
        ));
        let store = SqliteRuntimeStateStore::new(&path)?;
        let pending = pending_checkpoint("legacy-pending", "legacy-scope")?;
        assert!(
            store
                .save_checkpoint_for_scope("legacy-scope", &pending)
                .await
                .is_err()
        );
        let checkpoint_only = managed_checkpoint("legacy-v2", "managed")?;
        store.save_checkpoint(&checkpoint_only).await?;
        assert!(store.get_checkpoint("legacy-pending").await?.is_none());
        let restored = store
            .get_checkpoint("legacy-v2")
            .await?
            .ok_or_else(|| RuntimeStateError::NotFound("legacy-v2".to_string()))?
            .restore_messages()?;
        assert!(SqliteRuntimeStateStore::serialized_eq(
            &restored,
            &checkpoint_only.restore_messages()?
        )?);
        let cursor = super::super::TranscriptProjectionCheckpoint {
            generation_id: "legacy-cursor".to_string(),
            next_ordinal: 0,
            projected: Vec::new(),
        };
        let mut cursor_checkpoint = checkpoint("legacy-cursor", "cursor")?;
        cursor_checkpoint.messages_json = AgentCheckpoint::serialize_payload(
            vec![crate::llm::types::Message::user("cursor".to_string())],
            Some(cursor.clone()),
        )?;
        store.save_checkpoint(&cursor_checkpoint).await?;
        assert_eq!(
            store
                .get_checkpoint("legacy-cursor")
                .await?
                .ok_or_else(|| RuntimeStateError::NotFound("legacy-cursor".to_string()))?
                .restore_transcript_projection()?,
            Some(cursor)
        );
        assert!(store.runtime_state_ids("legacy-scope").await?.is_empty());
        let _ = std::fs::remove_file(path);
        Ok(())
    }

    #[tokio::test]
    async fn corrupt_pending_base_revision_fails_closed() -> Result<()> {
        let path = std::env::temp_dir().join(format!(
            "echo-state-base-corrupt-{}-{}.sqlite",
            std::process::id(),
            uuid::Uuid::new_v4()
        ));
        let store = SqliteRuntimeStateStore::new(&path)?;
        let pending = pending_checkpoint("runtime-base-corrupt", "scope-base-corrupt")?;
        store
            .compare_and_save_checkpoint(RuntimeCheckpointCasRequest {
                scope_id: "scope-base-corrupt".to_string(),
                runtime_state_id: "runtime-base-corrupt".to_string(),
                conversation_epoch: Some(1),
                expected_scope_revision: 0,
                expected_state_version: RuntimeStateExpectedVersion::Absent,
                checkpoint: pending.clone(),
            })
            .await?;
        let payload = pending.restore_managed_runtime_payload()?;
        let mut debt = payload
            .pending_transcript_projection
            .ok_or_else(|| RuntimeStateError::NotFound("pending debt".to_string()))?;
        debt.base_runtime_revision = 99;
        let tampered = AgentCheckpoint::serialize_managed_payload(
            payload.messages,
            payload.transcript_projection,
            Some(debt),
        )?;
        let connection = store.open_conn()?;
        connection
            .execute(
                "UPDATE agent_checkpoints SET messages_json = ?1 WHERE conversation_id = ?2",
                params![tampered, "runtime-base-corrupt"],
            )
            .map_err(|error| RuntimeStateError::Io(error.to_string()))?;
        assert!(
            store
                .load_runtime_state("scope-base-corrupt", "runtime-base-corrupt")
                .await
                .is_err()
        );
        assert!(store.get_checkpoint("runtime-base-corrupt").await.is_err());
        let persisted = connection
            .query_row(
                "SELECT messages_json FROM agent_checkpoints WHERE conversation_id = ?1",
                params!["runtime-base-corrupt"],
                |row| row.get::<_, String>(0),
            )
            .map_err(|error| RuntimeStateError::Io(error.to_string()))?;
        assert_eq!(persisted, tampered);
        let _ = std::fs::remove_file(path);
        Ok(())
    }

    #[tokio::test]
    async fn retry_terminal_failure_remains_durably_blocked() -> Result<()> {
        let path = std::env::temp_dir().join(format!(
            "echo-state-terminal-retry-{}-{}.sqlite",
            std::process::id(),
            uuid::Uuid::new_v4()
        ));
        let store = SqliteRuntimeStateStore::new(&path)?;
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
            .ok_or_else(|| RuntimeStateError::NotFound("runtime-terminal".to_string()))?;
        let debt = loaded
            .checkpoint
            .ok_or_else(|| RuntimeStateError::NotFound("runtime-terminal checkpoint".to_string()))?
            .restore_managed_runtime_payload()?
            .pending_transcript_projection
            .ok_or_else(|| RuntimeStateError::NotFound("terminal debt".to_string()))?;
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
        let _ = std::fs::remove_file(path);
        Ok(())
    }

    #[tokio::test]
    async fn managed_deadline_expires_in_queue_without_side_effects() -> Result<()> {
        let path = std::env::temp_dir().join(format!(
            "echo-state-deadline-{}-{}.sqlite",
            std::process::id(),
            uuid::Uuid::new_v4()
        ));
        let store = SqliteRuntimeStateStore::new(&path)?;
        assert_eq!(
            store.persistence_call_capability(),
            crate::memory::PersistenceCallCapability::AbsoluteDeadlineV1
        );
        let blocker_store = store.clone();
        let (entered_tx, entered_rx) = tokio::sync::oneshot::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let blocker = tokio::spawn(async move {
            blocker_store
                .run_blocking(
                    SqliteRuntimeStateStore::collection_scope("deadline-scope"),
                    move |_store| {
                        let _ignored = entered_tx.send(());
                        release_rx
                            .recv_timeout(std::time::Duration::from_secs(2))
                            .map_err(|error| RuntimeStateError::Io(error.to_string()))?;
                        Ok(())
                    },
                )
                .await
        });
        entered_rx
            .await
            .map_err(|error| RuntimeStateError::Io(error.to_string()))?;
        let context = crate::memory::PersistenceCallContext::with_timeout(
            std::time::Duration::from_millis(25),
        )?;
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
        tokio::time::sleep(std::time::Duration::from_millis(60)).await;
        release_tx
            .send(())
            .map_err(|error| RuntimeStateError::Io(error.to_string()))?;
        blocker
            .await
            .map_err(|error| RuntimeStateError::Io(error.to_string()))??;
        let error = queued
            .await
            .map_err(|error| RuntimeStateError::Io(error.to_string()))?
            .err()
            .ok_or_else(|| {
                RuntimeStateError::SerializationError(
                    "expired runtime write was accepted".to_string(),
                )
            })?;
        assert!(matches!(
            error,
            crate::error::ReactError::RuntimeState(error)
                if matches!(error.as_ref(), RuntimeStateError::DeadlineExceeded(_))
        ));
        assert!(
            store
                .load_runtime_state("deadline-scope", "deadline-runtime")
                .await?
                .is_none()
        );
        let _ = std::fs::remove_file(path);
        Ok(())
    }

    #[tokio::test]
    async fn managed_deadline_expires_while_waiting_for_sqlite_writer() -> Result<()> {
        let path = std::env::temp_dir().join(format!(
            "echo-state-deadline-writer-{}-{}.sqlite",
            std::process::id(),
            uuid::Uuid::new_v4()
        ));
        let store = SqliteRuntimeStateStore::new(&path)?;
        let writer = store.open_conn()?;
        writer
            .execute_batch("BEGIN IMMEDIATE")
            .map_err(|error| RuntimeStateError::Io(error.to_string()))?;
        let context = crate::memory::PersistenceCallContext::with_timeout(
            std::time::Duration::from_millis(25),
        )?;
        let queued_store = store.clone();
        let queued = tokio::spawn(async move {
            queued_store
                .compare_and_save_checkpoint_with_context(
                    context,
                    RuntimeCheckpointCasRequest {
                        scope_id: "deadline-writer-scope".to_string(),
                        runtime_state_id: "deadline-writer-runtime".to_string(),
                        conversation_epoch: Some(1),
                        expected_scope_revision: 0,
                        expected_state_version: RuntimeStateExpectedVersion::Absent,
                        checkpoint: managed_checkpoint("deadline-writer-runtime", "deadline")?,
                    },
                )
                .await
        });
        tokio::time::sleep(std::time::Duration::from_millis(60)).await;
        writer
            .execute_batch("ROLLBACK")
            .map_err(|error| RuntimeStateError::Io(error.to_string()))?;
        drop(writer);
        let error = queued
            .await
            .map_err(|error| RuntimeStateError::Io(error.to_string()))?
            .err()
            .ok_or_else(|| {
                RuntimeStateError::SerializationError(
                    "expired writer-blocked runtime write was accepted".to_string(),
                )
            })?;
        assert!(matches!(
            error,
            crate::error::ReactError::RuntimeState(error)
                if matches!(error.as_ref(), RuntimeStateError::DeadlineExceeded(_))
        ));
        assert!(
            store
                .load_runtime_state("deadline-writer-scope", "deadline-writer-runtime")
                .await?
                .is_none()
        );
        let _ = std::fs::remove_file(path);
        Ok(())
    }

    #[tokio::test]
    async fn invalid_runtime_state_requests_have_no_durable_side_effects() -> Result<()> {
        let path = std::env::temp_dir().join(format!(
            "echo-state-validation-{}-{}.sqlite",
            std::process::id(),
            uuid::Uuid::new_v4()
        ));
        let store = SqliteRuntimeStateStore::new(&path)?;
        let applied = store
            .compare_and_save_checkpoint(RuntimeCheckpointCasRequest {
                scope_id: "scope-validation".to_string(),
                runtime_state_id: "runtime-validation".to_string(),
                conversation_epoch: Some(1),
                expected_scope_revision: 0,
                expected_state_version: RuntimeStateExpectedVersion::Absent,
                checkpoint: checkpoint("runtime-validation", "original")?,
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

        let mut malformed_checkpoint = checkpoint("runtime-validation", "invalid")?;
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

        assert!(
            store
                .compare_and_save_checkpoint(RuntimeCheckpointCasRequest {
                    scope_id: String::new(),
                    runtime_state_id: "runtime-validation".to_string(),
                    conversation_epoch: Some(1),
                    expected_scope_revision: authority_before.revision,
                    expected_state_version: RuntimeStateExpectedVersion::Managed { revision: 1 },
                    checkpoint: checkpoint("runtime-validation", "invalid scope")?,
                })
                .await
                .is_err()
        );
        assert!(
            store
                .compare_and_save_checkpoint(RuntimeCheckpointCasRequest {
                    scope_id: "scope-validation".to_string(),
                    runtime_state_id: String::new(),
                    conversation_epoch: Some(1),
                    expected_scope_revision: authority_before.revision,
                    expected_state_version: RuntimeStateExpectedVersion::Managed { revision: 1 },
                    checkpoint: checkpoint("", "invalid runtime")?,
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
            .ok_or_else(|| RuntimeStateError::NotFound("runtime state".to_string()))?;
        assert_eq!(state.version, RuntimeStateVersion::Managed { revision: 1 });
        assert!(
            state
                .checkpoint
                .is_some_and(|checkpoint| checkpoint.messages_json.contains("original"))
        );
        let _ = std::fs::remove_file(path);
        Ok(())
    }

    #[tokio::test]
    async fn corrupt_runtime_scope_saga_fails_closed() -> Result<()> {
        let path = std::env::temp_dir().join(format!(
            "echo-state-corrupt-saga-{}-{}.sqlite",
            std::process::id(),
            uuid::Uuid::new_v4()
        ));
        let store = SqliteRuntimeStateStore::new(&path)?;
        let mut authority = RuntimeScopeAuthority {
            scope_id: "scope-corrupt".to_string(),
            conversation_epoch: Some(1),
            revision: 1,
            lifecycle: RuntimeScopeLifecycle::Active,
        };
        inject_scope_record(
            &store,
            &authority,
            &retirement_manifest("scope-corrupt", 1)?,
        )?;
        assert!(store.load_scope_authority("scope-corrupt").await.is_err());
        assert!(
            store
                .compare_and_save_checkpoint(RuntimeCheckpointCasRequest {
                    scope_id: "scope-corrupt".to_string(),
                    runtime_state_id: "runtime-corrupt".to_string(),
                    conversation_epoch: Some(1),
                    expected_scope_revision: 1,
                    expected_state_version: RuntimeStateExpectedVersion::Absent,
                    checkpoint: checkpoint("runtime-corrupt", "must not persist")?,
                })
                .await
                .is_err()
        );
        assert!(
            store
                .load_runtime_state("scope-corrupt", "runtime-corrupt")
                .await?
                .is_none()
        );

        authority.lifecycle = RuntimeScopeLifecycle::Retiring;
        inject_scope_record(
            &store,
            &authority,
            &retirement_manifest("scope-corrupt", 2)?,
        )?;
        assert!(store.load_scope_authority("scope-corrupt").await.is_err());

        let mut identity_mismatch = retirement_manifest("scope-corrupt", 1)?;
        identity_mismatch.payload_digest = "tampered".to_string();
        inject_scope_record(&store, &authority, &identity_mismatch)?;
        assert!(store.load_scope_authority("scope-corrupt").await.is_err());

        let checkpoint = checkpoint("runtime-false-drop", "still-active")?;
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
        let false_drop_authority = RuntimeScopeAuthority {
            lifecycle: RuntimeScopeLifecycle::Retiring,
            revision: created.scope.revision.saturating_add(1),
            ..created.scope
        };
        inject_scope_record(&store, &false_drop_authority, &manifest)?;
        let checkpoint_before = store
            .get_checkpoint("runtime-false-drop")
            .await?
            .ok_or_else(|| RuntimeStateError::NotFound("runtime-false-drop".to_string()))?;
        assert!(
            store
                .continue_scope_retirement(
                    "scope-false-drop",
                    &delete.operation_id,
                    false_drop_authority.revision,
                    ScopeRetirementAdvance::Complete,
                )
                .await
                .is_err()
        );
        assert_eq!(
            store
                .get_checkpoint("runtime-false-drop")
                .await?
                .ok_or_else(|| { RuntimeStateError::NotFound("runtime-false-drop".to_string()) })?
                .messages_json,
            checkpoint_before.messages_json
        );

        let mut duplicate = manifest.clone();
        duplicate.items.push(dropped_item);
        inject_scope_record(&store, &false_drop_authority, &duplicate)?;
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
        inject_scope_record(&store, &false_drop_authority, &forged_receipt)?;
        assert!(
            store
                .load_scope_authority("scope-false-drop")
                .await
                .is_err()
        );
        assert_eq!(
            store
                .get_checkpoint("runtime-false-drop")
                .await?
                .ok_or_else(|| { RuntimeStateError::NotFound("runtime-false-drop".to_string()) })?
                .messages_json,
            checkpoint_before.messages_json
        );

        let incomplete_manifest = retirement_manifest("scope-false-drop", 1)?;
        let completed_authority = RuntimeScopeAuthority {
            lifecycle: RuntimeScopeLifecycle::Active,
            revision: false_drop_authority.revision.saturating_add(1),
            ..false_drop_authority.clone()
        };
        let incomplete_history = vec![ScopeRetirementReceipt {
            scope: RuntimeScopeAuthority {
                lifecycle: RuntimeScopeLifecycle::Tombstoned,
                ..false_drop_authority.clone()
            },
            manifest: incomplete_manifest,
            dropped_operation_ids: Vec::new(),
            retention_floor_epoch: 0,
            status: ScopeRetirementStatus::InProgress,
        }];
        let connection = store.open_conn()?;
        connection
            .execute(
                "INSERT OR REPLACE INTO runtime_state_authorities
                 (scope_id, authority_json, current_retirement_json, completed_retirements_json)
                 VALUES (?1, ?2, NULL, ?3)",
                params![
                    &completed_authority.scope_id,
                    serde_json::to_string(&completed_authority).map_err(|error| {
                        RuntimeStateError::SerializationError(error.to_string())
                    })?,
                    serde_json::to_string(&incomplete_history).map_err(|error| {
                        RuntimeStateError::SerializationError(error.to_string())
                    })?,
                ],
            )
            .map_err(|error| RuntimeStateError::Io(error.to_string()))?;
        assert!(
            store
                .load_scope_authority("scope-false-drop")
                .await
                .is_err()
        );
        let _ = std::fs::remove_file(path);
        Ok(())
    }

    #[tokio::test]
    async fn runtime_state_scope_retirement_replays_and_isolates_new_incarnation() -> Result<()> {
        let path = std::env::temp_dir().join(format!(
            "echo-state-retirement-{}-{}.sqlite",
            std::process::id(),
            uuid::Uuid::new_v4()
        ));
        let store = SqliteRuntimeStateStore::new(&path)?;
        let first_request = RuntimeCheckpointCasRequest {
            scope_id: "scope-a".to_string(),
            runtime_state_id: "runtime-a1".to_string(),
            conversation_epoch: Some(10),
            expected_scope_revision: 0,
            expected_state_version: RuntimeStateExpectedVersion::Absent,
            checkpoint: checkpoint("runtime-a1", "one")?,
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
                checkpoint: checkpoint("runtime-a2", "two")?,
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
        let first_drop = store
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
        let replay = store
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
        assert_eq!(replay.scope.revision, first_drop.scope.revision);
        let second_drop = store
            .continue_scope_retirement(
                "scope-a",
                &request.delete_operation_id,
                first_drop.scope.revision,
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
                second_drop.scope.revision,
                ScopeRetirementAdvance::Complete,
            )
            .await?;
        assert_eq!(completed.status, ScopeRetirementStatus::Completed);

        let old_epoch_reopen = store
            .compare_and_save_checkpoint(RuntimeCheckpointCasRequest {
                scope_id: "scope-a".to_string(),
                runtime_state_id: "runtime-old-epoch".to_string(),
                conversation_epoch: Some(10),
                expected_scope_revision: completed.scope.revision,
                expected_state_version: RuntimeStateExpectedVersion::Absent,
                checkpoint: checkpoint("runtime-old-epoch", "stale")?,
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
                checkpoint: checkpoint("runtime-a3", "three")?,
            })
            .await?;
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
            .ok_or_else(|| RuntimeStateError::NotFound("scope authority".to_string()))?;
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
        assert_eq!(
            store
                .begin_scope_retirement(ScopeRetirementRequest::prepare(
                    "scope-a",
                    second_completed.scope.revision,
                    &stale_delete,
                )?)
                .await?
                .status,
            ScopeRetirementStatus::EpochConflict
        );
        let _ = std::fs::remove_file(path);
        Ok(())
    }

    #[tokio::test]
    async fn revisioned_tables_migrate_legacy_checkpoint_database() -> Result<()> {
        let path = std::env::temp_dir().join(format!(
            "echo-state-legacy-migration-{}-{}.sqlite",
            std::process::id(),
            uuid::Uuid::new_v4()
        ));
        let connection =
            Connection::open(&path).map_err(|error| RuntimeStateError::Io(error.to_string()))?;
        connection
            .execute_batch(
                "CREATE TABLE agent_checkpoints (
                    conversation_id TEXT PRIMARY KEY,
                    messages_json TEXT NOT NULL,
                    current_plan TEXT,
                    active_skills TEXT NOT NULL,
                    blocked_reason TEXT,
                    timestamp TEXT NOT NULL
                 );",
            )
            .map_err(|error| RuntimeStateError::Io(error.to_string()))?;
        connection
            .execute(
                "INSERT INTO agent_checkpoints
                 (conversation_id, messages_json, current_plan, active_skills, blocked_reason, timestamp)
                 VALUES (?1, ?2, NULL, ?3, NULL, ?4)",
                params!["legacy", "[]", "[]", Utc::now().to_rfc3339()],
            )
            .map_err(|error| RuntimeStateError::Io(error.to_string()))?;
        connection
            .execute(
                "INSERT INTO agent_checkpoints
                 (conversation_id, messages_json, current_plan, active_skills, blocked_reason, timestamp)
                 VALUES (?1, ?2, NULL, ?3, NULL, ?4)",
                params!["legacy-delete", "[]", "[]", Utc::now().to_rfc3339()],
            )
            .map_err(|error| RuntimeStateError::Io(error.to_string()))?;
        drop(connection);

        let store = SqliteRuntimeStateStore::new(&path)?;
        let delete = crate::memory::ManagedConversationDelete::prepare("legacy-delete", 10)?;
        let request = ScopeRetirementRequest::prepare("legacy-delete", 0, &delete)?;
        let begun = store.begin_scope_retirement(request.clone()).await?;
        assert_eq!(begun.manifest.items.len(), 1);
        assert_eq!(
            begun
                .manifest
                .items
                .first()
                .map(|item| item.runtime_state_id.as_str()),
            Some("legacy-delete")
        );
        let conversation = store
            .continue_scope_retirement(
                "legacy-delete",
                &request.delete_operation_id,
                begun.scope.revision,
                ScopeRetirementAdvance::ConversationDeleted {
                    receipt: crate::memory::ManagedConversationDeleteReceipt {
                        operation_id: request.delete_operation_id.clone(),
                        payload_digest: request.payload_digest.clone(),
                        deleted_epoch: 10,
                        retention_floor_epoch: 0,
                        status: crate::memory::ManagedConversationDeleteStatus::Deleted,
                    },
                },
            )
            .await?;
        let dropped = store
            .continue_scope_retirement(
                "legacy-delete",
                &request.delete_operation_id,
                conversation.scope.revision,
                ScopeRetirementAdvance::GenerationDropped {
                    runtime_state_id: "legacy-delete".to_string(),
                    pending_operation_id: None,
                },
            )
            .await?;
        let completed = store
            .continue_scope_retirement(
                "legacy-delete",
                &request.delete_operation_id,
                dropped.scope.revision,
                ScopeRetirementAdvance::Complete,
            )
            .await?;
        assert_eq!(completed.status, ScopeRetirementStatus::Completed);
        assert!(store.get_checkpoint("legacy-delete").await?.is_none());

        let unmanaged = store
            .load_runtime_state("legacy", "legacy")
            .await?
            .ok_or_else(|| RuntimeStateError::NotFound("legacy checkpoint".to_string()))?;
        let RuntimeStateVersion::Unmanaged { digest } = unmanaged.version else {
            return Err(RuntimeStateError::SerializationError(
                "legacy checkpoint was not exposed as unmanaged".to_string(),
            )
            .into());
        };
        let adopted = store
            .compare_and_save_checkpoint(RuntimeCheckpointCasRequest {
                scope_id: "legacy".to_string(),
                runtime_state_id: "legacy".to_string(),
                conversation_epoch: Some(1),
                expected_scope_revision: 0,
                expected_state_version: RuntimeStateExpectedVersion::Unmanaged { digest },
                checkpoint: checkpoint("legacy", "adopted")?,
            })
            .await?;
        assert_eq!(adopted.status, RuntimeCheckpointCasStatus::Applied);
        let _ = std::fs::remove_file(path);
        Ok(())
    }

    #[tokio::test]
    async fn generation_retirement_waits_for_pending_projection() -> Result<()> {
        let path = std::env::temp_dir().join(format!(
            "echo-state-pending-retirement-{}-{}.sqlite",
            std::process::id(),
            uuid::Uuid::new_v4()
        ));
        let store = SqliteRuntimeStateStore::new(&path)?;
        let initial = store
            .compare_and_save_checkpoint(RuntimeCheckpointCasRequest {
                scope_id: "scope-p".to_string(),
                runtime_state_id: "runtime-p".to_string(),
                conversation_epoch: Some(1),
                expected_scope_revision: 0,
                expected_state_version: RuntimeStateExpectedVersion::Absent,
                checkpoint: checkpoint("runtime-p", "initial")?,
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
        let _ = std::fs::remove_file(path);
        Ok(())
    }

    #[tokio::test]
    async fn concurrent_generation_writers_cannot_overwrite_each_other() -> Result<()> {
        let path = std::env::temp_dir().join(format!(
            "echo-state-concurrent-cas-{}-{}.sqlite",
            std::process::id(),
            uuid::Uuid::new_v4()
        ));
        let first_store = SqliteRuntimeStateStore::new(&path)?;
        let second_store = SqliteRuntimeStateStore::new(&path)?;
        let barrier = std::sync::Arc::new(tokio::sync::Barrier::new(3));
        let first_barrier = std::sync::Arc::clone(&barrier);
        let first = tokio::spawn(async move {
            first_barrier.wait().await;
            first_store
                .compare_and_save_checkpoint(RuntimeCheckpointCasRequest {
                    scope_id: "scope-c".to_string(),
                    runtime_state_id: "runtime-c".to_string(),
                    conversation_epoch: Some(1),
                    expected_scope_revision: 0,
                    expected_state_version: RuntimeStateExpectedVersion::Absent,
                    checkpoint: checkpoint("runtime-c", "first")?,
                })
                .await
        });
        let second_barrier = std::sync::Arc::clone(&barrier);
        let second = tokio::spawn(async move {
            second_barrier.wait().await;
            second_store
                .compare_and_save_checkpoint(RuntimeCheckpointCasRequest {
                    scope_id: "scope-c".to_string(),
                    runtime_state_id: "runtime-c".to_string(),
                    conversation_epoch: Some(1),
                    expected_scope_revision: 0,
                    expected_state_version: RuntimeStateExpectedVersion::Absent,
                    checkpoint: checkpoint("runtime-c", "second")?,
                })
                .await
        });
        barrier.wait().await;
        let first = first
            .await
            .map_err(|error| RuntimeStateError::Io(error.to_string()))??;
        let second = second
            .await
            .map_err(|error| RuntimeStateError::Io(error.to_string()))??;
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
        let authority = SqliteRuntimeStateStore::new(&path)?
            .load_scope_authority("scope-c")
            .await?
            .ok_or_else(|| RuntimeStateError::NotFound("concurrent scope".to_string()))?;
        assert_eq!(authority.revision, 1);
        let _ = std::fs::remove_file(path);
        Ok(())
    }

    #[tokio::test]
    async fn sqlite_runtime_checkpoint_lifecycle() -> Result<()> {
        let path = std::env::temp_dir().join(format!(
            "echo-state-test-{}-{}.sqlite",
            std::process::id(),
            uuid::Uuid::new_v4()
        ));
        let store = SqliteRuntimeStateStore::new(&path)?;
        let checkpoint = AgentCheckpoint {
            conversation_id: "conv-1".to_string(),
            messages_json: "[]".to_string(),
            current_plan: Some("plan".to_string()),
            active_skills: vec!["coding".to_string()],
            blocked_reason: None,
            working_dir: Some(std::path::PathBuf::from("/tmp/work")),
            timestamp: Utc::now(),
        };
        store.save_checkpoint(&checkpoint).await?;

        let loaded = store
            .get_checkpoint("conv-1")
            .await?
            .ok_or_else(|| RuntimeStateError::Io("checkpoint missing after save".to_string()))?;
        assert_eq!(loaded.active_skills, vec!["coding"]);
        assert_eq!(loaded.working_dir, checkpoint.working_dir);

        store.clear_conversation("conv-1").await?;
        assert!(store.get_checkpoint("conv-1").await?.is_none());
        let _ = std::fs::remove_file(path);
        Ok(())
    }

    #[tokio::test]
    async fn sqlite_scope_index_survives_restart_and_clears_exactly() -> Result<()> {
        let path = std::env::temp_dir().join(format!(
            "echo-state-scope-{}-{}.sqlite",
            std::process::id(),
            uuid::Uuid::new_v4()
        ));
        let checkpoint = |runtime_state_id: &str| AgentCheckpoint {
            conversation_id: runtime_state_id.to_string(),
            messages_json: "[]".to_string(),
            current_plan: None,
            active_skills: Vec::new(),
            blocked_reason: None,
            working_dir: None,
            timestamp: Utc::now(),
        };
        let store = SqliteRuntimeStateStore::new(&path)?;
        store
            .save_checkpoint_for_scope("alice", &checkpoint("alice-1"))
            .await?;
        store
            .save_checkpoint_for_scope("alice", &checkpoint("alice-2"))
            .await?;
        store
            .save_checkpoint_for_scope("bob", &checkpoint("bob-1"))
            .await?;
        assert!(
            store
                .save_checkpoint_for_scope("bob", &checkpoint("alice-2"))
                .await
                .is_err()
        );
        drop(store);

        let restarted = SqliteRuntimeStateStore::new(&path)?;
        assert_eq!(
            restarted.runtime_state_ids("alice").await?,
            vec!["alice-1".to_string(), "alice-2".to_string()]
        );
        assert!(
            restarted
                .clear_runtime_state("alice-2", "alice-2")
                .await
                .is_err()
        );
        assert!(
            restarted
                .clear_runtime_state_scope("alice-2")
                .await?
                .runtime_state_ids
                .is_empty()
        );
        assert!(restarted.clear_conversation("alice-2").await.is_err());
        assert!(restarted.clear_conversation_sync("alice-2").is_err());
        assert!(restarted.get_checkpoint("alice-2").await?.is_some());
        assert!(
            restarted
                .clear_runtime_state("alice", "alice-1")
                .await?
                .checkpoint_removed
        );
        assert!(restarted.get_checkpoint("alice-1").await?.is_none());
        assert!(restarted.get_checkpoint("alice-2").await?.is_some());
        assert!(restarted.get_checkpoint("bob-1").await?.is_some());

        restarted
            .save_checkpoint_for_scope("scope-a", &checkpoint("scope-a-1"))
            .await?;
        restarted
            .save_checkpoint_for_scope("scope-b", &checkpoint("scope-a"))
            .await?;
        let same_name = restarted.clear_runtime_state_scope("scope-a").await?;
        assert_eq!(same_name.runtime_state_ids, vec!["scope-a-1".to_string()]);
        assert!(restarted.get_checkpoint("scope-a-1").await?.is_none());
        assert!(restarted.get_checkpoint("scope-a").await?.is_some());
        assert_eq!(
            restarted.runtime_state_ids("scope-b").await?,
            vec!["scope-a".to_string()]
        );
        let cleared = restarted.clear_runtime_state_scope("alice").await?;
        assert_eq!(cleared.runtime_state_ids, vec!["alice-2".to_string()]);
        assert!(restarted.get_checkpoint("alice-2").await?.is_none());
        assert!(restarted.get_checkpoint("bob-1").await?.is_some());
        let _ = std::fs::remove_file(path);
        Ok(())
    }

    #[tokio::test]
    async fn corrupt_checkpoint_is_rejected() -> Result<()> {
        let path = std::env::temp_dir().join(format!(
            "echo-state-corrupt-{}-{}.sqlite",
            std::process::id(),
            uuid::Uuid::new_v4()
        ));
        let store = SqliteRuntimeStateStore::new(&path)?;
        let connection = store.open_conn()?;
        connection
            .execute(
                "INSERT INTO agent_checkpoints
                 (conversation_id, messages_json, current_plan, active_skills, blocked_reason, working_dir, timestamp)
                 VALUES (?1, ?2, NULL, ?3, NULL, NULL, ?4)",
                params!["corrupt", "[]", "not-json", "not-a-timestamp"],
            )
            .map_err(|error| RuntimeStateError::Io(error.to_string()))?;
        drop(connection);

        let error =
            store.get_checkpoint("corrupt").await.err().ok_or_else(|| {
                RuntimeStateError::Io("corrupt checkpoint was accepted".to_string())
            })?;
        assert!(error.to_string().contains("active_skills"));
        let _ = std::fs::remove_file(path);
        Ok(())
    }

    #[tokio::test(flavor = "current_thread")]
    async fn sqlite_blocking_owner_preserves_runtime_heartbeat() -> Result<()> {
        let path = std::env::temp_dir().join(format!(
            "echo-state-blocking-{}-{}.sqlite",
            std::process::id(),
            uuid::Uuid::new_v4()
        ));
        let store = SqliteRuntimeStateStore::new(&path)?;
        let operation_store = store.clone();
        let (entered_tx, entered_rx) = tokio::sync::oneshot::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let operation = tokio::spawn(async move {
            operation_store
                .run_blocking(
                    SqliteRuntimeStateStore::entity_scope("heartbeat"),
                    move |_store| {
                        let _ignored = entered_tx.send(());
                        release_rx
                            .recv_timeout(Duration::from_secs(2))
                            .map_err(|error| {
                                RuntimeStateError::Io(format!(
                                    "blocking test release failed: {error}"
                                ))
                                .into()
                            })
                    },
                )
                .await
        });
        entered_rx.await.map_err(|error| {
            RuntimeStateError::Io(format!("blocking test did not start: {error}"))
        })?;
        tokio::time::timeout(Duration::from_millis(250), async {
            for _ in 0..64 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .map_err(|_| RuntimeStateError::Io("SQLite stalled the Tokio runtime".to_string()))?;
        release_tx.send(()).map_err(|error| {
            RuntimeStateError::Io(format!("blocking test release failed: {error}"))
        })?;
        operation.await.map_err(|error| {
            RuntimeStateError::Io(format!("blocking test join failed: {error}"))
        })??;
        let _ = std::fs::remove_file(path);
        Ok(())
    }
}
