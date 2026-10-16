//! Scheduler runner — periodic tick loop that fires cron tasks.
//!
//! The [`SchedulerRunner`] accepts an application callback and remains
//! decoupled from any specific Agent or task service.

use super::cron_task::{CronTask, CronTaskStatus, CronTaskStore, SchedulerMutationOwner};
use chrono::{DateTime, Utc};
use echo_core::utils::fs::FileDurability;
use echo_state::delivery::{
    DeliveryClaim, DeliveryEnvelope, DeliveryEvent, DeliveryLedger, DeliveryLedgerConfig,
    DeliveryLedgerProjection, DeliveryOutcome, DeliveryPhase, DeliveryRecord, DeliverySettlement,
};
use echo_state::journal::{
    ApplyReceipt, CheckpointStore, EventJournal, FileCheckpointStore, FileEventJournal,
    JournalDurabilityStatus, PreparedJournalBatch,
};
use futures::future::BoxFuture;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex as StdMutex, OnceLock, Weak};
use tokio::sync::{Mutex, RwLock};
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;
use tracing::{debug, info, warn};

/// Type alias for the fire function: takes a CronTask, returns a future with the result.
pub type FireFn =
    Arc<dyn Fn(CronTask) -> BoxFuture<'static, echo_core::error::Result<String>> + Send + Sync>;

/// Callback that receives durable identity for one scheduler delivery attempt.
///
/// `occurrence_id` remains stable when an owner-loss recovery retries a
/// callback. `attempt_id` changes for every physical execution attempt. A
/// callback that performs non-idempotent external effects should use
/// `occurrence_id` as its idempotency key.
pub type OccurrenceFireFn = Arc<
    dyn Fn(SchedulerInvocation) -> BoxFuture<'static, echo_core::error::Result<String>>
        + Send
        + Sync,
>;

/// Source of one durable scheduler occurrence.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
#[serde(rename_all = "snake_case")]
pub enum SchedulerTrigger {
    /// A cron expression produced the occurrence.
    Scheduled,
    /// A caller requested an immediate one-off execution.
    Manual,
}

/// Exact callback attempt delivered by [`SchedulerRunner`].
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct SchedulerInvocation {
    /// Stable idempotency identity shared by every retry of this occurrence.
    pub occurrence_id: String,
    /// Opaque identity for this physical callback attempt.
    pub attempt_id: String,
    /// Monotonic attempt number within the occurrence.
    pub attempt: u32,
    /// Task definition captured when the occurrence was durably persisted.
    pub task: CronTask,
    /// Original scheduled time, or the request time for a manual run.
    pub scheduled_at: DateTime<Utc>,
    /// Whether the occurrence came from cron polling or `run_once`.
    pub trigger: SchedulerTrigger,
    /// True when a prior callback owner disappeared after effect admission.
    pub recovered_after_owner_loss: bool,
}

impl SchedulerInvocation {
    /// Construct a first callback attempt for an application-owned occurrence.
    ///
    /// The durable scheduler overwrites these attempt fields when it delivers
    /// its own journaled occurrences. This constructor exists for embedding
    /// adapters and source-compatible callback wrappers, which cannot use a
    /// struct literal because this contract is non-exhaustive.
    pub fn new(
        occurrence_id: impl Into<String>,
        task: CronTask,
        scheduled_at: DateTime<Utc>,
        trigger: SchedulerTrigger,
    ) -> Self {
        Self {
            occurrence_id: occurrence_id.into(),
            attempt_id: uuid::Uuid::new_v4().to_string(),
            attempt: 1,
            task,
            scheduled_at,
            trigger,
            recovered_after_owner_loss: false,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum OccurrenceProtection {
    LegacyAtLeastOnce,
    PhysicalSettlementV1,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(from = "OccurrencePayloadWire", into = "OccurrencePayloadWire")]
struct OccurrencePayload {
    task: CronTask,
    scheduled_at: DateTime<Utc>,
    trigger: SchedulerTrigger,
    control_revision: u64,
    protection: OccurrenceProtection,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct OccurrencePayloadFields {
    task: CronTask,
    scheduled_at: DateTime<Utc>,
    trigger: SchedulerTrigger,
    #[serde(default)]
    control_revision: u64,
}

// A protected wrapper has no legacy top-level task, so old decoders fail
// closed instead of ignoring a new flag and replaying a protected effect.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
enum OccurrencePayloadWire {
    ProtectedV1 {
        physical_settlement_v1: OccurrencePayloadFields,
    },
    Legacy(OccurrencePayloadFields),
}

impl From<OccurrencePayload> for OccurrencePayloadWire {
    fn from(value: OccurrencePayload) -> Self {
        let fields = OccurrencePayloadFields {
            task: value.task,
            scheduled_at: value.scheduled_at,
            trigger: value.trigger,
            control_revision: value.control_revision,
        };
        match value.protection {
            OccurrenceProtection::PhysicalSettlementV1 => Self::ProtectedV1 {
                physical_settlement_v1: fields,
            },
            OccurrenceProtection::LegacyAtLeastOnce => Self::Legacy(fields),
        }
    }
}

impl From<OccurrencePayloadWire> for OccurrencePayload {
    fn from(value: OccurrencePayloadWire) -> Self {
        let (fields, protection) = match value {
            OccurrencePayloadWire::ProtectedV1 {
                physical_settlement_v1,
            } => (
                physical_settlement_v1,
                OccurrenceProtection::PhysicalSettlementV1,
            ),
            OccurrencePayloadWire::Legacy(fields) => {
                (fields, OccurrenceProtection::LegacyAtLeastOnce)
            }
        };
        Self {
            task: fields.task,
            scheduled_at: fields.scheduled_at,
            trigger: fields.trigger,
            control_revision: fields.control_revision,
            protection,
        }
    }
}

type OccurrenceEvent = DeliveryEvent<String, OccurrencePayload>;
type OccurrenceJournal = FileEventJournal<OccurrenceEvent>;
type OccurrenceProjection = DeliveryLedgerProjection<String, OccurrencePayload>;
type OccurrenceLedger = DeliveryLedger<OccurrenceJournal, String, OccurrencePayload>;

struct OccurrenceAuthority {
    journal: Arc<OccurrenceJournal>,
    ledger: OccurrenceLedger,
    operation: StdMutex<()>,
    active_attempts: StdMutex<HashSet<String>>,
    latest_scheduled_at: StdMutex<HashMap<(String, String), DateTime<Utc>>>,
    delivery_lock: Mutex<()>,
    #[cfg(test)]
    settle_after_commit_fault: std::sync::atomic::AtomicBool,
    #[cfg(test)]
    cold_scans: std::sync::atomic::AtomicUsize,
    #[cfg(test)]
    enqueue_after_commit_fault: std::sync::atomic::AtomicBool,
}

/// Detects a dropped callback owner while the process remains alive.
///
/// This set is only a liveness hint. The journaled `EffectStarted` and
/// `OutcomeUnknown` transitions remain the recovery authority.
struct CallbackOwnerGuard {
    authority: Weak<OccurrenceAuthority>,
    attempt_id: String,
}

impl Drop for CallbackOwnerGuard {
    fn drop(&mut self) {
        let Some(authority) = self.authority.upgrade() else {
            return;
        };
        let mut active = match authority.active_attempts.lock() {
            Ok(active) => active,
            Err(error) => error.into_inner(),
        };
        active.remove(&self.attempt_id);
    }
}

fn occurrence_authorities() -> &'static StdMutex<HashMap<PathBuf, Weak<OccurrenceAuthority>>> {
    static AUTHORITIES: OnceLock<StdMutex<HashMap<PathBuf, Weak<OccurrenceAuthority>>>> =
        OnceLock::new();
    AUTHORITIES.get_or_init(|| StdMutex::new(HashMap::new()))
}

/// A scheduled occurrence retains both the public task ID and the definition
/// identity captured from the store.  The scheduled timestamp makes repeated
/// cron ticks address the same occurrence instead of treating an entire task
/// as the deduplication unit.
#[derive(Debug, Clone)]
struct ScheduledOccurrence {
    task: CronTask,
    scheduled_at: DateTime<Utc>,
}

type LastFiredByTask = HashMap<String, (String, DateTime<Utc>)>;
const SCHEDULED_OCCURRENCE_ID_VERSION: u8 = 1;

fn definition_identity(task: &CronTask) -> &str {
    if task.definition_id.is_empty() {
        &task.created_at
    } else {
        &task.definition_id
    }
}

fn definition_key(task: &CronTask) -> (String, String) {
    (task.id.clone(), definition_identity(task).to_string())
}

impl OccurrenceAuthority {
    fn enqueue_if_absent(
        &self,
        occurrence_id: &str,
        payload: OccurrencePayload,
    ) -> echo_core::error::Result<bool> {
        let _operation = self.operation.lock().map_err(|error| {
            echo_core::error::ReactError::Other(format!(
                "Scheduler occurrence authority lock poisoned: {error}"
            ))
        })?;
        self.enqueue_if_absent_unlocked(occurrence_id, payload, false)
    }

    fn enqueue_generated_manual(
        &self,
        payload: OccurrencePayload,
    ) -> echo_core::error::Result<String> {
        if payload.trigger != SchedulerTrigger::Manual {
            return Err(ledger_error(
                "manual run",
                "generated manual admission received a scheduled payload",
            ));
        }
        let _operation = self.operation.lock().map_err(|error| {
            echo_core::error::ReactError::Other(format!(
                "Scheduler occurrence authority lock poisoned: {error}"
            ))
        })?;
        // The journal generation and next sequence identify an unused slot
        // while this same operation lock owns the subsequent append.
        let occurrence_id = format!(
            "cron-manual:{}:{}",
            self.journal.journal_identity(),
            self.journal.next_sequence()
        );
        if !self.enqueue_if_absent_unlocked(&occurrence_id, payload, true)? {
            return Err(ledger_error(
                "manual run",
                "generated journal sequence already owns an occurrence",
            ));
        }
        Ok(occurrence_id)
    }

    fn enqueue_if_absent_unlocked(
        &self,
        occurrence_id: &str,
        payload: OccurrencePayload,
        generated_manual: bool,
    ) -> echo_core::error::Result<bool> {
        if self
            .ledger
            .with_projection(|projection| projection.record(occurrence_id).is_some())
        {
            return Ok(false);
        }
        let key =
            (payload.trigger == SchedulerTrigger::Scheduled).then(|| definition_key(&payload.task));
        let previous_max = {
            let latest = self
                .latest_scheduled_at
                .lock()
                .map_err(|error| ledger_error("scheduler occurrence watermark", error))?;
            key.as_ref().and_then(|key| latest.get(key).copied())
        };
        let definitely_newer = generated_manual
            || (key.is_some() && previous_max.is_none_or(|latest| payload.scheduled_at > latest));
        if !definitely_newer && self.journal_contains(occurrence_id)? {
            return Ok(false);
        }
        let scheduled_at = payload.scheduled_at;
        let route = payload.task.id.clone();
        let receipt = self
            .ledger
            .enqueue(DeliveryEnvelope::new(occurrence_id, route, payload))
            .map_err(|error| ledger_error("persist scheduler occurrence", error))?;
        if let Some(key) = key {
            let mut latest = self
                .latest_scheduled_at
                .lock()
                .map_err(|error| ledger_error("scheduler occurrence watermark", error))?;
            latest
                .entry(key)
                .and_modify(|current| {
                    if scheduled_at > *current {
                        *current = scheduled_at;
                    }
                })
                .or_insert(scheduled_at);
        }
        #[cfg(test)]
        if self
            .enqueue_after_commit_fault
            .swap(false, std::sync::atomic::Ordering::AcqRel)
        {
            return Err(ledger_error(
                "persist scheduler occurrence",
                format!("injected error after committing {occurrence_id}"),
            ));
        }
        self.confirm_durable(&receipt, "persist scheduler occurrence")?;
        Ok(true)
    }

    fn journal_contains(&self, occurrence_id: &str) -> echo_core::error::Result<bool> {
        #[cfg(test)]
        self.cold_scans
            .fetch_add(1, std::sync::atomic::Ordering::AcqRel);
        let mut after = self.journal.retained_floor().saturating_sub(1);
        loop {
            let records = self.journal.replay_after(after, 128)?;
            if records.is_empty() {
                return Ok(false);
            }
            for record in records {
                if matches!(record.event.as_ref(), OccurrenceEvent::Persisted { envelope, .. }
                    if envelope.message_id == occurrence_id)
                {
                    return Ok(true);
                }
                after = record.sequence;
            }
        }
    }

    fn claim_next(
        &self,
    ) -> echo_core::error::Result<Option<(DeliveryClaim<String, OccurrencePayload>, bool)>> {
        let _operation = self.operation.lock().map_err(|error| {
            echo_core::error::ReactError::Other(format!(
                "Scheduler occurrence authority lock poisoned: {error}"
            ))
        })?;
        self.reconcile_inactive_owner_loss_unlocked()?;
        let Some(mut draft) = self
            .ledger
            .prepare_claim_next_available()
            .map_err(|error| ledger_error("prepare scheduler occurrence claim", error))?
        else {
            return Ok(None);
        };
        let recovered_after_owner_loss = self.ledger.with_projection(|projection| {
            projection
                .record(&draft.claim.message_id)
                .is_some_and(|record| record.outcome == Some(DeliveryOutcome::OutcomeUnknown))
        });
        if recovered_after_owner_loss {
            // Old history may have its first recovery; this newly admitted
            // physical attempt must be protected in the same original batch.
            draft.claim.payload.protection = OccurrenceProtection::PhysicalSettlementV1;
            let batch = PreparedJournalBatch::new(vec![
                draft.event,
                DeliveryEvent::EffectStarted {
                    replacement_payload: Some(draft.claim.payload.clone()),
                    message_id: draft.claim.message_id.clone(),
                    attempt_id: draft.claim.attempt_id.clone(),
                    turn_id: draft.claim.attempt_id.clone(),
                    started_at: Utc::now(),
                },
            ])
            .map_err(|error| ledger_error("prepare scheduler recovery admission", error))?;
            let receipt = self
                .ledger
                .apply_prepared(batch)
                .map_err(|error| ledger_error("commit scheduler recovery admission", error))?;
            if receipt.journal != JournalDurabilityStatus::Confirmed {
                self.journal.sync_data()?;
            }
        } else {
            let receipt = self
                .ledger
                .apply(draft.event)
                .map_err(|error| ledger_error("commit scheduler occurrence claim", error))?;
            self.confirm_durable(&receipt, "commit scheduler occurrence claim")?;
        }
        Ok(Some((draft.claim, recovered_after_owner_loss)))
    }

    fn begin_effect(
        self: &Arc<Self>,
        claim: &DeliveryClaim<String, OccurrencePayload>,
    ) -> echo_core::error::Result<CallbackOwnerGuard> {
        let _operation = self.operation.lock().map_err(|error| {
            echo_core::error::ReactError::Other(format!(
                "Scheduler occurrence authority lock poisoned: {error}"
            ))
        })?;
        let admitted = self.ledger.with_projection(|projection| {
            projection.record(&claim.message_id).is_some_and(|record| {
                record.phase == DeliveryPhase::EffectStarted
                    && record.attempt_id.as_deref() == Some(claim.attempt_id.as_str())
            })
        });
        if !admitted {
            let mut payload = claim.payload.clone();
            payload.protection = OccurrenceProtection::PhysicalSettlementV1;
            let mut event = self
                .ledger
                .prepare_begin_effect(claim, claim.attempt_id.clone())
                .map_err(|error| ledger_error("prepare scheduler callback admission", error))?;
            if let DeliveryEvent::EffectStarted {
                replacement_payload,
                ..
            } = &mut event
            {
                *replacement_payload = Some(payload);
            }
            let receipt = self
                .ledger
                .apply(event)
                .map_err(|error| ledger_error("commit scheduler callback admission", error))?;
            self.confirm_durable(&receipt, "commit scheduler callback admission")?;
        }
        self.active_attempts
            .lock()
            .map_err(|error| {
                echo_core::error::ReactError::Other(format!(
                    "Scheduler active callback lock poisoned: {error}"
                ))
            })?
            .insert(claim.attempt_id.clone());
        Ok(CallbackOwnerGuard {
            authority: Arc::downgrade(self),
            attempt_id: claim.attempt_id.clone(),
        })
    }

    fn settle(
        &self,
        claim: &DeliveryClaim<String, OccurrencePayload>,
        settlement: DeliverySettlement,
    ) -> echo_core::error::Result<()> {
        let _operation = self.operation.lock().map_err(|error| {
            echo_core::error::ReactError::Other(format!(
                "Scheduler occurrence authority lock poisoned: {error}"
            ))
        })?;
        let receipt = self
            .ledger
            .settle(claim, settlement)
            .map_err(|error| ledger_error("settle scheduler occurrence", error))?;
        #[cfg(test)]
        if self
            .settle_after_commit_fault
            .swap(false, std::sync::atomic::Ordering::AcqRel)
        {
            return Err(ledger_error(
                "settle scheduler occurrence",
                "injected error after durable terminal commit",
            ));
        }
        self.confirm_durable(&receipt, "settle scheduler occurrence")
    }

    fn reconcile_owner_loss(&self) -> echo_core::error::Result<()> {
        let _operation = self.operation.lock().map_err(|error| {
            echo_core::error::ReactError::Other(format!(
                "Scheduler occurrence authority lock poisoned: {error}"
            ))
        })?;
        self.reconcile_inactive_owner_loss_unlocked()
    }

    fn reconcile_inactive_owner_loss_unlocked(&self) -> echo_core::error::Result<()> {
        let active_attempts = self.active_attempts.lock().map_err(|error| {
            echo_core::error::ReactError::Other(format!(
                "Scheduler active callback lock poisoned: {error}"
            ))
        })?;
        let interrupted = self.ledger.with_projection(|projection| {
            projection
                .frontier()
                .filter(|record| {
                    matches!(
                        record.phase,
                        DeliveryPhase::EffectStarted
                            | DeliveryPhase::MailboxAccepted
                            | DeliveryPhase::Drained
                    )
                })
                .filter(|record| {
                    record.payload.protection == OccurrenceProtection::LegacyAtLeastOnce
                })
                .filter(|record| {
                    record
                        .attempt_id
                        .as_ref()
                        .is_none_or(|attempt_id| !active_attempts.contains(attempt_id))
                })
                .map(claim_from_record)
                .collect::<echo_core::error::Result<Vec<_>>>()
        })?;
        drop(active_attempts);
        for claim in interrupted {
            warn!(
                occurrence_id = %claim.message_id,
                attempt_id = %claim.attempt_id,
                "Scheduler callback owner disappeared; marking outcome unknown for at-least-once replay"
            );
            let receipt = self
                .ledger
                .settle(
                    &claim,
                    DeliverySettlement::retry(
                        Some(claim.attempt_id.clone()),
                        DeliveryOutcome::OutcomeUnknown,
                        None,
                        Some(
                            "scheduler callback owner disappeared before durable settlement".into(),
                        ),
                        Utc::now(),
                    ),
                )
                .map_err(|error| ledger_error("reconcile scheduler callback owner loss", error))?;
            self.confirm_durable(&receipt, "reconcile scheduler callback owner loss")?;
        }
        Ok(())
    }

    fn confirm_durable(
        &self,
        receipt: &ApplyReceipt,
        operation: &str,
    ) -> echo_core::error::Result<()> {
        if receipt.journal == JournalDurabilityStatus::Confirmed {
            return Ok(());
        }
        self.journal.sync_data().map_err(|error| {
            echo_core::error::ReactError::Other(format!(
                "{operation} committed but durability could not be confirmed: {error}"
            ))
        })
    }
}

fn claim_from_record(
    record: &DeliveryRecord<String, OccurrencePayload>,
) -> echo_core::error::Result<DeliveryClaim<String, OccurrencePayload>> {
    let attempt_id = record.attempt_id.clone().ok_or_else(|| {
        echo_core::error::ReactError::Other(format!(
            "Interrupted scheduler occurrence '{}' has no attempt identity",
            record.message_id
        ))
    })?;
    let claimed_at = record.claimed_at.ok_or_else(|| {
        echo_core::error::ReactError::Other(format!(
            "Interrupted scheduler occurrence '{}' has no claim timestamp",
            record.message_id
        ))
    })?;
    Ok(DeliveryClaim {
        payload: record.payload.clone(),
        message_id: record.message_id.clone(),
        route: record.route.clone(),
        attempt_id,
        attempt: record.attempt,
        claimed_at,
    })
}

fn open_occurrence_authority(
    store: &CronTaskStore,
    tasks: &[CronTask],
) -> echo_core::error::Result<Arc<OccurrenceAuthority>> {
    let (journal_path, checkpoint_path) = store.occurrence_paths()?;
    let journal_path = canonical_sibling_path(&journal_path)?;
    let checkpoint_path = canonical_sibling_path(&checkpoint_path)?;
    let mut authorities = occurrence_authorities().lock().map_err(|error| {
        echo_core::error::ReactError::Other(format!(
            "Scheduler occurrence registry lock poisoned: {error}"
        ))
    })?;
    authorities.retain(|_, authority| authority.strong_count() > 0);
    if authorities
        .get(&journal_path)
        .and_then(Weak::upgrade)
        .is_some()
    {
        return Err(echo_core::error::ReactError::Other(format!(
            "Scheduler occurrence journal '{}' already has an active runner",
            journal_path.display()
        )));
    }

    let journal = Arc::new(OccurrenceJournal::open(
        &journal_path,
        FileDurability::SyncData,
    )?);
    let checkpoints: Arc<dyn CheckpointStore<OccurrenceProjection>> =
        Arc::new(FileCheckpointStore::open(checkpoint_path));
    let ledger = OccurrenceLedger::new(
        Arc::clone(&journal),
        checkpoints,
        DeliveryLedgerConfig::default(),
        32,
    );
    ledger.recover()?;
    let latest_scheduled_at = load_latest_scheduled_at(&journal, tasks)?;
    let authority = Arc::new(OccurrenceAuthority {
        journal,
        ledger,
        operation: StdMutex::new(()),
        active_attempts: StdMutex::new(HashSet::new()),
        latest_scheduled_at: StdMutex::new(latest_scheduled_at),
        delivery_lock: Mutex::new(()),
        #[cfg(test)]
        settle_after_commit_fault: std::sync::atomic::AtomicBool::new(false),
        #[cfg(test)]
        cold_scans: std::sync::atomic::AtomicUsize::new(0),
        #[cfg(test)]
        enqueue_after_commit_fault: std::sync::atomic::AtomicBool::new(false),
    });
    authority.reconcile_owner_loss()?;
    authorities.insert(journal_path, Arc::downgrade(&authority));
    Ok(authority)
}

fn load_latest_scheduled_at(
    journal: &OccurrenceJournal,
    active_tasks: &[CronTask],
) -> echo_core::error::Result<HashMap<(String, String), DateTime<Utc>>> {
    // Bound the rebuildable watermark to current definitions, not to the
    // number of historical occurrences or removed task generations.
    let active = active_tasks
        .iter()
        .map(definition_key)
        .collect::<HashSet<_>>();
    let mut latest = HashMap::new();
    if active.is_empty() {
        return Ok(latest);
    }
    let mut after = journal.retained_floor().saturating_sub(1);
    loop {
        let records = journal.replay_after(after, 128)?;
        if records.is_empty() {
            return Ok(latest);
        }
        for record in records {
            if let OccurrenceEvent::Persisted { envelope, .. } = record.event.as_ref()
                && envelope.payload.trigger == SchedulerTrigger::Scheduled
            {
                let key = definition_key(&envelope.payload.task);
                if active.contains(&key) {
                    latest
                        .entry(key)
                        .and_modify(|current| {
                            if envelope.payload.scheduled_at > *current {
                                *current = envelope.payload.scheduled_at;
                            }
                        })
                        .or_insert(envelope.payload.scheduled_at);
                }
            }
            after = record.sequence;
        }
    }
}

fn canonical_sibling_path(path: &Path) -> echo_core::error::Result<PathBuf> {
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    std::fs::create_dir_all(parent).map_err(|error| {
        echo_core::error::ReactError::Other(format!(
            "Failed to create scheduler occurrence directory: {error}"
        ))
    })?;
    let canonical_parent = std::fs::canonicalize(parent).map_err(|error| {
        echo_core::error::ReactError::Other(format!(
            "Failed to resolve scheduler occurrence directory: {error}"
        ))
    })?;
    let file_name = path.file_name().ok_or_else(|| {
        echo_core::error::ReactError::Other(
            "Scheduler occurrence path has no file name".to_string(),
        )
    })?;
    Ok(canonical_parent.join(file_name))
}

fn scheduled_occurrence_id(
    task: &CronTask,
    scheduled_at: DateTime<Utc>,
) -> echo_core::error::Result<String> {
    let identity = serde_json::to_vec(&(
        SCHEDULED_OCCURRENCE_ID_VERSION,
        task.id.as_str(),
        task.definition_id.as_str(),
        task.created_at.as_str(),
        scheduled_at,
    ))
    .map_err(|error| {
        echo_core::error::ReactError::Other(format!(
            "Failed to encode scheduler occurrence identity: {error}"
        ))
    })?;
    let digest = Sha256::digest(identity);
    Ok(format!("cron-scheduled:{digest:x}"))
}

fn ledger_error(operation: &str, error: impl std::fmt::Display) -> echo_core::error::ReactError {
    echo_core::error::ReactError::Other(format!("Failed to {operation}: {error}"))
}

async fn run_owned_mutation<T, F>(operation: &str, future: F) -> echo_core::error::Result<T>
where
    T: Send + 'static,
    F: std::future::Future<Output = echo_core::error::Result<T>> + Send + 'static,
{
    let runtime = tokio::runtime::Handle::try_current()
        .map_err(|error| ledger_error(operation, format!("Tokio runtime unavailable: {error}")))?;
    runtime
        .spawn(future)
        .await
        .map_err(|error| ledger_error(operation, error))?
}

/// Owned handle for a running scheduler loop.
pub struct SchedulerHandle {
    cancel: CancellationToken,
    join: Option<JoinHandle<()>>,
    join_error: Option<String>,
    owner: Arc<SchedulerRunner>,
}

impl SchedulerHandle {
    /// Signal the existing scheduler token; admitted callbacks keep their owner.
    pub fn cancel(&self) {
        self.cancel.cancel();
    }

    /// Whether the loop ended, independently of physical callback settlement.
    pub fn is_finished(&self) -> bool {
        self.join.as_ref().is_none_or(JoinHandle::is_finished)
    }

    /// Observe the same loop without consuming its parent. Retain this handle
    /// after an uncertain result; repeated observation never executes again.
    pub async fn join(&mut self) -> echo_core::error::Result<()> {
        if let Some(join) = self.join.as_mut() {
            let result = join.await;
            self.join.take();
            self.join_error = result.err().map(|error| error.to_string());
        }
        if let Some(error) = self.owner.unsettled_occurrence() {
            return Err(error);
        }
        match self.join_error.as_ref() {
            Some(error) => Err(ledger_error("scheduler loop join", error)),
            None => Ok(()),
        }
    }

    /// Signal shutdown and observe its loop. An unknown callback is not a
    /// successful physical shutdown; the same caller-held parent is preserved.
    pub async fn shutdown(&mut self) -> echo_core::error::Result<()> {
        self.cancel();
        self.join().await
    }
}

/// Periodic scheduler that checks and fires cron tasks.
///
/// Runs a tokio background task that polls every 30 seconds.
/// Uses `last_fired` tracking to prevent double-firing.
pub struct SchedulerRunner {
    store: CronTaskStore,
    mutation_owner: SchedulerMutationOwner,
    fire_fn: OccurrenceFireFn,
    occurrence_authority: Arc<OccurrenceAuthority>,
    tasks: Arc<RwLock<Vec<CronTask>>>,
    last_fired: Arc<RwLock<LastFiredByTask>>,
    last_tick_at: Arc<RwLock<DateTime<Utc>>>,
    control_lock: Arc<Mutex<()>>,
    cancel: CancellationToken,
}

struct AdmittedOccurrence {
    claim: DeliveryClaim<String, OccurrencePayload>,
    recovered_after_owner_loss: bool,
    completion: JoinHandle<echo_core::error::Result<String>>,
}

struct CallbackSettlementOwner {
    store: CronTaskStore,
    mutation_owner: SchedulerMutationOwner,
    fire_fn: OccurrenceFireFn,
    occurrence_authority: Arc<OccurrenceAuthority>,
    tasks: Arc<RwLock<Vec<CronTask>>>,
    control_lock: Arc<Mutex<()>>,
}

impl CallbackSettlementOwner {
    async fn run(
        self,
        claim: DeliveryClaim<String, OccurrencePayload>,
        recovered_after_owner_loss: bool,
        callback_owner: CallbackOwnerGuard,
    ) -> echo_core::error::Result<String> {
        let payload = claim.payload.clone();
        let callback_result = (self.fire_fn)(SchedulerInvocation {
            occurrence_id: claim.message_id.clone(),
            attempt_id: claim.attempt_id.clone(),
            attempt: claim.attempt,
            task: payload.task.clone(),
            scheduled_at: payload.scheduled_at,
            trigger: payload.trigger,
            recovered_after_owner_loss,
        })
        .await;
        if matches!(
            &callback_result,
            Err(echo_core::error::ReactError::ExecutionNotSettled { .. })
        ) {
            // This future returned observation, not physical settlement. Its
            // original protected claim survives the liveness guard's Drop.
            return callback_result;
        }
        let (outcome, result_text, callback_error) = match callback_result {
            Ok(result) => (DeliveryOutcome::Completed, result, None),
            Err(error) => {
                let error_text = error.to_string();
                (
                    DeliveryOutcome::Failed,
                    format!("ERROR: {error_text}"),
                    Some(error),
                )
            }
        };
        let projection_result = {
            let store = self.store.clone();
            let owner = self.mutation_owner.clone();
            let control = Arc::clone(&self.control_lock);
            let tasks = Arc::clone(&self.tasks);
            let task = payload.task.clone();
            let result = result_text.clone();
            run_owned_mutation("last-run projection owner", async move {
                let _control = control.lock().await;
                let mut tasks = tasks.write().await;
                let updated = match store
                    .update_last_run_for_task(&task, &result, Some(&owner))
                    .await
                {
                    Ok(updated) => updated,
                    Err(error) => {
                        if store.is_authority_poisoned() {
                            tasks.clear();
                        }
                        return Err(error);
                    }
                };
                if let Some(cached) = tasks
                    .iter_mut()
                    .find(|candidate| candidate.same_definition(&updated))
                {
                    *cached = updated;
                } else {
                    tasks.push(updated);
                }
                Ok::<(), echo_core::error::ReactError>(())
            })
            .await
        };
        let settlement_reason = match (&callback_error, &projection_result) {
            (Some(callback), Err(projection)) => Some(format!(
                "callback failed: {callback}; last-run projection failed: {projection}"
            )),
            (Some(callback), Ok(())) => Some(callback.to_string()),
            (None, Err(projection)) => Some(format!("last-run projection failed: {projection}")),
            (None, Ok(())) => None,
        };
        let settlement_result = {
            let _delivery = self.occurrence_authority.delivery_lock.lock().await;
            self.occurrence_authority.settle(
                &claim,
                DeliverySettlement::terminal(
                    Some(claim.attempt_id.clone()),
                    outcome,
                    None,
                    settlement_reason,
                    None,
                ),
            )
        };

        drop(callback_owner);
        settlement_result?;
        projection_result?;
        match callback_error {
            Some(error) => Err(error),
            None => Ok(result_text),
        }
    }
}

impl SchedulerRunner {
    /// Create a scheduler with the source-compatible task-only callback.
    ///
    /// The callback still uses the canonical durable occurrence ledger, but it
    /// cannot observe the stable idempotency identity. New integrations that
    /// perform external effects should use
    /// [`Self::new_with_occurrence_context`].
    pub async fn new(
        store: CronTaskStore,
        cancel: CancellationToken,
        fire_fn: FireFn,
    ) -> echo_core::error::Result<Self> {
        let occurrence_fire_fn: OccurrenceFireFn =
            Arc::new(move |invocation| (fire_fn)(invocation.task));
        Self::new_with_occurrence_context(store, cancel, occurrence_fire_fn).await
    }

    /// Create a scheduler whose callback can make external effects idempotent.
    /// Claims definition mutation ownership before loading the task cache.
    pub async fn new_with_occurrence_context(
        store: CronTaskStore,
        cancel: CancellationToken,
        fire_fn: OccurrenceFireFn,
    ) -> echo_core::error::Result<Self> {
        let (mutation_owner, tasks) = store.claim_runner().await?;
        let occurrence_authority = open_occurrence_authority(&store, &tasks)?;
        Ok(Self {
            store,
            mutation_owner,
            fire_fn,
            occurrence_authority,
            tasks: Arc::new(RwLock::new(tasks)),
            last_fired: Arc::new(RwLock::new(HashMap::new())),
            last_tick_at: Arc::new(RwLock::new(Utc::now())),
            control_lock: Arc::new(Mutex::new(())),
            cancel,
        })
    }

    // Construct exactly one retained execution after original durable admission.
    fn start_admitted_callback(
        &self,
        mut claim: DeliveryClaim<String, OccurrencePayload>,
        recovered_after_owner_loss: bool,
    ) -> echo_core::error::Result<AdmittedOccurrence> {
        let runtime = tokio::runtime::Handle::try_current()
            .map_err(|error| ledger_error("callback owner runtime", error))?;
        let owner = CallbackSettlementOwner {
            store: self.store.clone(),
            mutation_owner: self.mutation_owner.clone(),
            fire_fn: self.fire_fn.clone(),
            occurrence_authority: self.occurrence_authority.clone(),
            tasks: self.tasks.clone(),
            control_lock: self.control_lock.clone(),
        };
        claim.payload.protection = OccurrenceProtection::PhysicalSettlementV1;
        let callback_owner = self.occurrence_authority.begin_effect(&claim)?;
        let owned_claim = claim.clone();
        let completion = runtime.spawn(async move {
            owner
                .run(owned_claim, recovered_after_owner_loss, callback_owner)
                .await
        });
        Ok(AdmittedOccurrence {
            claim,
            recovered_after_owner_loss,
            completion,
        })
    }

    fn unsettled_occurrence(&self) -> Option<echo_core::error::ReactError> {
        self.occurrence_authority
            .ledger
            .with_projection(|projection| {
                projection
                    .frontier()
                    .find(|record| {
                        record.payload.protection == OccurrenceProtection::PhysicalSettlementV1
                            && record.effect_started_at.is_some()
                    })
                    .map(|record| echo_core::error::ReactError::ExecutionNotSettled {
                        run_id: record.message_id.clone(),
                        execution_id: record.attempt_id.clone(),
                    })
            })
    }

    /// Spawn the existing loop and return its explicit parent owner. Keep the
    /// handle when join/shutdown reports unresolved physical callbacks.
    pub fn spawn(self: Arc<Self>) -> SchedulerHandle {
        let cancel = self.cancel.clone();
        let owner = self.clone();
        let join = tokio::spawn(async move {
            self.run_loop().await;
        });
        SchedulerHandle {
            cancel,
            join: Some(join),
            join_error: None,
            owner,
        }
    }

    /// Main loop: tick every 30 seconds, checking and firing due tasks.
    async fn run_loop(&self) {
        info!("Scheduler runner started (30s tick interval)");
        if let Err(error) = self.drain_pending(None).await {
            warn!(%error, "Failed to replay pending scheduler occurrences");
        }
        loop {
            tokio::select! {
                _ = self.cancel.cancelled() => {
                    info!("Scheduler runner cancelled");
                    return;
                }
                _ = tokio::time::sleep(std::time::Duration::from_secs(30)) => {
                    if let Err(error) = self.drain_pending(None).await {
                        warn!(%error, "Failed to retry pending scheduler occurrences");
                    }
                    self.tick().await;
                }
            }
        }
    }

    /// Single tick: check all enabled tasks and fire those that are due.
    async fn tick(&self) {
        let now = Utc::now();
        self.tick_at(now).await;
    }

    async fn tick_at(&self, now: DateTime<Utc>) {
        if let Err(error) = self.store.check_authority() {
            warn!(%error, "Scheduler definition authority is unavailable; tick blocked");
            return;
        }
        let window_start = *self.last_tick_at.read().await;
        let tasks = self.tasks.read().await;

        let mut to_fire: Vec<ScheduledOccurrence> = Vec::new();
        let last_fired = self.last_fired.read().await;

        for task in tasks.iter() {
            if task.status != CronTaskStatus::Enabled {
                continue;
            }
            if let Some(next) = task.next_run_after(&window_start) {
                // Check if next_run is within the window
                if next <= now {
                    // The in-process reservation is made only after durable
                    // admission or an explicit control-gate suppression.
                    if let Some((definition_id, last)) = last_fired.get(&task.id)
                        && definition_id == definition_identity(task)
                        && *last == next
                    {
                        debug!(task = %task.name, "Skipping double-fire");
                        continue;
                    }
                    to_fire.push(ScheduledOccurrence {
                        task: task.clone(),
                        scheduled_at: next,
                    });
                }
            }
        }
        drop(tasks);
        drop(last_fired);

        // Persist the whole due set before awaiting any callback. A hung
        // callback cannot prevent later tasks in the same tick from reaching
        // the journal, and an interrupted admission leaves the window open.
        let mut failed_admission = false;
        let mut admitted = Vec::new();
        for occurrence in to_fire {
            let identity = (
                definition_identity(&occurrence.task).to_string(),
                occurrence.scheduled_at,
            );
            let task_id = occurrence.task.id.clone();
            match self.enqueue_scheduled(&occurrence).await {
                Ok(occurrence_id) => {
                    let _control = self.control_lock.lock().await;
                    let still_present = self
                        .tasks
                        .read()
                        .await
                        .iter()
                        .any(|task| task.id == task_id);
                    if still_present {
                        self.last_fired.write().await.insert(task_id, identity);
                    } else {
                        self.last_fired.write().await.remove(&task_id);
                    }
                    if let Some(occurrence_id) = occurrence_id {
                        admitted.push(occurrence_id);
                    }
                }
                Err(error) => {
                    warn!(%error, "Failed to persist scheduled occurrence");
                    failed_admission = true;
                }
            }
        }
        if !failed_admission {
            let mut last_tick_at = self.last_tick_at.write().await;
            if *last_tick_at < now {
                *last_tick_at = now;
            }
        }

        for result in futures::future::join_all(
            admitted
                .iter()
                .map(|id| self.drain_pending(Some(id.as_str()))),
        )
        .await
        {
            if let Err(error) = result {
                warn!(%error, "Failed to deliver scheduled occurrence");
            }
        }
    }

    /// Drive one captured occurrence through the production admission path.
    #[cfg(test)]
    async fn fire_task(&self, occurrence: ScheduledOccurrence) -> echo_core::error::Result<()> {
        if let Some(occurrence_id) = self.enqueue_scheduled(&occurrence).await? {
            let _ = self.drain_pending(Some(occurrence_id.as_str())).await?;
        }
        Ok(())
    }

    async fn enqueue_scheduled(
        &self,
        occurrence: &ScheduledOccurrence,
    ) -> echo_core::error::Result<Option<String>> {
        self.store.check_authority()?;
        let occurrence_id = scheduled_occurrence_id(&occurrence.task, occurrence.scheduled_at)?;
        let payload = OccurrencePayload {
            protection: OccurrenceProtection::PhysicalSettlementV1,
            task: occurrence.task.clone(),
            scheduled_at: occurrence.scheduled_at,
            trigger: SchedulerTrigger::Scheduled,
            control_revision: occurrence.task.control_revision,
        };
        {
            let _delivery = self.occurrence_authority.delivery_lock.lock().await;
            let _control = self.control_lock.lock().await;
            self.store.check_authority()?;
            let tasks = self.tasks.read().await;
            let current = tasks.iter().any(|candidate| {
                candidate.same_definition(&occurrence.task)
                    && candidate.status == CronTaskStatus::Enabled
                    && candidate.control_revision == occurrence.task.control_revision
            });
            if !current {
                debug!(
                    task_id = %occurrence.task.id,
                    scheduled_at = %occurrence.scheduled_at,
                    "Skipping occurrence disabled or removed before durable admission"
                );
                return Ok(None);
            };
            let inserted = self
                .occurrence_authority
                .enqueue_if_absent(&occurrence_id, payload)?;
            if !inserted
                && !self
                    .occurrence_authority
                    .ledger
                    .with_projection(|projection| {
                        projection
                            .record(&occurrence_id)
                            .is_some_and(|record| !record.terminal)
                    })
            {
                return Ok(None);
            }
        }
        Ok(Some(occurrence_id))
    }

    async fn drain_pending(
        &self,
        target_occurrence_id: Option<&str>,
    ) -> echo_core::error::Result<Option<String>> {
        self.drain_pending_with_admitted(target_occurrence_id, None)
            .await
    }

    async fn drain_pending_with_admitted(
        &self,
        target_occurrence_id: Option<&str>,
        mut admitted: Option<AdmittedOccurrence>,
    ) -> echo_core::error::Result<Option<String>> {
        loop {
            // New-admission gates cannot short-circuit an original owner
            // which already durably started its effect. Its result/projection
            // retains the existing post-closure error policy.
            if admitted.is_none() {
                self.store.check_authority()?;
                if self.cancel.is_cancelled() {
                    return Err(echo_core::error::ReactError::Other(
                        "Scheduler is cancelled; pending occurrences remain recoverable".into(),
                    ));
                }
            }
            let admitted = if let Some(admitted) = admitted.take() {
                admitted
            } else {
                let _delivery = self.occurrence_authority.delivery_lock.lock().await;
                if let Some(target) = target_occurrence_id {
                    let target_already_owned =
                        self.occurrence_authority
                            .ledger
                            .with_projection(|projection| {
                                projection.record(target).is_some_and(|record| {
                                    record.terminal || record.effect_started_at.is_some()
                                })
                            });
                    if target_already_owned {
                        return Ok(None);
                    }
                }
                let Some((claim, recovered_after_owner_loss)) =
                    self.occurrence_authority.claim_next()?
                else {
                    return Ok(None);
                };
                let occurrence_id = claim.message_id.clone();
                let is_target = target_occurrence_id == Some(occurrence_id.as_str());
                let payload = claim.payload.clone();
                // Durable effect admission is the linearization point relative
                // to disable/remove. A replay whose prior effect already began
                // stays admitted even when the definition was later removed.
                let _control = self.control_lock.lock().await;
                self.store.check_authority()?;
                if self.cancel.is_cancelled() {
                    return Err(echo_core::error::ReactError::Other(
                        "Scheduler was cancelled before callback admission".into(),
                    ));
                }
                let current = self.tasks.read().await.iter().any(|candidate| {
                    candidate.same_definition(&payload.task)
                        && candidate.status == CronTaskStatus::Enabled
                        && candidate.control_revision == payload.control_revision
                });
                if !current && !recovered_after_owner_loss {
                    self.occurrence_authority.settle(
                        &claim,
                        DeliverySettlement::terminal(
                            None,
                            DeliveryOutcome::Dropped,
                            None,
                            Some(
                                "task definition was disabled or removed before callback admission"
                                    .into(),
                            ),
                            None,
                        ),
                    )?;
                    if is_target {
                        return Err(echo_core::error::ReactError::Other(format!(
                            "Scheduler occurrence '{occurrence_id}' was withdrawn before admission"
                        )));
                    }
                    continue;
                }
                self.start_admitted_callback(claim, recovered_after_owner_loss)?
            };
            let AdmittedOccurrence {
                claim,
                recovered_after_owner_loss,
                completion,
            } = admitted;
            let occurrence_id = claim.message_id.clone();
            let is_target = target_occurrence_id == Some(occurrence_id.as_str());
            let payload = claim.payload.clone();

            info!(
                task_id = %payload.task.id,
                task_name = %payload.task.name,
                %occurrence_id,
                attempt_id = %claim.attempt_id,
                attempt = claim.attempt,
                recovered_after_owner_loss,
                "Firing cron task"
            );
            let result = completion.await.unwrap_or_else(|_| {
                Err(echo_core::error::ReactError::ExecutionNotSettled {
                    run_id: claim.message_id.clone(),
                    execution_id: Some(claim.attempt_id.clone()),
                })
            });
            match result {
                Ok(result) => {
                    info!(task = %payload.task.name, %occurrence_id, "Cron task completed");
                    if is_target {
                        return Ok(Some(result));
                    }
                }
                Err(error) => {
                    if matches!(
                        &error,
                        echo_core::error::ReactError::ExecutionNotSettled { .. }
                    ) {
                        warn!(task = %payload.task.name, %occurrence_id, %error, "Cron callback physical settlement is unknown");
                    } else {
                        warn!(task = %payload.task.name, %occurrence_id, %error, "Cron task failed");
                    }
                    if is_target {
                        return Err(error);
                    }
                }
            }
        }
    }

    // ── Management API ────────────────────────────────────────────

    /// Add a new cron task and persist it.
    pub async fn add_task(&self, task: CronTask) -> echo_core::error::Result<()> {
        let store = self.store.clone();
        let owner = self.mutation_owner.clone();
        let control = Arc::clone(&self.control_lock);
        let tasks = Arc::clone(&self.tasks);
        run_owned_mutation("add scheduler task owner", async move {
            let _control = control.lock().await;
            let mut tasks = tasks.write().await;
            match store.add_owned(task, &owner).await {
                Ok(stored) => {
                    tasks.push(stored);
                    Ok(())
                }
                Err(error) => {
                    if store.is_authority_poisoned() {
                        tasks.clear();
                    }
                    Err(error)
                }
            }
        })
        .await
    }

    /// Remove a cron task by its unique ID.
    pub async fn remove_task(&self, id: &str) -> echo_core::error::Result<bool> {
        self.remove_task_exact(id).await
    }

    /// Remove exactly one cron task by its complete ID.
    pub async fn remove_task_exact(&self, id: &str) -> echo_core::error::Result<bool> {
        let id = id.to_string();
        let store = self.store.clone();
        let owner = self.mutation_owner.clone();
        let control = Arc::clone(&self.control_lock);
        let tasks = Arc::clone(&self.tasks);
        let last_fired = Arc::clone(&self.last_fired);
        let authority = Arc::clone(&self.occurrence_authority);
        run_owned_mutation("remove scheduler task owner", async move {
            let _control = control.lock().await;
            let mut tasks = tasks.write().await;
            match store.remove_with_snapshot(&id, Some(&owner)).await {
                Ok(Some(committed)) => {
                    *tasks = committed;
                    last_fired.write().await.remove(&id);
                    authority
                        .latest_scheduled_at
                        .lock()
                        .map_err(|error| ledger_error("remove occurrence watermark", error))?
                        .retain(|(task_id, _), _| task_id != &id);
                    Ok(true)
                }
                Ok(None) => Ok(false),
                Err(error) => {
                    if store.is_authority_poisoned() {
                        tasks.clear();
                    }
                    Err(error)
                }
            }
        })
        .await
    }

    /// Enable or disable a task.
    pub async fn set_status(
        &self,
        id: &str,
        status: CronTaskStatus,
    ) -> echo_core::error::Result<bool> {
        let id = id.to_string();
        let store = self.store.clone();
        let owner = self.mutation_owner.clone();
        let control = Arc::clone(&self.control_lock);
        let tasks = Arc::clone(&self.tasks);
        run_owned_mutation("status scheduler task owner", async move {
            let _control = control.lock().await;
            let mut tasks = tasks.write().await;
            match store
                .set_status_with_snapshot(&id, status, Some(&owner))
                .await
            {
                Ok(Some(committed)) => {
                    *tasks = committed;
                    Ok(true)
                }
                Ok(None) => Ok(false),
                Err(error) => {
                    if store.is_authority_poisoned() {
                        tasks.clear();
                    }
                    Err(error)
                }
            }
        })
        .await
    }

    /// List all cron tasks.
    pub async fn list_tasks(&self) -> Vec<CronTask> {
        let _control = self.control_lock.lock().await;
        self.tasks.read().await.clone()
    }

    /// Manually fire a task immediately (bypassing schedule).
    pub async fn run_once(&self, id: &str) -> echo_core::error::Result<String> {
        let requested_at = Utc::now();
        loop {
            if self.cancel.is_cancelled() {
                return Err(echo_core::error::ReactError::Other(
                    "Scheduler is cancelled before manual admission".into(),
                ));
            }
            if let Some((occurrence_id, admitted)) = self.admit_manual(id, requested_at).await? {
                return self
                    .drain_pending_with_admitted(Some(&occurrence_id), Some(admitted))
                    .await?
                    .ok_or_else(|| ledger_error("manual run", "admitted callback did not settle"));
            }
            // Older unstarted work owns the FIFO frontier. Drive it before
            // persisting this manual occurrence so caller cancellation cannot
            // leave a surprise unadmitted manual run behind.
            self.drain_pending(None).await?;
            tokio::select! {
                _ = tokio::time::sleep(std::time::Duration::from_millis(100)) => {}
                _ = self.cancel.cancelled() => {
                    return Err(echo_core::error::ReactError::Other(
                        "Manual scheduler run was cancelled before admission".into()
                    ));
                }
            }
        }
    }

    async fn admit_manual(
        &self,
        id: &str,
        requested_at: DateTime<Utc>,
    ) -> echo_core::error::Result<Option<(String, AdmittedOccurrence)>> {
        self.store.check_authority()?;
        let _delivery = self.occurrence_authority.delivery_lock.lock().await;
        let _control = self.control_lock.lock().await;
        self.store.check_authority()?;
        let tasks = self.tasks.read().await;
        let task = tasks
            .iter()
            .find(|task| task.id == id && task.status == CronTaskStatus::Enabled)
            .cloned()
            .ok_or_else(|| {
                echo_core::error::ReactError::Other(format!("Enabled cron task '{id}' not found"))
            })?;
        if self.cancel.is_cancelled() {
            return Err(echo_core::error::ReactError::Other(
                "Scheduler is cancelled before manual admission".into(),
            ));
        }
        self.occurrence_authority.reconcile_owner_loss()?;
        let can_admit = self
            .occurrence_authority
            .ledger
            .with_projection(|projection| {
                projection
                    .frontier()
                    .all(|record| record.effect_started_at.is_some())
            });
        if !can_admit {
            return Ok(None);
        }
        let payload = OccurrencePayload {
            protection: OccurrenceProtection::PhysicalSettlementV1,
            control_revision: task.control_revision,
            task,
            scheduled_at: requested_at,
            trigger: SchedulerTrigger::Manual,
        };
        let occurrence_id = self
            .occurrence_authority
            .enqueue_generated_manual(payload.clone())?;
        let (claim, recovered_after_owner_loss) = self
            .occurrence_authority
            .claim_next()?
            .ok_or_else(|| ledger_error("manual run", "admitted occurrence has no claim"))?;
        if claim.message_id != occurrence_id {
            return Err(ledger_error(
                "manual run",
                "a previous unstarted occurrence crossed manual admission",
            ));
        }
        let admitted = self.start_admitted_callback(claim, recovered_after_owner_loss)?;
        Ok(Some((occurrence_id, admitted)))
    }

    /// Reload tasks from the store.
    pub async fn reload(&self) -> echo_core::error::Result<usize> {
        let _control = self.control_lock.lock().await;
        self.refresh_cache().await
    }

    async fn refresh_cache(&self) -> echo_core::error::Result<usize> {
        let tasks = self.store.recover_authority().await?;
        let count = tasks.len();
        let ids = tasks
            .iter()
            .map(|task| task.id.clone())
            .collect::<HashSet<_>>();
        let latest = load_latest_scheduled_at(&self.occurrence_authority.journal, &tasks)?;
        let mut current = self.tasks.write().await;
        self.last_fired
            .write()
            .await
            .retain(|id, _| ids.contains(id.as_str()));
        *current = tasks;
        *self
            .occurrence_authority
            .latest_scheduled_at
            .lock()
            .map_err(|error| ledger_error("reload occurrence watermark", error))? = latest;
        Ok(count)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use tokio::sync::Notify;

    async fn current_task(
        runner: &SchedulerRunner,
        id: &str,
    ) -> echo_core::error::Result<CronTask> {
        runner
            .list_tasks()
            .await
            .into_iter()
            .find(|task| task.id == id)
            .ok_or_else(|| {
                echo_core::error::ReactError::Other(format!("current cron task '{id}' is missing"))
            })
    }

    #[tokio::test]
    async fn interrupted_recovery_preserves_admission_after_definition_withdrawal()
    -> echo_core::error::Result<()> {
        for remove in [false, true] {
            let temp = tempfile::tempdir()?;
            let store = CronTaskStore::new().with_path(temp.path().join("tasks.json"));
            let calls = Arc::new(AtomicUsize::new(0));
            let expected_id = "original-occurrence";
            let calls_for_fn = Arc::clone(&calls);
            let fire_fn: OccurrenceFireFn = Arc::new(move |invocation| {
                assert_eq!(invocation.occurrence_id, expected_id);
                assert!(invocation.recovered_after_owner_loss);
                assert_eq!(invocation.attempt, 3);
                calls_for_fn.fetch_add(1, Ordering::SeqCst);
                Box::pin(async { Ok("replayed".into()) })
            });
            let runner = SchedulerRunner::new_with_occurrence_context(
                store.clone(),
                CancellationToken::new(),
                Arc::clone(&fire_fn),
            )
            .await?;
            let task = CronTask::new("恢复", "* * * * *", "run");
            runner.add_task(task.clone()).await?;
            let task = current_task(&runner, &task.id).await?;
            runner.occurrence_authority.enqueue_if_absent(
                expected_id,
                OccurrencePayload {
                    protection: OccurrenceProtection::LegacyAtLeastOnce,
                    control_revision: task.control_revision,
                    task: task.clone(),
                    scheduled_at: Utc::now(),
                    trigger: SchedulerTrigger::Manual,
                },
            )?;
            let first = runner
                .occurrence_authority
                .ledger
                .claim_next()
                .map_err(|error| ledger_error("legacy first claim", error))?
                .ok_or_else(|| ledger_error("legacy first claim", "missing"))?;
            runner
                .occurrence_authority
                .ledger
                .begin_effect(&first, first.attempt_id.clone())
                .map_err(|error| ledger_error("legacy first effect", error))?;
            runner.occurrence_authority.reconcile_owner_loss()?;
            let recovery = runner
                .occurrence_authority
                .ledger
                .claim_next()
                .map_err(|error| ledger_error("legacy second claim", error))?
                .ok_or_else(|| ledger_error("legacy second claim", "missing"))?;
            // Both prior attempts are original flat/None-field journal facts;
            // the current callback starts only after reopening this history.
            runner
                .occurrence_authority
                .ledger
                .begin_effect(&recovery, recovery.attempt_id.clone())
                .map_err(|error| ledger_error("legacy second effect", error))?;
            assert_eq!(recovery.attempt, 2);
            if remove {
                runner.remove_task_exact(&task.id).await?;
            } else {
                runner
                    .set_status(&task.id, CronTaskStatus::Disabled)
                    .await?;
            }
            drop(runner);
            let reopened = SchedulerRunner::new_with_occurrence_context(
                store,
                CancellationToken::new(),
                fire_fn,
            )
            .await?;
            reopened.drain_pending(None).await?;
            assert_eq!(calls.load(Ordering::SeqCst), 1);
            let outcome = reopened
                .occurrence_authority
                .ledger
                .with_projection(|projection| {
                    projection
                        .record(expected_id)
                        .and_then(|record| record.outcome)
                });
            assert_eq!(outcome, Some(DeliveryOutcome::Completed));
        }
        Ok(())
    }

    struct ReloadFailureStore {
        inner: echo_state::memory::InMemoryStore,
        fault: AtomicUsize,
        committed: tokio::sync::Notify,
        release: tokio::sync::Notify,
    }

    impl echo_core::memory::Store for ReloadFailureStore {
        fn put<'a>(
            &'a self,
            ns: &'a [&'a str],
            key: &'a str,
            value: serde_json::Value,
        ) -> BoxFuture<'a, echo_core::error::Result<()>> {
            Box::pin(async move {
                if self.fault.load(Ordering::SeqCst) == 5 {
                    return Err(echo_core::error::ReactError::Memory(Box::new(
                        echo_core::error::MemoryError::TransientNoCommit(
                            "injected pre-commit failure".to_string(),
                        ),
                    )));
                }
                self.inner.put(ns, key, value).await?;
                if self.fault.load(Ordering::SeqCst) == 7 {
                    return Err(ledger_error(
                        "write",
                        "injected ambiguous error after backend commit",
                    ));
                }
                if self.fault.load(Ordering::SeqCst) == 6 {
                    self.committed.notify_one();
                    self.release.notified().await;
                    return Ok(());
                }
                if self.fault.load(Ordering::SeqCst) > 0 {
                    self.committed.notify_one();
                    self.fault.fetch_add(2, Ordering::SeqCst);
                }
                Ok(())
            })
        }
        fn get<'a>(
            &'a self,
            ns: &'a [&'a str],
            key: &'a str,
        ) -> BoxFuture<'a, echo_core::error::Result<Option<echo_core::memory::StoreItem>>> {
            Box::pin(async move {
                match self.fault.load(Ordering::SeqCst) {
                    3 => return Err(ledger_error("reload", "injected post-commit failure")),
                    4 => return std::future::pending().await,
                    _ => {}
                }
                self.inner.get(ns, key).await
            })
        }
        fn search<'a>(
            &'a self,
            ns: &'a [&'a str],
            query: &'a str,
            limit: usize,
        ) -> BoxFuture<'a, echo_core::error::Result<Vec<echo_core::memory::StoreItem>>> {
            self.inner.search(ns, query, limit)
        }
        fn delete<'a>(
            &'a self,
            ns: &'a [&'a str],
            key: &'a str,
        ) -> BoxFuture<'a, echo_core::error::Result<bool>> {
            self.inner.delete(ns, key)
        }
        fn list_namespaces<'a>(
            &'a self,
            prefix: Option<&'a [&'a str]>,
        ) -> BoxFuture<'a, echo_core::error::Result<Vec<Vec<String>>>> {
            self.inner.list_namespaces(prefix)
        }
        fn list<'a>(
            &'a self,
            ns: &'a [&'a str],
        ) -> BoxFuture<'a, echo_core::error::Result<Vec<echo_core::memory::StoreItem>>> {
            self.inner.list(ns)
        }
    }

    #[tokio::test]
    async fn committed_control_updates_cache_without_a_post_commit_read()
    -> echo_core::error::Result<()> {
        for operation in 0..3 {
            let temp = tempfile::tempdir()?;
            let backend = Arc::new(ReloadFailureStore {
                inner: echo_state::memory::InMemoryStore::new(),
                fault: AtomicUsize::new(0),
                committed: tokio::sync::Notify::new(),
                release: tokio::sync::Notify::new(),
            });
            let store = CronTaskStore::with_backend_path_for_test(
                backend.clone(),
                temp.path().join("tasks.json"),
            );
            let calls = Arc::new(AtomicUsize::new(0));
            let calls_for_fn = Arc::clone(&calls);
            let fire_fn: FireFn = Arc::new(move |_| {
                calls_for_fn.fetch_add(1, Ordering::SeqCst);
                Box::pin(async { Ok("done".into()) })
            });
            let runner =
                Arc::new(SchedulerRunner::new(store, CancellationToken::new(), fire_fn).await?);
            let task = CronTask::new("fenced", "* * * * *", "run");
            runner.add_task(task.clone()).await?;
            let task = current_task(&runner, &task.id).await?;
            runner.occurrence_authority.enqueue_if_absent(
                "queued",
                OccurrencePayload {
                    protection: OccurrenceProtection::LegacyAtLeastOnce,
                    control_revision: task.control_revision,
                    task: task.clone(),
                    scheduled_at: Utc::now(),
                    trigger: SchedulerTrigger::Manual,
                },
            )?;
            backend.fault.store(1, Ordering::SeqCst);
            let management = tokio::spawn({
                let runner = Arc::clone(&runner);
                let id = task.id.clone();
                async move {
                    match operation {
                        0 => runner.set_status(&id, CronTaskStatus::Disabled).await,
                        1 => runner.remove_task(&id).await,
                        _ => runner.remove_task_exact(&id).await,
                    }
                }
            });
            backend.committed.notified().await;
            assert!(
                management
                    .await
                    .map_err(|error| ledger_error("join", error))??
            );
            runner.drain_pending(None).await?;
            assert!(runner.run_once(&task.id).await.is_err());
            runner
                .fire_task(ScheduledOccurrence {
                    task: task.clone(),
                    scheduled_at: Utc::now(),
                })
                .await?;
            assert_eq!(calls.load(Ordering::SeqCst), 0);
            assert!(runner.reload().await.is_err());
            backend.fault.store(0, Ordering::SeqCst);
            runner.reload().await?;
            assert!(runner.run_once(&task.id).await.is_err());
            if operation == 0 {
                runner.set_status(&task.id, CronTaskStatus::Enabled).await?;
            } else {
                runner.add_task(task.clone()).await?;
            }
            runner.run_once(&task.id).await?;
            assert_eq!(calls.load(Ordering::SeqCst), 1);
        }
        Ok(())
    }

    #[tokio::test]
    async fn rejected_mutation_preserves_store_and_runner_cache() -> echo_core::error::Result<()> {
        for operation in 0..4 {
            let temp = tempfile::tempdir()?;
            let backend = Arc::new(ReloadFailureStore {
                inner: echo_state::memory::InMemoryStore::new(),
                fault: AtomicUsize::new(0),
                committed: Notify::new(),
                release: Notify::new(),
            });
            let store = CronTaskStore::with_backend_path_for_test(
                backend.clone(),
                temp.path().join("tasks.json"),
            );
            let fire_fn: FireFn = Arc::new(|_| Box::pin(async { Ok("done".into()) }));
            let runner = SchedulerRunner::new(store, CancellationToken::new(), fire_fn).await?;
            let task = CronTask::new("kept", "* * * * *", "run");
            runner.add_task(task.clone()).await?;
            let extra = CronTask::new("not added", "* * * * *", "run");

            backend.fault.store(5, Ordering::SeqCst);
            let failed = match operation {
                0 => runner
                    .set_status(&task.id, CronTaskStatus::Disabled)
                    .await
                    .map(|_| ()),
                1 => runner.remove_task(&task.id).await.map(|_| ()),
                2 => runner.remove_task_exact(&task.id).await.map(|_| ()),
                _ => runner.add_task(extra.clone()).await,
            };
            assert!(failed.is_err());
            backend.fault.store(0, Ordering::SeqCst);
            let cached = runner.list_tasks().await;
            let committed = runner.store.load_all().await?;
            assert_eq!(cached, committed);
            assert_eq!(cached.len(), 1);
            assert_eq!(
                cached.first().map(|task| task.status),
                Some(CronTaskStatus::Enabled)
            );
            assert_eq!(runner.run_once(&task.id).await?, "done");
        }
        Ok(())
    }

    #[tokio::test]
    async fn visible_file_commit_after_sync_error_updates_runner_cache()
    -> echo_core::error::Result<()> {
        let temp = tempfile::tempdir()?;
        let store = CronTaskStore::new().with_path(temp.path().join("tasks.json"));
        let fire_fn: FireFn = Arc::new(|_| Box::pin(async { Ok("done".into()) }));
        let runner = SchedulerRunner::new(store.clone(), CancellationToken::new(), fire_fn).await?;
        let task = CronTask::new("reconciled", "* * * * *", "run");
        runner.add_task(task.clone()).await?;

        store.inject_file_after_replace_fault();
        assert!(
            runner
                .set_status(&task.id, CronTaskStatus::Disabled)
                .await?
        );

        let cached = runner.list_tasks().await;
        let committed = store.load_all().await?;
        assert_eq!(cached, committed);
        assert_eq!(cached.len(), 1);
        assert_eq!(
            cached.first().map(|current| current.status),
            Some(CronTaskStatus::Disabled)
        );
        assert!(runner.run_once(&task.id).await.is_err());
        Ok(())
    }

    #[tokio::test]
    async fn ambiguous_file_commit_poison_blocks_callbacks_until_reload()
    -> echo_core::error::Result<()> {
        let temp = tempfile::tempdir()?;
        let store = CronTaskStore::new().with_path(temp.path().join("tasks.json"));
        let calls = Arc::new(AtomicUsize::new(0));
        let calls_for_fire = Arc::clone(&calls);
        let fire_fn: FireFn = Arc::new(move |_| {
            calls_for_fire.fetch_add(1, Ordering::SeqCst);
            Box::pin(async { Ok("unexpected".into()) })
        });
        let runner = SchedulerRunner::new(store.clone(), CancellationToken::new(), fire_fn).await?;
        let task = CronTask::new("ambiguous", "* * * * *", "run");
        runner.add_task(task.clone()).await?;

        store.inject_file_after_replace_reconcile_read_fault();
        assert!(
            runner
                .set_status(&task.id, CronTaskStatus::Disabled)
                .await
                .is_err()
        );
        assert!(store.is_authority_poisoned());
        assert!(runner.list_tasks().await.is_empty());
        assert!(runner.run_once(&task.id).await.is_err());
        runner
            .tick_at(Utc::now() + chrono::Duration::minutes(2))
            .await;
        assert_eq!(calls.load(Ordering::SeqCst), 0);

        assert_eq!(runner.reload().await?, 1);
        assert!(!store.is_authority_poisoned());
        let recovered = runner.list_tasks().await;
        assert_eq!(recovered.len(), 1);
        assert_eq!(
            recovered.first().map(|current| current.status),
            Some(CronTaskStatus::Disabled)
        );
        assert!(runner.run_once(&task.id).await.is_err());
        Ok(())
    }

    #[tokio::test]
    async fn ambiguous_store_commit_poison_blocks_callbacks_until_reload()
    -> echo_core::error::Result<()> {
        let temp = tempfile::tempdir()?;
        let backend = Arc::new(ReloadFailureStore {
            inner: echo_state::memory::InMemoryStore::new(),
            fault: AtomicUsize::new(0),
            committed: Notify::new(),
            release: Notify::new(),
        });
        let store = CronTaskStore::with_backend_path_for_test(
            backend.clone(),
            temp.path().join("tasks.json"),
        );
        let calls = Arc::new(AtomicUsize::new(0));
        let calls_for_fire = Arc::clone(&calls);
        let fire_fn: FireFn = Arc::new(move |_| {
            calls_for_fire.fetch_add(1, Ordering::SeqCst);
            Box::pin(async { Ok("unexpected".into()) })
        });
        let runner = SchedulerRunner::new(store.clone(), CancellationToken::new(), fire_fn).await?;
        let task = CronTask::new("ambiguous backend", "* * * * *", "run");
        runner.add_task(task.clone()).await?;

        backend.fault.store(7, Ordering::SeqCst);
        assert!(
            runner
                .set_status(&task.id, CronTaskStatus::Disabled)
                .await
                .is_err()
        );
        assert!(store.is_authority_poisoned());
        assert!(runner.list_tasks().await.is_empty());
        assert!(runner.run_once(&task.id).await.is_err());
        runner
            .tick_at(Utc::now() + chrono::Duration::minutes(2))
            .await;
        assert_eq!(calls.load(Ordering::SeqCst), 0);

        backend.fault.store(0, Ordering::SeqCst);
        assert_eq!(runner.reload().await?, 1);
        assert!(!store.is_authority_poisoned());
        let recovered = runner.list_tasks().await;
        assert_eq!(
            recovered.first().map(|current| current.status),
            Some(CronTaskStatus::Disabled)
        );
        assert!(runner.run_once(&task.id).await.is_err());
        Ok(())
    }

    #[tokio::test]
    async fn cancelled_control_caller_does_not_abandon_committed_cache_projection()
    -> echo_core::error::Result<()> {
        let temp = tempfile::tempdir()?;
        let backend = Arc::new(ReloadFailureStore {
            inner: echo_state::memory::InMemoryStore::new(),
            fault: AtomicUsize::new(0),
            committed: Notify::new(),
            release: Notify::new(),
        });
        let store = CronTaskStore::with_backend_path_for_test(
            backend.clone(),
            temp.path().join("tasks.json"),
        );
        let fire_fn: FireFn = Arc::new(|_| Box::pin(async { Ok("unexpected".into()) }));
        let runner =
            Arc::new(SchedulerRunner::new(store, CancellationToken::new(), fire_fn).await?);
        let task = CronTask::new("cancelled control", "* * * * *", "run");
        runner.add_task(task.clone()).await?;
        backend.fault.store(6, Ordering::SeqCst);
        let committed = backend.committed.notified();
        let update = tokio::spawn({
            let runner = Arc::clone(&runner);
            let id = task.id.clone();
            async move { runner.set_status(&id, CronTaskStatus::Disabled).await }
        });
        tokio::time::timeout(std::time::Duration::from_secs(2), committed)
            .await
            .map_err(|_| ledger_error("control", "backend did not physically commit"))?;
        update.abort();
        assert!(update.await.is_err_and(|error| error.is_cancelled()));
        backend.release.notify_one();
        let cache = tokio::time::timeout(std::time::Duration::from_secs(2), runner.list_tasks())
            .await
            .map_err(|_| ledger_error("control", "detached cache projection did not settle"))?;
        assert_eq!(
            cache.first().map(|task| task.status),
            Some(CronTaskStatus::Disabled)
        );
        assert_eq!(cache, runner.store.load_all().await?);
        assert!(runner.run_once(&task.id).await.is_err());
        Ok(())
    }

    #[tokio::test]
    async fn cancelled_replay_does_not_construct_callback() -> echo_core::error::Result<()> {
        let temp = tempfile::tempdir()?;
        let store = CronTaskStore::new().with_path(temp.path().join("tasks.json"));
        let constructed = Arc::new(AtomicUsize::new(0));
        let constructed_for_fn = Arc::clone(&constructed);
        let fire_fn: FireFn = Arc::new(move |_task| {
            constructed_for_fn.fetch_add(1, Ordering::SeqCst);
            Box::pin(std::future::pending())
        });
        let cancel = CancellationToken::new();
        let runner = SchedulerRunner::new(store, cancel.clone(), fire_fn).await?;
        let task = CronTask::new("cancelled", "* * * * *", "run");
        runner.add_task(task.clone()).await?;
        let task = current_task(&runner, &task.id).await?;
        let scheduled_at = Utc::now();
        let occurrence_id = scheduled_occurrence_id(&task, scheduled_at)?;
        runner.occurrence_authority.enqueue_if_absent(
            &occurrence_id,
            OccurrencePayload {
                protection: OccurrenceProtection::LegacyAtLeastOnce,
                control_revision: task.control_revision,
                task,
                scheduled_at,
                trigger: SchedulerTrigger::Scheduled,
            },
        )?;
        cancel.cancel();
        assert!(runner.drain_pending(None).await.is_err());
        assert_eq!(constructed.load(Ordering::SeqCst), 0);
        let phase = runner
            .occurrence_authority
            .ledger
            .with_projection(|projection| {
                projection.record(&occurrence_id).map(|record| record.phase)
            });
        assert_eq!(phase, Some(DeliveryPhase::Persisted));
        Ok(())
    }

    #[test]
    fn scheduled_occurrence_identity_is_stable_and_definition_scoped()
    -> echo_core::error::Result<()> {
        let task = CronTask::new("identity", "* * * * *", "run");
        let scheduled_at = Utc
            .with_ymd_and_hms(2026, 8, 13, 0, 1, 0)
            .single()
            .ok_or_else(|| echo_core::error::ReactError::Other("invalid test time".into()))?;
        let first = scheduled_occurrence_id(&task, scheduled_at)?;
        assert_eq!(first, scheduled_occurrence_id(&task, scheduled_at)?);

        let mut replacement = task.clone();
        replacement.created_at = "2026-08-13T00:00:01Z".to_string();
        assert_ne!(first, scheduled_occurrence_id(&replacement, scheduled_at)?);
        let later = Utc
            .with_ymd_and_hms(2026, 8, 13, 0, 2, 0)
            .single()
            .ok_or_else(|| echo_core::error::ReactError::Other("invalid test time".into()))?;
        assert_ne!(first, scheduled_occurrence_id(&task, later)?);
        Ok(())
    }

    #[tokio::test]
    async fn queued_occurrence_is_fenced_by_durable_control_revision()
    -> echo_core::error::Result<()> {
        let temp = tempfile::tempdir().map_err(|error| {
            echo_core::error::ReactError::Other(format!("tempdir failed: {error}"))
        })?;
        let store = CronTaskStore::new().with_path(temp.path().join("tasks.json"));
        let fired = Arc::new(StdMutex::new(Vec::<String>::new()));
        let fired_for_fn = Arc::clone(&fired);
        let fire_fn: OccurrenceFireFn = Arc::new(move |invocation| {
            let fired = Arc::clone(&fired_for_fn);
            Box::pin(async move {
                fired
                    .lock()
                    .map_err(|error| {
                        echo_core::error::ReactError::Other(format!(
                            "callback recorder lock poisoned: {error}"
                        ))
                    })?
                    .push(invocation.task.id);
                Ok("done".to_string())
            })
        });
        let runner =
            SchedulerRunner::new_with_occurrence_context(store, CancellationToken::new(), fire_fn)
                .await?;
        let front = CronTask::new("front", "* * * * *", "front");
        let stale = CronTask::new("stale", "* * * * *", "stale");
        let stale_id = stale.id.clone();
        runner.add_task(front.clone()).await?;
        runner.add_task(stale.clone()).await?;
        let front = current_task(&runner, &front.id).await?;
        let stale = current_task(&runner, &stale_id).await?;
        let scheduled_at = Utc
            .with_ymd_and_hms(2026, 8, 13, 0, 1, 0)
            .single()
            .ok_or_else(|| echo_core::error::ReactError::Other("invalid test time".into()))?;
        let front_occurrence_id = scheduled_occurrence_id(&front, scheduled_at)?;
        let stale_occurrence_id = scheduled_occurrence_id(&stale, scheduled_at)?;
        for (occurrence_id, task) in [
            (front_occurrence_id.as_str(), front.clone()),
            (stale_occurrence_id.as_str(), stale.clone()),
        ] {
            runner.occurrence_authority.enqueue_if_absent(
                occurrence_id,
                OccurrencePayload {
                    protection: OccurrenceProtection::LegacyAtLeastOnce,
                    control_revision: task.control_revision,
                    task,
                    scheduled_at,
                    trigger: SchedulerTrigger::Scheduled,
                },
            )?;
        }
        runner
            .set_status(&stale_id, CronTaskStatus::Disabled)
            .await?;
        runner
            .set_status(&stale_id, CronTaskStatus::Enabled)
            .await?;

        let _ = runner.drain_pending(None).await?;
        let callbacks = fired.lock().map_err(|error| {
            echo_core::error::ReactError::Other(format!("callback recorder lock poisoned: {error}"))
        })?;
        assert_eq!(callbacks.len(), 1);
        assert_eq!(
            callbacks.first().map(String::as_str),
            Some(front.id.as_str())
        );
        drop(callbacks);
        let stale_record = runner
            .occurrence_authority
            .ledger
            .with_projection(|projection| projection.record(&stale_occurrence_id).cloned())
            .ok_or_else(|| {
                echo_core::error::ReactError::Other("stale occurrence record missing".into())
            })?;
        assert_eq!(stale_record.phase, DeliveryPhase::TurnSettled);
        assert_eq!(stale_record.outcome, Some(DeliveryOutcome::Dropped));
        Ok(())
    }

    #[tokio::test]
    async fn custom_store_occurrence_journals_are_isolated_by_full_path()
    -> echo_core::error::Result<()> {
        let temp = tempfile::tempdir().map_err(|error| {
            echo_core::error::ReactError::Other(format!("tempdir failed: {error}"))
        })?;
        let backend_a: Arc<dyn echo_core::memory::Store> =
            Arc::new(echo_state::memory::InMemoryStore::new());
        let backend_b: Arc<dyn echo_core::memory::Store> =
            Arc::new(echo_state::memory::InMemoryStore::new());
        let store_a = CronTaskStore::with_backend_path_for_test(
            backend_a,
            temp.path().join("backend-a/tasks.json"),
        );
        let store_b = CronTaskStore::with_backend_path_for_test(
            backend_b,
            temp.path().join("backend-b/tasks.json"),
        );
        let no_path = CronTaskStore::with_backend_path_for_test(
            Arc::new(echo_state::memory::InMemoryStore::new()),
            PathBuf::new(),
        );
        let unused: FireFn = Arc::new(|_task| Box::pin(async { Ok("unused".to_string()) }));
        assert!(
            SchedulerRunner::new(no_path, CancellationToken::new(), Arc::clone(&unused))
                .await
                .is_err()
        );

        let a_callbacks = Arc::new(AtomicUsize::new(0));
        let a_callbacks_for_fn = Arc::clone(&a_callbacks);
        let fire_a: FireFn = Arc::new(move |task| {
            let callbacks = Arc::clone(&a_callbacks_for_fn);
            Box::pin(async move {
                assert_eq!(task.name, "backend-a");
                callbacks.fetch_add(1, Ordering::SeqCst);
                Ok("a".to_string())
            })
        });
        let b_callbacks = Arc::new(AtomicUsize::new(0));
        let b_callbacks_for_fn = Arc::clone(&b_callbacks);
        let fire_b: FireFn = Arc::new(move |task| {
            let callbacks = Arc::clone(&b_callbacks_for_fn);
            Box::pin(async move {
                assert_eq!(task.name, "backend-b");
                callbacks.fetch_add(1, Ordering::SeqCst);
                Ok("b".to_string())
            })
        });
        let runner_a = SchedulerRunner::new(
            store_a.clone(),
            CancellationToken::new(),
            Arc::clone(&fire_a),
        )
        .await?;
        let runner_b =
            SchedulerRunner::new(store_b.clone(), CancellationToken::new(), fire_b).await?;
        let task_a = CronTask::new("backend-a", "* * * * *", "a");
        let task_b = CronTask::new("backend-b", "* * * * *", "b");
        let task_b_id = task_b.id.clone();
        runner_a.add_task(task_a.clone()).await?;
        runner_b.add_task(task_b).await?;
        let task_a = current_task(&runner_a, &task_a.id).await?;
        let scheduled_at = Utc
            .with_ymd_and_hms(2026, 8, 13, 0, 1, 0)
            .single()
            .ok_or_else(|| echo_core::error::ReactError::Other("invalid test time".into()))?;
        let a_occurrence_id = scheduled_occurrence_id(&task_a, scheduled_at)?;
        runner_a.occurrence_authority.enqueue_if_absent(
            &a_occurrence_id,
            OccurrencePayload {
                protection: OccurrenceProtection::LegacyAtLeastOnce,
                control_revision: task_a.control_revision,
                task: task_a,
                scheduled_at,
                trigger: SchedulerTrigger::Scheduled,
            },
        )?;
        let (a_claim, _) = runner_a
            .occurrence_authority
            .claim_next()?
            .ok_or_else(|| echo_core::error::ReactError::Other("backend A claim missing".into()))?;
        // This isolation/restart control encodes genuine old flat history,
        // before any current callback. New physical admissions are protected.
        let receipt = runner_a
            .occurrence_authority
            .ledger
            .begin_effect(&a_claim, a_claim.attempt_id.clone())
            .map_err(|error| ledger_error("legacy backend A history", error))?;
        runner_a
            .occurrence_authority
            .confirm_durable(&receipt, "legacy backend A history")?;

        let _ = runner_b.drain_pending(None).await?;
        assert_eq!(a_callbacks.load(Ordering::SeqCst), 0);
        assert_eq!(b_callbacks.load(Ordering::SeqCst), 0);
        assert_eq!(runner_b.run_once(&task_b_id).await?, "b");
        assert_eq!(b_callbacks.load(Ordering::SeqCst), 1);
        drop(runner_a);

        let reopened_a = SchedulerRunner::new(store_a, CancellationToken::new(), fire_a).await?;
        let _ = reopened_a.drain_pending(None).await?;
        assert_eq!(a_callbacks.load(Ordering::SeqCst), 1);
        assert_eq!(b_callbacks.load(Ordering::SeqCst), 1);
        drop(reopened_a);
        drop(runner_b);
        Ok(())
    }

    #[tokio::test]
    async fn tick_fires_occurrence_in_previous_window() -> echo_core::error::Result<()> {
        let root = std::env::temp_dir().join(format!("echo-scheduler-{}", uuid::Uuid::new_v4()));
        let store = CronTaskStore::new().with_path(root.join("tasks.json"));
        let fired = Arc::new(AtomicUsize::new(0));
        let fired_for_fn = Arc::clone(&fired);
        let fire_fn: FireFn = Arc::new(move |_task| {
            let fired = Arc::clone(&fired_for_fn);
            Box::pin(async move {
                fired.fetch_add(1, Ordering::SeqCst);
                Ok("done".to_string())
            })
        });
        let runner = SchedulerRunner::new(store, CancellationToken::new(), fire_fn).await?;
        runner
            .tasks
            .write()
            .await
            .push(CronTask::new("every minute", "* * * * *", "run"));
        let previous = Utc
            .with_ymd_and_hms(2026, 8, 13, 0, 0, 30)
            .single()
            .ok_or_else(|| echo_core::error::ReactError::Other("invalid test time".into()))?;
        *runner.last_tick_at.write().await = previous;
        let now = Utc
            .with_ymd_and_hms(2026, 8, 13, 0, 1, 5)
            .single()
            .ok_or_else(|| echo_core::error::ReactError::Other("invalid test time".into()))?;
        runner.tick_at(now).await;
        assert_eq!(fired.load(Ordering::SeqCst), 1);
        let _ = std::fs::remove_dir_all(root);
        Ok(())
    }

    #[tokio::test]
    async fn occurrence_journal_has_one_live_runner_authority() -> echo_core::error::Result<()> {
        let root = std::env::temp_dir().join(format!(
            "echo-scheduler-single-authority-{}",
            uuid::Uuid::new_v4()
        ));
        let store = CronTaskStore::new().with_path(root.join("tasks.json"));
        let fire_fn: FireFn = Arc::new(|_task| Box::pin(async { Ok("done".to_string()) }));
        let runner = SchedulerRunner::new(
            store.clone(),
            CancellationToken::new(),
            Arc::clone(&fire_fn),
        )
        .await?;
        let duplicate = SchedulerRunner::new(
            store.clone(),
            CancellationToken::new(),
            Arc::clone(&fire_fn),
        )
        .await;
        assert!(duplicate.is_err());
        drop(runner);

        let reopened = SchedulerRunner::new(store, CancellationToken::new(), fire_fn).await?;
        drop(reopened);
        let _ = std::fs::remove_dir_all(root);
        Ok(())
    }

    #[tokio::test]
    async fn shutdown_waits_for_scheduler_and_cancels_in_flight_fire()
    -> echo_core::error::Result<()> {
        let root = std::env::temp_dir().join(format!("echo-scheduler-{}", uuid::Uuid::new_v4()));
        let store = CronTaskStore::new().with_path(root.join("tasks.json"));
        let fire_started = Arc::new(tokio::sync::Notify::new());
        let fire_dropped = Arc::new(AtomicUsize::new(0));
        let release = Arc::new(tokio::sync::Notify::new());
        let release_for_fn = release.clone();
        let started_for_fn = Arc::clone(&fire_started);
        let dropped_for_fn = Arc::clone(&fire_dropped);
        let fire_fn: FireFn = Arc::new(move |_task| {
            let started = Arc::clone(&started_for_fn);
            let dropped = Arc::clone(&dropped_for_fn);
            let release = release_for_fn.clone();
            Box::pin(async move {
                struct DropMarker(Arc<AtomicUsize>);
                impl Drop for DropMarker {
                    fn drop(&mut self) {
                        self.0.fetch_add(1, Ordering::SeqCst);
                    }
                }

                let _marker = DropMarker(dropped);
                started.notify_one();
                release.notified().await;
                Ok("original callback physically closed".to_string())
            })
        });
        let runner =
            Arc::new(SchedulerRunner::new(store, CancellationToken::new(), fire_fn).await?);
        runner
            .tasks
            .write()
            .await
            .push(CronTask::new("every minute", "* * * * *", "run"));
        let previous = Utc
            .with_ymd_and_hms(2026, 8, 13, 0, 0, 30)
            .single()
            .ok_or_else(|| echo_core::error::ReactError::Other("invalid test time".into()))?;
        *runner.last_tick_at.write().await = previous;
        let now = Utc
            .with_ymd_and_hms(2026, 8, 13, 0, 1, 5)
            .single()
            .ok_or_else(|| echo_core::error::ReactError::Other("invalid test time".into()))?;

        let mut handle = Arc::clone(&runner).spawn();
        let tick = tokio::spawn(async move { runner.tick_at(now).await });
        fire_started.notified().await;
        let observation =
            tokio::time::timeout(std::time::Duration::from_millis(20), handle.shutdown()).await;
        let early_is_observation = matches!(
            observation,
            Err(_)
                | Ok(Err(
                    echo_core::error::ReactError::ExecutionNotSettled { .. }
                ))
        );
        let held_drops = fire_dropped.load(Ordering::SeqCst);
        let held_tick = !tick.is_finished();
        release.notify_one();
        tokio::time::timeout(std::time::Duration::from_secs(1), tick)
            .await
            .map_err(|_| {
                echo_core::error::ReactError::Other(
                    "in-flight scheduler task did not observe shutdown".into(),
                )
            })?
            .map_err(|error| {
                echo_core::error::ReactError::Other(format!("scheduler tick task failed: {error}"))
            })?;

        handle.shutdown().await?;
        assert!(early_is_observation && held_tick);
        assert_eq!(held_drops, 0);
        assert_eq!(fire_dropped.load(Ordering::SeqCst), 1);
        let _ = std::fs::remove_dir_all(root);
        Ok(())
    }

    #[tokio::test]
    async fn protected_scheduled_occurrence_is_not_replayed_after_observation()
    -> echo_core::error::Result<()> {
        assert_observed_callback_protection(false, false, true).await
    }

    #[tokio::test]
    async fn crash_after_callback_reuses_occurrence_id_with_a_new_attempt()
    -> echo_core::error::Result<()> {
        let root = std::env::temp_dir().join(format!(
            "echo-scheduler-at-least-once-{}",
            uuid::Uuid::new_v4()
        ));
        let store = CronTaskStore::new().with_path(root.join("tasks.json"));
        let invocations = Arc::new(StdMutex::new(Vec::<SchedulerInvocation>::new()));
        let scheduled_at = Utc
            .with_ymd_and_hms(2026, 8, 13, 0, 1, 0)
            .single()
            .ok_or_else(|| echo_core::error::ReactError::Other("invalid test time".into()))?;
        let task = CronTask::new("at-least-once", "* * * * *", "run");
        let task_id = task.id.clone();
        let (first_attempt_id, occurrence_id) = {
            let invocations_for_fn = Arc::clone(&invocations);
            let fire_fn: OccurrenceFireFn = Arc::new(move |invocation| {
                let invocations = Arc::clone(&invocations_for_fn);
                Box::pin(async move {
                    invocations
                        .lock()
                        .map_err(|error| {
                            echo_core::error::ReactError::Other(format!(
                                "invocation recorder lock poisoned: {error}"
                            ))
                        })?
                        .push(invocation);
                    Ok("effect completed".to_string())
                })
            });
            let first_runner = SchedulerRunner::new_with_occurrence_context(
                store.clone(),
                CancellationToken::new(),
                fire_fn,
            )
            .await?;
            first_runner.add_task(task.clone()).await?;
            let task = current_task(&first_runner, &task_id).await?;
            let occurrence_id = scheduled_occurrence_id(&task, scheduled_at)?;
            first_runner.occurrence_authority.enqueue_if_absent(
                &occurrence_id,
                OccurrencePayload {
                    protection: OccurrenceProtection::LegacyAtLeastOnce,
                    task: task.clone(),
                    scheduled_at,
                    trigger: SchedulerTrigger::Scheduled,
                    control_revision: task.control_revision,
                },
            )?;
            let (claim, recovered) =
                first_runner
                    .occurrence_authority
                    .claim_next()?
                    .ok_or_else(|| {
                        echo_core::error::ReactError::Other(
                            "first scheduler occurrence claim missing".into(),
                        )
                    })?;
            assert!(!recovered);
            // Simulate the actual old journal contract, not new admission.
            let receipt = first_runner
                .occurrence_authority
                .ledger
                .begin_effect(&claim, claim.attempt_id.clone())
                .map_err(|error| ledger_error("legacy callback admission", error))?;
            first_runner
                .occurrence_authority
                .confirm_durable(&receipt, "legacy callback admission")?;
            let callback = (first_runner.fire_fn)(SchedulerInvocation {
                occurrence_id: occurrence_id.clone(),
                attempt_id: claim.attempt_id.clone(),
                attempt: claim.attempt,
                task: claim.payload.task.clone(),
                scheduled_at: claim.payload.scheduled_at,
                trigger: claim.payload.trigger,
                recovered_after_owner_loss: false,
            });
            assert_eq!(callback.await?, "effect completed");
            let phase = first_runner
                .occurrence_authority
                .ledger
                .with_projection(|projection| {
                    projection.record(&occurrence_id).map(|record| record.phase)
                });
            assert_eq!(phase, Some(DeliveryPhase::EffectStarted));
            (claim.attempt_id, occurrence_id)
        };

        let invocations_for_fn = Arc::clone(&invocations);
        let replay_fire_fn: OccurrenceFireFn = Arc::new(move |invocation| {
            let invocations = Arc::clone(&invocations_for_fn);
            Box::pin(async move {
                invocations
                    .lock()
                    .map_err(|error| {
                        echo_core::error::ReactError::Other(format!(
                            "invocation recorder lock poisoned: {error}"
                        ))
                    })?
                    .push(invocation);
                Ok("effect replayed".to_string())
            })
        });
        let replay_runner = SchedulerRunner::new_with_occurrence_context(
            store,
            CancellationToken::new(),
            replay_fire_fn,
        )
        .await?;
        let recovered = replay_runner
            .occurrence_authority
            .ledger
            .with_projection(|projection| projection.record(&occurrence_id).cloned())
            .ok_or_else(|| {
                echo_core::error::ReactError::Other("recovered occurrence missing".into())
            })?;
        assert_eq!(recovered.phase, DeliveryPhase::Deferred);
        assert_eq!(recovered.outcome, Some(DeliveryOutcome::OutcomeUnknown));
        assert_eq!(
            recovered.attempt_id.as_deref(),
            Some(first_attempt_id.as_str())
        );

        let _ = replay_runner.drain_pending(None).await?;
        let recorded = invocations.lock().map_err(|error| {
            echo_core::error::ReactError::Other(format!(
                "invocation recorder lock poisoned: {error}"
            ))
        })?;
        assert_eq!(recorded.len(), 2);
        let first = recorded.first().ok_or_else(|| {
            echo_core::error::ReactError::Other("first invocation missing".into())
        })?;
        let second = recorded.get(1).ok_or_else(|| {
            echo_core::error::ReactError::Other("second invocation missing".into())
        })?;
        assert_eq!(first.occurrence_id, second.occurrence_id);
        assert_eq!(first.attempt, 1);
        assert_eq!(second.attempt, 2);
        assert_ne!(first.attempt_id, second.attempt_id);
        assert!(second.recovered_after_owner_loss);
        drop(recorded);

        let settled = replay_runner
            .occurrence_authority
            .ledger
            .with_projection(|projection| projection.record(&occurrence_id).cloned())
            .ok_or_else(|| {
                echo_core::error::ReactError::Other("settled occurrence missing".into())
            })?;
        assert_eq!(settled.phase, DeliveryPhase::TurnSettled);
        assert_eq!(settled.outcome, Some(DeliveryOutcome::Completed));
        assert!(settled.terminal);

        drop(replay_runner);
        let _ = std::fs::remove_dir_all(root);
        Ok(())
    }

    #[tokio::test]
    async fn borrowed_waiter_abort_preserves_original_callback_without_replay()
    -> echo_core::error::Result<()> {
        let root = std::env::temp_dir().join(format!(
            "echo-scheduler-aborted-owner-{}",
            uuid::Uuid::new_v4()
        ));
        let store = CronTaskStore::new().with_path(root.join("tasks.json"));
        let first_started = Arc::new(tokio::sync::Notify::new());
        let replayed = Arc::new(tokio::sync::Notify::new());
        let release = Arc::new(tokio::sync::Notify::new());
        let release_for_fn = release.clone();
        let started_for_fn = Arc::clone(&first_started);
        let replayed_for_fn = Arc::clone(&replayed);
        let fire_fn: OccurrenceFireFn = Arc::new(move |invocation| {
            let started = Arc::clone(&started_for_fn);
            let replayed = Arc::clone(&replayed_for_fn);
            let release = release_for_fn.clone();
            Box::pin(async move {
                if invocation.attempt == 1 {
                    started.notify_one();
                    release.notified().await;
                    return Ok("same original callback completed".to_string());
                }
                assert!(invocation.recovered_after_owner_loss);
                replayed.notify_one();
                Ok("replayed after abort".to_string())
            })
        });
        let runner = Arc::new(
            SchedulerRunner::new_with_occurrence_context(store, CancellationToken::new(), fire_fn)
                .await?,
        );
        let task = CronTask::new("aborted", "* * * * *", "run");
        let task_id = task.id.clone();
        runner.add_task(task).await?;

        let runner_for_manual = Arc::clone(&runner);
        let manual = tokio::spawn(async move { runner_for_manual.run_once(&task_id).await });
        first_started.notified().await;
        manual.abort();
        let aborted = manual.await;
        assert!(aborted.is_err_and(|error| error.is_cancelled()));

        let _ = runner.drain_pending(None).await?;
        let unexpected_replay = futures::poll!(Box::pin(replayed.notified()).as_mut()).is_ready();
        release.notify_one();
        tokio::time::timeout(std::time::Duration::from_millis(250), async {
            loop {
                if runner
                    .occurrence_authority
                    .ledger
                    .with_projection(|projection| {
                        projection
                            .records()
                            .next()
                            .is_some_and(|record| record.terminal)
                    })
                {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .map_err(|_| ledger_error("original callback", "physical callback did not settle"))?;
        let completed = runner
            .occurrence_authority
            .ledger
            .with_projection(|projection| projection.records().next().cloned())
            .ok_or_else(|| {
                echo_core::error::ReactError::Other("reconciled occurrence missing".into())
            })?;
        assert_eq!(completed.phase, DeliveryPhase::TurnSettled);
        assert_eq!(completed.outcome, Some(DeliveryOutcome::Completed));
        assert!(!unexpected_replay);
        assert_eq!(completed.attempt, 1);

        drop(runner);
        let _ = std::fs::remove_dir_all(root);
        Ok(())
    }

    #[tokio::test]
    async fn legacy_fire_fn_uses_the_canonical_occurrence_settlement()
    -> echo_core::error::Result<()> {
        let root = std::env::temp_dir().join(format!(
            "echo-scheduler-legacy-adapter-{}",
            uuid::Uuid::new_v4()
        ));
        let store = CronTaskStore::new().with_path(root.join("tasks.json"));
        let fire_fn: FireFn =
            Arc::new(|_task| Box::pin(async { Ok("legacy completed".to_string()) }));
        let runner = SchedulerRunner::new(store, CancellationToken::new(), fire_fn).await?;
        let task = CronTask::new("legacy", "* * * * *", "run");
        let task_id = task.id.clone();
        runner.add_task(task).await?;

        assert_eq!(runner.run_once(&task_id).await?, "legacy completed");
        let records = runner
            .occurrence_authority
            .ledger
            .with_projection(|projection| projection.records().cloned().collect::<Vec<_>>());
        assert_eq!(records.len(), 1);
        let record = records.first().ok_or_else(|| {
            echo_core::error::ReactError::Other("legacy occurrence missing".into())
        })?;
        assert_eq!(record.phase, DeliveryPhase::TurnSettled);
        assert_eq!(record.outcome, Some(DeliveryOutcome::Completed));
        assert_eq!(record.attempt, 1);

        drop(runner);
        let _ = std::fs::remove_dir_all(root);
        Ok(())
    }

    #[tokio::test]
    async fn observed_callback_keeps_original_occurrence_nonterminal_across_reopen()
    -> echo_core::error::Result<()> {
        assert_observed_callback_protection(false, false, false).await
    }

    #[tokio::test]
    async fn legacy_occurrence_new_callback_is_protected_before_its_effect()
    -> echo_core::error::Result<()> {
        assert_observed_callback_protection(true, false, false).await
    }

    #[tokio::test]
    async fn legacy_recovered_callback_is_protected_before_its_effect()
    -> echo_core::error::Result<()> {
        assert_observed_callback_protection(true, true, false).await
    }

    async fn assert_observed_callback_protection(
        legacy: bool,
        previously_started: bool,
        scheduled: bool,
    ) -> echo_core::error::Result<()> {
        let directory = tempfile::tempdir()?;
        let effect = directory.path().join("original-scheduler-effect.txt");
        let store = CronTaskStore::new().with_path(directory.path().join("tasks.json"));
        let release = Arc::new(tokio::sync::Notify::new());
        let entered = Arc::new(tokio::sync::Notify::new());
        let producer = Arc::new(StdMutex::new(None::<JoinHandle<std::io::Result<()>>>));
        let calls = Arc::new(AtomicUsize::new(0));
        let fire_fn: OccurrenceFireFn = {
            let release = release.clone();
            let entered = entered.clone();
            let producer = producer.clone();
            let calls = calls.clone();
            let effect = effect.clone();
            Arc::new(move |invocation| {
                let release = release.clone();
                let entered = entered.clone();
                let producer = producer.clone();
                let calls = calls.clone();
                let effect = effect.clone();
                Box::pin(async move {
                    calls.fetch_add(1, Ordering::SeqCst);
                    let source_entered = entered.clone();
                    let physical = tokio::spawn(async move {
                        tokio::fs::write(&effect, b"original-scheduler-producer-entered").await?;
                        source_entered.notify_one();
                        release.notified().await;
                        tokio::fs::write(&effect, b"original-scheduler-producer-closed").await
                    });
                    *producer.lock().unwrap_or_else(|error| error.into_inner()) = Some(physical);
                    entered.notified().await;
                    Err(echo_core::error::ReactError::ExecutionNotSettled {
                        run_id: invocation.occurrence_id,
                        execution_id: Some(invocation.attempt_id),
                    })
                })
            })
        };
        let mut runner = SchedulerRunner::new_with_occurrence_context(
            store.clone(),
            CancellationToken::new(),
            fire_fn.clone(),
        )
        .await?;
        let task = CronTask::new("physical observation", "* * * * *", "one actual effect");
        let task_id = task.id.clone();
        runner.add_task(task).await?;
        let result = if legacy {
            // Actual old flat Persisted history, not a new callback relabeled
            // legacy. This reader is now starting its first physical attempt.
            let task = current_task(&runner, &task_id).await?;
            runner.occurrence_authority.enqueue_if_absent(
                "legacy-observation",
                OccurrencePayload {
                    control_revision: task.control_revision,
                    task,
                    scheduled_at: Utc::now(),
                    trigger: SchedulerTrigger::Manual,
                    protection: OccurrenceProtection::LegacyAtLeastOnce,
                },
            )?;
            if previously_started {
                // Encode genuine old admitted history through the unchanged
                // generic helper (None field), not the new Scheduler admission.
                let old_claim = runner
                    .occurrence_authority
                    .ledger
                    .claim_next()
                    .map_err(|error| ledger_error("old history claim", error))?
                    .ok_or_else(|| ledger_error("old history claim", "missing"))?;
                let receipt = runner
                    .occurrence_authority
                    .ledger
                    .begin_effect(&old_claim, old_claim.attempt_id.clone())
                    .map_err(|error| ledger_error("old history admission", error))?;
                runner
                    .occurrence_authority
                    .confirm_durable(&receipt, "old history admission")?;
                // The production restart path reconciles old orphaned history
                // before target lookup. Reopen that same real journal here.
                drop(runner);
                runner = SchedulerRunner::new_with_occurrence_context(
                    store.clone(),
                    CancellationToken::new(),
                    fire_fn.clone(),
                )
                .await?;
            }
            runner
                .drain_pending(Some("legacy-observation"))
                .await
                .and_then(|result| {
                    result
                        .ok_or_else(|| ledger_error("legacy counter", "callback observation lost"))
                })
        } else if scheduled {
            let task = current_task(&runner, &task_id).await?;
            runner
                .fire_task(ScheduledOccurrence {
                    task,
                    scheduled_at: Utc::now(),
                })
                .await
                .map(|()| "scheduled source completed".to_string())
        } else {
            runner.run_once(&task_id).await
        };
        let before = runner
            .occurrence_authority
            .ledger
            .with_projection(|projection| projection.records().next().cloned())
            .ok_or_else(|| ledger_error("observation counter", "original occurrence missing"))?;
        let task_before = current_task(&runner, &task_id).await?;
        let held_effect = tokio::fs::read(&effect).await;
        drop(runner);
        let reopened =
            SchedulerRunner::new_with_occurrence_context(store, CancellationToken::new(), fire_fn)
                .await?;
        // Checking admission suffices; never launch a second backend merely
        // to prove the same still-held effect was reclaimable.
        let replay = reopened.occurrence_authority.claim_next();
        let after = reopened
            .occurrence_authority
            .ledger
            .with_projection(|projection| projection.record(&before.message_id).cloned())
            .ok_or_else(|| {
                ledger_error(
                    "observation counter",
                    "original occurrence lost after reopen",
                )
            })?;
        let calls_before_release = calls.load(Ordering::SeqCst);
        // Close and join the same real backend before assertions, including red.
        release.notify_one();
        let physical = producer
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .take()
            .ok_or_else(|| {
                ledger_error(
                    "observation counter",
                    format!("original backend join missing; invocation result={result:?}"),
                )
            })?;
        tokio::time::timeout(std::time::Duration::from_secs(2), physical)
            .await
            .map_err(|_| {
                ledger_error(
                    "observation counter",
                    "released original backend did not close",
                )
            })?
            .map_err(|error| ledger_error("observation counter", error))??;
        let closed_effect = tokio::fs::read(&effect).await?;
        eprintln!(
            "SCHEDULER_PHYSICAL_OBSERVATION {}",
            serde_json::json!({
                "legacy":legacy,"previously_started":previously_started,"protection_before":format!("{:?}",before.payload.protection),
            "result":format!("{result:?}"),"phase_before":format!("{:?}",before.phase),
                "terminal_before":before.terminal,"outcome_before":format!("{:?}",before.outcome),
                "attempt_before":before.attempt_id,"phase_after_reopen":format!("{:?}",after.phase),
                "attempt_after_reopen":after.attempt_id,"calls_before_release":calls_before_release,
                "last_result":task_before.last_result,"physical_joined":true
            })
        );
        assert_eq!(held_effect?, b"original-scheduler-producer-entered");
        assert_eq!(closed_effect, b"original-scheduler-producer-closed");
        assert!(
            matches!(result, Err(echo_core::error::ReactError::ExecutionNotSettled { ref run_id, ref execution_id })
            if run_id == &before.message_id && execution_id == &before.attempt_id)
        );
        assert_eq!(
            before.payload.protection,
            OccurrenceProtection::PhysicalSettlementV1,
            "new physical attempts require protection even for old persisted occurrences"
        );
        assert_eq!(
            before.phase,
            DeliveryPhase::EffectStarted,
            "observation is not a physical settlement"
        );
        assert!(!before.terminal);
        assert!(before.outcome.is_none());
        assert!(task_before.last_run_at.is_none() && task_before.last_result.is_none());
        assert!(
            matches!(replay, Ok(None)),
            "unknown original effect cannot admit a replacement"
        );
        assert_eq!(after.phase, DeliveryPhase::EffectStarted);
        assert_eq!(after.attempt_id, before.attempt_id);
        assert!(!after.terminal);
        assert_eq!(calls_before_release, 1);
        Ok(())
    }

    #[tokio::test]
    async fn started_scheduler_callback_survives_borrowed_waiter_drop()
    -> echo_core::error::Result<()> {
        assert_started_scheduler_callback_retained(false, false, false).await
    }

    #[tokio::test]
    async fn started_scheduler_callback_survives_shutdown_observation()
    -> echo_core::error::Result<()> {
        assert_started_scheduler_callback_retained(true, false, false).await
    }

    #[tokio::test]
    async fn cancellation_after_manual_admission_waits_for_same_physical_callback()
    -> echo_core::error::Result<()> {
        assert_started_scheduler_callback_retained(true, true, false).await
    }

    #[tokio::test]
    async fn poisoned_definition_after_admission_waits_for_same_physical_callback()
    -> echo_core::error::Result<()> {
        assert_started_scheduler_callback_retained(false, false, true).await
    }

    async fn assert_started_scheduler_callback_retained(
        shutdown: bool,
        cancel_after_admission: bool,
        poison_after_admission: bool,
    ) -> echo_core::error::Result<()> {
        use futures::FutureExt;
        type PhysicalJoin = futures::future::Shared<BoxFuture<'static, Result<(), String>>>;
        let directory = tempfile::tempdir()?;
        let effect = directory.path().join("started-effect.txt");
        let backend = poison_after_admission.then(|| {
            Arc::new(ReloadFailureStore {
                inner: echo_state::memory::InMemoryStore::new(),
                fault: AtomicUsize::new(0),
                committed: tokio::sync::Notify::new(),
                release: tokio::sync::Notify::new(),
            })
        });
        let store = if let Some(backend) = backend.as_ref() {
            CronTaskStore::with_backend_path_for_test(
                backend.clone(),
                directory.path().join("tasks.json"),
            )
        } else {
            CronTaskStore::new().with_path(directory.path().join("tasks.json"))
        };
        let poisoned = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let split_admission = cancel_after_admission || poison_after_admission;
        let observation_ready = Arc::new(tokio::sync::Notify::new());
        let release = Arc::new(tokio::sync::Notify::new());
        let entered = Arc::new(tokio::sync::Notify::new());
        let producer = Arc::new(StdMutex::new(None::<PhysicalJoin>));
        let drops = Arc::new(AtomicUsize::new(0));
        let fire_fn: OccurrenceFireFn = {
            let release = release.clone();
            let entered = entered.clone();
            let producer = producer.clone();
            let effect = effect.clone();
            let drops = drops.clone();
            Arc::new(move |_| {
                let release = release.clone();
                let entered = entered.clone();
                let producer = producer.clone();
                let effect = effect.clone();
                let drops = drops.clone();
                Box::pin(async move {
                    struct CallbackLifetime(Arc<AtomicUsize>);
                    impl Drop for CallbackLifetime {
                        fn drop(&mut self) {
                            self.0.fetch_add(1, Ordering::SeqCst);
                        }
                    }
                    let _lifetime = CallbackLifetime(drops);
                    let backend = tokio::spawn(async move {
                        tokio::fs::write(&effect, b"started-effect-held").await?;
                        entered.notify_one();
                        release.notified().await;
                        tokio::fs::write(&effect, b"started-effect-closed").await
                    });
                    let physical = async move {
                        backend
                            .await
                            .map_err(|error| error.to_string())?
                            .map_err(|error| error.to_string())
                    }
                    .boxed()
                    .shared();
                    *producer.lock().unwrap_or_else(|error| error.into_inner()) =
                        Some(physical.clone());
                    physical
                        .await
                        .map_err(|error| ledger_error("original physical join", error))?;
                    Ok("same physical callback completed".to_string())
                })
            })
        };
        let cancel = CancellationToken::new();
        let runner = Arc::new(
            SchedulerRunner::new_with_occurrence_context(store, cancel.clone(), fire_fn).await?,
        );
        let task = CronTask::new("started owner", "* * * * *", "one original effect");
        let id = task.id.clone();
        runner.add_task(task).await?;
        let admitted = if split_admission {
            runner.admit_manual(&id, Utc::now()).await?
        } else {
            None
        };
        let started_before = if split_admission {
            Some(tokio::time::timeout(std::time::Duration::from_secs(2), entered.notified()).await)
        } else {
            None
        };
        let owned = runner.clone();
        let owned_cancel = cancel.clone();
        let owned_poisoned = poisoned.clone();
        let owned_observation_ready = observation_ready.clone();
        let mut manual = tokio::spawn(async move {
            if let Some((occurrence_id, admitted)) = admitted {
                if cancel_after_admission {
                    owned_cancel.cancel();
                }
                if poison_after_admission {
                    let backend =
                        backend.ok_or_else(|| ledger_error("poison control", "backend missing"))?;
                    backend.fault.store(7, Ordering::SeqCst);
                    let mutation = owned.set_status(&id, CronTaskStatus::Disabled).await;
                    if mutation.is_err() && owned.store.is_authority_poisoned() {
                        owned_poisoned.store(true, Ordering::SeqCst);
                    } else {
                        return Err(ledger_error(
                            "poison control",
                            "real ambiguous mutation did not poison authority",
                        ));
                    }
                }
                owned_observation_ready.notify_one();
                owned
                    .drain_pending_with_admitted(Some(&occurrence_id), Some(admitted))
                    .await
                    .and_then(|result| {
                        result.ok_or_else(|| ledger_error("admitted observer", "result missing"))
                    })
            } else {
                owned.run_once(&id).await
            }
        });
        let started = if let Some(started) = started_before {
            started
        } else {
            tokio::time::timeout(std::time::Duration::from_secs(2), entered.notified()).await
        };
        let original = runner
            .occurrence_authority
            .ledger
            .with_projection(|projection| projection.records().next().cloned())
            .ok_or_else(|| ledger_error("owner counter", "original occurrence missing"))?;
        if shutdown {
            cancel.cancel();
        }
        let seam_ready = if split_admission {
            tokio::time::timeout(
                std::time::Duration::from_secs(2),
                observation_ready.notified(),
            )
            .await
            .is_ok()
        } else {
            true
        };
        // Existing cases drop the borrowed waiter. The admitted-cancel case
        // checks that this same observer does not report an ordinary failure.
        let early_result = if split_admission {
            tokio::time::timeout(std::time::Duration::from_millis(20), &mut manual)
                .await
                .ok()
        } else {
            manual.abort();
            let _ = (&mut manual).await;
            None
        };
        let observer_pending = !split_admission || early_result.is_none();
        let held_effect = tokio::fs::read(&effect).await;
        let drops_while_held = drops.load(Ordering::SeqCst);
        let active_while_held = runner
            .occurrence_authority
            .active_attempts
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .contains(original.attempt_id.as_deref().unwrap_or_default());
        let reclaimed = runner.occurrence_authority.claim_next()?;
        release.notify_one();
        let physical = producer
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .clone()
            .ok_or_else(|| ledger_error("owner counter", "same original join missing"))?;
        tokio::time::timeout(std::time::Duration::from_secs(2), physical)
            .await
            .map_err(|_| ledger_error("owner counter", "released backend failed to join"))?
            .map_err(|error| ledger_error("owner counter", error))?;
        let admitted_result = if split_admission {
            let joined = if let Some(early) = early_result {
                early
            } else {
                tokio::time::timeout(std::time::Duration::from_secs(2), &mut manual)
                    .await
                    .map_err(|_| {
                        ledger_error("admitted observer", "original result did not arrive")
                    })?
            };
            Some(joined.map_err(|error| ledger_error("admitted observer", error))?)
        } else {
            None
        };
        let after_release = tokio::time::timeout(std::time::Duration::from_secs(2), async {
            loop {
                let settled = runner
                    .occurrence_authority
                    .ledger
                    .with_projection(|projection| {
                        projection
                            .record(&original.message_id)
                            .is_some_and(|record| record.terminal)
                    });
                if drops.load(Ordering::SeqCst) == 1 && (reclaimed.is_some() || settled) {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await;
        let closed_effect = tokio::fs::read(&effect).await?;
        eprintln!(
            "SCHEDULER_STARTED_OWNER {}",
            serde_json::json!({"shutdown":shutdown,"cancel_after_admission":cancel_after_admission,
            "poison_after_admission":poison_after_admission,"poisoned":poisoned.load(Ordering::SeqCst),
            "observer_pending":observer_pending,"admitted_result":format!("{admitted_result:?}"),"drops_while_backend_held":drops_while_held,
            "original_attempt":original.attempt_id,"active_while_held":active_while_held,"reclaimed":reclaimed.as_ref().map(|(claim,_)|&claim.attempt_id),"same_backend_joined":true})
        );
        assert!(started.is_ok() && seam_ready && after_release.is_ok());
        assert!(
            observer_pending,
            "admitted callback cannot become a pre-admission failure"
        );
        if let Some(result) = admitted_result {
            if poison_after_admission {
                assert!(poisoned.load(Ordering::SeqCst));
                assert!(
                    matches!(result, Err(echo_core::error::ReactError::Other(_))),
                    "real post-closure last-run poison keeps its original error policy"
                );
            } else {
                assert_eq!(result?, "same physical callback completed");
            }
        }
        assert_eq!(held_effect?, b"started-effect-held");
        assert_eq!(closed_effect, b"started-effect-closed");
        assert_eq!(
            drops_while_held, 0,
            "borrowed observation cannot drop the original started callback"
        );
        assert!(active_while_held);
        assert!(
            reclaimed.is_none(),
            "held original effect must not admit a second physical attempt"
        );
        assert_eq!(drops.load(Ordering::SeqCst), 1);
        Ok(())
    }

    #[tokio::test]
    async fn handle_only_unknown_shutdown_retains_original_callback_resource_root()
    -> echo_core::error::Result<()> {
        struct CallbackResource {
            drops: Arc<AtomicUsize>,
            _permit: tokio::sync::OwnedSemaphorePermit,
        }
        impl Drop for CallbackResource {
            fn drop(&mut self) {
                self.drops.fetch_add(1, Ordering::SeqCst);
            }
        }
        let directory = tempfile::tempdir()?;
        let effect = directory.path().join("handle-only-effect.txt");
        let capacity = Arc::new(tokio::sync::Semaphore::new(1));
        let drops = Arc::new(AtomicUsize::new(0));
        let resource = Arc::new(CallbackResource {
            drops: drops.clone(),
            _permit: capacity
                .clone()
                .acquire_owned()
                .await
                .map_err(|error| ledger_error("handle resource", error))?,
        });
        let resource_watch = Arc::downgrade(&resource);
        let release = Arc::new(tokio::sync::Notify::new());
        let observed = Arc::new(tokio::sync::Notify::new());
        let producer = Arc::new(StdMutex::new(None::<JoinHandle<std::io::Result<()>>>));
        let facts = {
            let fire_fn: OccurrenceFireFn = {
                let release = release.clone();
                let observed = observed.clone();
                let producer = producer.clone();
                let effect = effect.clone();
                Arc::new(move |invocation| {
                    let resource = resource.clone();
                    let release = release.clone();
                    let observed = observed.clone();
                    let producer = producer.clone();
                    let effect = effect.clone();
                    Box::pin(async move {
                        let _original_resource = resource;
                        let entered = Arc::new(tokio::sync::Notify::new());
                        let original_entered = entered.clone();
                        let physical = tokio::spawn(async move {
                            tokio::fs::write(&effect, b"handle-only-source-held").await?;
                            original_entered.notify_one();
                            release.notified().await;
                            tokio::fs::write(&effect, b"handle-only-source-closed").await
                        });
                        *producer.lock().unwrap_or_else(|error| error.into_inner()) =
                            Some(physical);
                        entered.notified().await;
                        observed.notify_one();
                        Err(echo_core::error::ReactError::ExecutionNotSettled {
                            run_id: invocation.occurrence_id,
                            execution_id: Some(invocation.attempt_id),
                        })
                    })
                })
            };
            let runner = SchedulerRunner::new_with_occurrence_context(
                CronTaskStore::new().with_path(directory.path().join("tasks.json")),
                CancellationToken::new(),
                fire_fn,
            )
            .await?;
            let task = CronTask::new("handle only", "* * * * *", "original callback owner");
            let id = task.id.clone();
            runner.add_task(task).await?;
            let task = current_task(&runner, &id).await?;
            runner
                .enqueue_scheduled(&ScheduledOccurrence {
                    task,
                    scheduled_at: Utc::now(),
                })
                .await?;
            // No caller-held Runner or FireFn remains after this spawn.
            let mut handle = Arc::new(runner).spawn();
            let entered =
                tokio::time::timeout(std::time::Duration::from_secs(2), observed.notified()).await;
            let mut borrowed = Box::pin(handle.join());
            assert!(futures::poll!(borrowed.as_mut()).is_pending());
            drop(borrowed);
            assert!(
                handle.join.is_some(),
                "cancelled borrowed wait keeps original join"
            );
            let shutdown = handle.shutdown().await;
            let repeated = handle.join().await;
            assert!(matches!((&shutdown, &repeated),
                (Err(echo_core::error::ReactError::ExecutionNotSettled { run_id:a, execution_id:b }),
                 Err(echo_core::error::ReactError::ExecutionNotSettled { run_id:c, execution_id:d })) if a==c && b==d));
            let held = (
                drops.load(Ordering::SeqCst),
                capacity.available_permits(),
                resource_watch.strong_count(),
            );
            let held_effect = tokio::fs::read(&effect).await;
            release.notify_one();
            let physical = producer
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .take()
                .ok_or_else(|| ledger_error("handle counter", "original physical join missing"))?;
            tokio::time::timeout(std::time::Duration::from_secs(2), physical)
                .await
                .map_err(|_| {
                    ledger_error("handle counter", "released original backend did not close")
                })?
                .map_err(|error| ledger_error("handle counter", error))??;
            (entered, shutdown, held, held_effect)
            // With the borrowed API, scope exit explicitly destroys this last
            // parent only after the same actual backend has closed and joined.
        };
        eprintln!(
            "SCHEDULER_HANDLE_ROOT {}",
            serde_json::json!({"shutdown":format!("{:?}",facts.1),
            "drops_while_backend_held":facts.2.0,"capacity_while_held":facts.2.1,"strong_root_while_held":facts.2.2,
            "same_backend_joined":true,"drops_after_parent_scope":drops.load(Ordering::SeqCst)})
        );
        assert!(facts.0.is_ok());
        assert_eq!(facts.3?, b"handle-only-source-held");
        assert_eq!(
            tokio::fs::read(&effect).await?,
            b"handle-only-source-closed"
        );
        assert!(matches!(
            facts.1,
            Err(echo_core::error::ReactError::ExecutionNotSettled { .. })
        ));
        assert_eq!(facts.2, (0, 0, 1));
        assert_eq!(drops.load(Ordering::SeqCst), 1);
        assert_eq!(capacity.available_permits(), 1);
        assert_eq!(resource_watch.strong_count(), 0);
        Ok(())
    }

    #[tokio::test]
    async fn protected_callback_factory_and_poll_panic_keep_original_effect_fenced()
    -> echo_core::error::Result<()> {
        for poll_panic in [false, true] {
            let directory = tempfile::tempdir()?;
            let effect = directory.path().join("panicked-owner-source.txt");
            let release = Arc::new(tokio::sync::Notify::new());
            let entered = Arc::new(tokio::sync::Notify::new());
            let producer = Arc::new(StdMutex::new(None::<JoinHandle<std::io::Result<()>>>));
            let calls = Arc::new(AtomicUsize::new(0));
            let fire_fn: OccurrenceFireFn = {
                let release = release.clone();
                let entered = entered.clone();
                let producer = producer.clone();
                let effect = effect.clone();
                let calls = calls.clone();
                Arc::new(move |_| {
                    calls.fetch_add(1, Ordering::SeqCst);
                    let release = release.clone();
                    let entered = entered.clone();
                    let effect = effect.clone();
                    let backend = tokio::spawn(async move {
                        tokio::fs::write(&effect, b"owner-panic-backend-held").await?;
                        entered.notify_one();
                        release.notified().await;
                        tokio::fs::write(&effect, b"owner-panic-backend-closed").await
                    });
                    *producer.lock().unwrap_or_else(|error| error.into_inner()) = Some(backend);
                    if !poll_panic {
                        std::panic::resume_unwind(Box::new(
                            "controlled original callback factory loss",
                        ));
                    }
                    Box::pin(async move {
                        std::panic::resume_unwind(Box::new(
                            "controlled original callback poll loss",
                        ));
                    })
                })
            };
            let store = CronTaskStore::new().with_path(directory.path().join("tasks.json"));
            let runner = SchedulerRunner::new_with_occurrence_context(
                store.clone(),
                CancellationToken::new(),
                fire_fn.clone(),
            )
            .await?;
            let task = CronTask::new("owner loss", "* * * * *", "original effect");
            let id = task.id.clone();
            runner.add_task(task).await?;
            let result = runner.run_once(&id).await;
            let started =
                tokio::time::timeout(std::time::Duration::from_secs(2), entered.notified()).await;
            let record = runner
                .occurrence_authority
                .ledger
                .with_projection(|projection| projection.records().next().cloned())
                .ok_or_else(|| ledger_error("lost owner control", "original claim missing"))?;
            let original_task = current_task(&runner, &id).await?;
            let held_effect = tokio::fs::read(&effect).await;
            drop(runner);
            let reopened = SchedulerRunner::new_with_occurrence_context(
                store,
                CancellationToken::new(),
                fire_fn,
            )
            .await?;
            let next = reopened.occurrence_authority.claim_next()?;
            release.notify_one();
            let backend = producer
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .take()
                .ok_or_else(|| ledger_error("lost owner control", "original backend missing"))?;
            tokio::time::timeout(std::time::Duration::from_secs(2), backend)
                .await
                .map_err(|_| ledger_error("lost owner control", "original backend did not close"))?
                .map_err(|error| ledger_error("lost owner control", error))??;
            assert!(started.is_ok());
            assert_eq!(held_effect?, b"owner-panic-backend-held");
            assert_eq!(
                tokio::fs::read(&effect).await?,
                b"owner-panic-backend-closed"
            );
            assert!(
                matches!(result,Err(echo_core::error::ReactError::ExecutionNotSettled {run_id,execution_id})
                if run_id==record.message_id && execution_id==record.attempt_id)
            );
            assert_eq!(record.phase, DeliveryPhase::EffectStarted);
            assert!(!record.terminal);
            assert!(record.outcome.is_none() && original_task.last_result.is_none());
            assert!(next.is_none());
            assert_eq!(calls.load(Ordering::SeqCst), 1);
        }
        Ok(())
    }

    #[test]
    fn protected_occurrence_payload_rejects_legacy_decoder_and_keeps_original_fields()
    -> echo_core::error::Result<()> {
        let task = CronTask::new("codec", "* * * * *", "original fields");
        let mut payload = OccurrencePayload {
            task,
            scheduled_at: Utc::now(),
            trigger: SchedulerTrigger::Manual,
            control_revision: 7,
            protection: OccurrenceProtection::LegacyAtLeastOnce,
        };
        let old = serde_json::to_value(&payload)
            .map_err(|error| ledger_error("legacy encoding", error))?;
        let decoded: OccurrencePayload = serde_json::from_value(old.clone())
            .map_err(|error| ledger_error("legacy decoding", error))?;
        assert_eq!(decoded, payload);
        assert!(serde_json::from_value::<OccurrencePayloadFields>(old).is_ok());
        payload.protection = OccurrenceProtection::PhysicalSettlementV1;
        let protected = serde_json::to_value(&payload)
            .map_err(|error| ledger_error("protected encoding", error))?;
        assert!(serde_json::from_value::<OccurrencePayloadFields>(protected.clone()).is_err());
        assert_eq!(
            serde_json::from_value::<OccurrencePayload>(protected)
                .map_err(|error| ledger_error("protected decoding", error))?,
            payload
        );
        Ok(())
    }

    #[tokio::test]
    async fn known_callback_failure_is_terminal_and_not_replayed() -> echo_core::error::Result<()> {
        let root = std::env::temp_dir().join(format!(
            "echo-scheduler-known-failure-{}",
            uuid::Uuid::new_v4()
        ));
        let store = CronTaskStore::new().with_path(root.join("tasks.json"));
        let attempts = Arc::new(AtomicUsize::new(0));
        let attempts_for_fn = Arc::clone(&attempts);
        let fire_fn: FireFn = Arc::new(move |_task| {
            let attempts = Arc::clone(&attempts_for_fn);
            Box::pin(async move {
                attempts.fetch_add(1, Ordering::SeqCst);
                Err(echo_core::error::ReactError::Other(
                    "known callback failure".into(),
                ))
            })
        });
        let runner = SchedulerRunner::new(store.clone(), CancellationToken::new(), fire_fn).await?;
        let task = CronTask::new("failure", "* * * * *", "run");
        let task_id = task.id.clone();
        runner.add_task(task).await?;
        assert!(runner.run_once(&task_id).await.is_err());
        let failed = runner
            .occurrence_authority
            .ledger
            .with_projection(|projection| projection.records().next().cloned())
            .ok_or_else(|| {
                echo_core::error::ReactError::Other("failed occurrence missing".into())
            })?;
        assert_eq!(failed.phase, DeliveryPhase::TurnSettled);
        assert_eq!(failed.outcome, Some(DeliveryOutcome::Failed));
        drop(runner);

        let replayed = Arc::new(AtomicUsize::new(0));
        let replayed_for_fn = Arc::clone(&replayed);
        let replay_fn: FireFn = Arc::new(move |_task| {
            let replayed = Arc::clone(&replayed_for_fn);
            Box::pin(async move {
                replayed.fetch_add(1, Ordering::SeqCst);
                Ok("unexpected replay".to_string())
            })
        });
        let replay_runner =
            SchedulerRunner::new(store, CancellationToken::new(), replay_fn).await?;
        let _ = replay_runner.drain_pending(None).await?;
        assert_eq!(attempts.load(Ordering::SeqCst), 1);
        assert_eq!(replayed.load(Ordering::SeqCst), 0);

        drop(replay_runner);
        let _ = std::fs::remove_dir_all(root);
        Ok(())
    }

    #[tokio::test]
    async fn completed_callback_is_not_replayed_when_last_run_projection_loses_definition()
    -> echo_core::error::Result<()> {
        let root = std::env::temp_dir().join(format!(
            "echo-scheduler-projection-loss-{}",
            uuid::Uuid::new_v4()
        ));
        let store = CronTaskStore::new().with_path(root.join("tasks.json"));
        let callback_started = Arc::new(tokio::sync::Notify::new());
        let callback_finish = Arc::new(tokio::sync::Notify::new());
        let started_for_fn = Arc::clone(&callback_started);
        let finish_for_fn = Arc::clone(&callback_finish);
        let fire_fn: FireFn = Arc::new(move |_task| {
            let started = Arc::clone(&started_for_fn);
            let finish = Arc::clone(&finish_for_fn);
            Box::pin(async move {
                started.notify_one();
                finish.notified().await;
                Ok("completed before projection".to_string())
            })
        });
        let runner =
            Arc::new(SchedulerRunner::new(store.clone(), CancellationToken::new(), fire_fn).await?);
        let task = CronTask::new("projection", "* * * * *", "run");
        let task_id = task.id.clone();
        let scheduled_at = Utc
            .with_ymd_and_hms(2026, 8, 13, 0, 1, 0)
            .single()
            .ok_or_else(|| echo_core::error::ReactError::Other("invalid test time".into()))?;
        runner.add_task(task.clone()).await?;
        let task = current_task(&runner, &task_id).await?;
        let occurrence_id = scheduled_occurrence_id(&task, scheduled_at)?;
        let exact_replacement = task.clone();
        let original_definition_id = task.definition_id.clone();
        let runner_for_fire = Arc::clone(&runner);
        let fire = tokio::spawn(async move {
            runner_for_fire
                .fire_task(ScheduledOccurrence { task, scheduled_at })
                .await
        });
        callback_started.notified().await;
        assert!(runner.remove_task_exact(&task_id).await?);
        runner.add_task(exact_replacement).await?;
        let replacement = current_task(&runner, &task_id).await?;
        assert_ne!(replacement.definition_id, original_definition_id);
        callback_finish.notify_one();
        let fire_result = fire.await.map_err(|error| {
            echo_core::error::ReactError::Other(format!(
                "projection-loss scheduler fire failed to join: {error}"
            ))
        })?;
        assert!(fire_result.is_err());
        let completed = runner
            .occurrence_authority
            .ledger
            .with_projection(|projection| projection.record(&occurrence_id).cloned())
            .ok_or_else(|| {
                echo_core::error::ReactError::Other("completed occurrence missing".into())
            })?;
        assert_eq!(completed.phase, DeliveryPhase::TurnSettled);
        assert_eq!(completed.outcome, Some(DeliveryOutcome::Completed));
        assert!(
            completed
                .reason
                .as_deref()
                .is_some_and(|reason| reason.contains("last-run projection failed"))
        );
        let replacement = current_task(&runner, &task_id).await?;
        assert_eq!(replacement.last_result, None);
        drop(runner);

        let replays = Arc::new(AtomicUsize::new(0));
        let replays_for_fn = Arc::clone(&replays);
        let replay_fn: FireFn = Arc::new(move |_task| {
            let replays = Arc::clone(&replays_for_fn);
            Box::pin(async move {
                replays.fetch_add(1, Ordering::SeqCst);
                Ok("unexpected replay".to_string())
            })
        });
        let replay_runner =
            SchedulerRunner::new(store, CancellationToken::new(), replay_fn).await?;
        let _ = replay_runner.drain_pending(None).await?;
        assert_eq!(replays.load(Ordering::SeqCst), 0);

        drop(replay_runner);
        let _ = std::fs::remove_dir_all(root);
        Ok(())
    }

    #[tokio::test]
    async fn callback_success_refreshes_runner_cache_after_store_commit()
    -> echo_core::error::Result<()> {
        let root =
            std::env::temp_dir().join(format!("echo-scheduler-cache-{}", uuid::Uuid::new_v4()));
        let store = CronTaskStore::new().with_path(root.join("tasks.json"));
        let fire_fn: FireFn = Arc::new(|_task| Box::pin(async { Ok("fresh result".to_string()) }));
        let runner = SchedulerRunner::new(store, CancellationToken::new(), fire_fn).await?;
        let task = CronTask::new("cache", "*/5 * * * *", "run");
        let task_id = task.id.clone();
        runner.add_task(task).await?;

        assert_eq!(runner.run_once(&task_id).await?, "fresh result");
        let cached = runner
            .list_tasks()
            .await
            .into_iter()
            .find(|task| task.id == task_id)
            .ok_or_else(|| echo_core::error::ReactError::Other("cached task missing".into()))?;
        assert_eq!(cached.last_result.as_deref(), Some("fresh result"));
        assert!(cached.last_run_at.is_some());
        let _ = std::fs::remove_dir_all(root);
        Ok(())
    }

    #[tokio::test]
    async fn add_and_last_run_update_cache_without_post_commit_reload()
    -> echo_core::error::Result<()> {
        let temp = tempfile::tempdir()?;
        let backend = Arc::new(ReloadFailureStore {
            inner: echo_state::memory::InMemoryStore::new(),
            fault: AtomicUsize::new(0),
            committed: tokio::sync::Notify::new(),
            release: tokio::sync::Notify::new(),
        });
        let store = CronTaskStore::with_backend_path_for_test(
            backend.clone(),
            temp.path().join("tasks.json"),
        );
        let fire_fn: FireFn = Arc::new(|_| Box::pin(async { Ok("fresh".to_string()) }));
        let runner = SchedulerRunner::new(store, CancellationToken::new(), fire_fn).await?;
        let task = CronTask::new("cache", "*/5 * * * *", "run");
        let task_id = task.id.clone();

        backend.fault.store(1, Ordering::SeqCst);
        runner.add_task(task).await?;
        assert!(
            runner
                .list_tasks()
                .await
                .iter()
                .any(|task| task.id == task_id)
        );
        backend.fault.store(0, Ordering::SeqCst);

        backend.fault.store(1, Ordering::SeqCst);
        assert_eq!(runner.run_once(&task_id).await?, "fresh");
        let cached = runner
            .list_tasks()
            .await
            .into_iter()
            .find(|task| task.id == task_id)
            .ok_or_else(|| echo_core::error::ReactError::Other("cached task missing".into()))?;
        assert_eq!(cached.last_result.as_deref(), Some("fresh"));
        Ok(())
    }

    #[tokio::test]
    async fn manual_run_can_overlap_an_admitted_scheduled_callback() -> echo_core::error::Result<()>
    {
        let temp = tempfile::tempdir()?;
        let store = CronTaskStore::new().with_path(temp.path().join("tasks.json"));
        let scheduled_started = Arc::new(tokio::sync::Notify::new());
        let scheduled_release = Arc::new(tokio::sync::Notify::new());
        let fire_fn: OccurrenceFireFn = Arc::new({
            let scheduled_started = scheduled_started.clone();
            let scheduled_release = scheduled_release.clone();
            move |invocation| {
                let scheduled_started = scheduled_started.clone();
                let scheduled_release = scheduled_release.clone();
                Box::pin(async move {
                    if invocation.trigger == SchedulerTrigger::Scheduled {
                        scheduled_started.notify_one();
                        scheduled_release.notified().await;
                        Ok("scheduled".to_string())
                    } else {
                        Ok("manual".to_string())
                    }
                })
            }
        });
        let runner = Arc::new(
            SchedulerRunner::new_with_occurrence_context(store, CancellationToken::new(), fire_fn)
                .await?,
        );
        let task = CronTask::new("overlap", "*/5 * * * *", "run");
        let task_id = task.id.clone();
        runner.add_task(task).await?;
        let task = current_task(&runner, &task_id).await?;
        let scheduled = tokio::spawn({
            let runner = runner.clone();
            async move {
                runner
                    .fire_task(ScheduledOccurrence {
                        task,
                        scheduled_at: Utc::now(),
                    })
                    .await
            }
        });
        scheduled_started.notified().await;
        let manual =
            tokio::time::timeout(std::time::Duration::from_secs(1), runner.run_once(&task_id))
                .await
                .map_err(|_| {
                    ledger_error("manual run", "manual run waited for scheduled callback")
                })??;
        assert_eq!(manual, "manual");
        scheduled_release.notify_one();
        scheduled
            .await
            .map_err(|error| ledger_error("scheduled join", error))??;
        Ok(())
    }

    #[tokio::test]
    async fn control_committed_before_admission_suppresses_captured_occurrence()
    -> echo_core::error::Result<()> {
        let root =
            std::env::temp_dir().join(format!("echo-scheduler-control-{}", uuid::Uuid::new_v4()));
        let store = CronTaskStore::new().with_path(root.join("tasks.json"));
        let fired = Arc::new(AtomicUsize::new(0));
        let fired_for_fn = Arc::clone(&fired);
        let fire_fn: FireFn = Arc::new(move |_task| {
            let fired = Arc::clone(&fired_for_fn);
            Box::pin(async move {
                fired.fetch_add(1, Ordering::SeqCst);
                Ok("should not fire".to_string())
            })
        });
        let runner = SchedulerRunner::new(store, CancellationToken::new(), fire_fn).await?;
        let task = CronTask::new("controlled", "*/5 * * * *", "run");
        let scheduled_at = Utc
            .with_ymd_and_hms(2026, 8, 13, 0, 1, 0)
            .single()
            .ok_or_else(|| echo_core::error::ReactError::Other("invalid test time".into()))?;
        let task_id = task.id.clone();
        runner.add_task(task).await?;
        let task = current_task(&runner, &task_id).await?;
        let occurrence = ScheduledOccurrence { task, scheduled_at };
        runner
            .set_status(&occurrence.task.id, CronTaskStatus::Disabled)
            .await?;
        runner.fire_task(occurrence).await?;
        assert_eq!(fired.load(Ordering::SeqCst), 0);

        let _ = std::fs::remove_dir_all(root);
        Ok(())
    }

    #[tokio::test]
    async fn disable_then_reenable_does_not_admit_stale_occurrence() -> echo_core::error::Result<()>
    {
        let root =
            std::env::temp_dir().join(format!("echo-scheduler-reenable-{}", uuid::Uuid::new_v4()));
        let store = CronTaskStore::new().with_path(root.join("tasks.json"));
        let fired = Arc::new(AtomicUsize::new(0));
        let fired_for_fn = Arc::clone(&fired);
        let fire_fn: FireFn = Arc::new(move |_task| {
            let fired = Arc::clone(&fired_for_fn);
            Box::pin(async move {
                fired.fetch_add(1, Ordering::SeqCst);
                Ok("stale".to_string())
            })
        });
        let runner = SchedulerRunner::new(store, CancellationToken::new(), fire_fn).await?;
        let task = CronTask::new("reenable", "*/5 * * * *", "run");
        let task_id = task.id.clone();
        runner.add_task(task.clone()).await?;
        let task = current_task(&runner, &task_id).await?;
        let occurrence = ScheduledOccurrence {
            task,
            scheduled_at: Utc
                .with_ymd_and_hms(2026, 8, 13, 0, 1, 0)
                .single()
                .ok_or_else(|| echo_core::error::ReactError::Other("invalid test time".into()))?,
        };
        runner
            .set_status(&task_id, CronTaskStatus::Disabled)
            .await?;
        runner.set_status(&task_id, CronTaskStatus::Enabled).await?;
        runner.fire_task(occurrence).await?;
        assert_eq!(fired.load(Ordering::SeqCst), 0);
        let _ = std::fs::remove_dir_all(root);
        Ok(())
    }

    #[tokio::test]
    async fn remove_then_readd_does_not_admit_stale_occurrence() -> echo_core::error::Result<()> {
        let root =
            std::env::temp_dir().join(format!("echo-scheduler-readd-{}", uuid::Uuid::new_v4()));
        let store = CronTaskStore::new().with_path(root.join("tasks.json"));
        let fired = Arc::new(AtomicUsize::new(0));
        let fired_for_fn = Arc::clone(&fired);
        let fire_fn: FireFn = Arc::new(move |_task| {
            let fired = Arc::clone(&fired_for_fn);
            Box::pin(async move {
                fired.fetch_add(1, Ordering::SeqCst);
                Ok("stale".to_string())
            })
        });
        let runner = SchedulerRunner::new(store, CancellationToken::new(), fire_fn).await?;
        let task = CronTask::new("readd", "*/5 * * * *", "run");
        let task_id = task.id.clone();
        runner.add_task(task).await?;
        let task = current_task(&runner, &task_id).await?;
        let scheduled_at = Utc
            .with_ymd_and_hms(2026, 8, 13, 0, 1, 0)
            .single()
            .ok_or_else(|| echo_core::error::ReactError::Other("invalid test time".into()))?;
        let occurrence_id = scheduled_occurrence_id(&task, scheduled_at)?;
        runner.occurrence_authority.enqueue_if_absent(
            &occurrence_id,
            OccurrencePayload {
                protection: OccurrenceProtection::LegacyAtLeastOnce,
                control_revision: task.control_revision,
                task: task.clone(),
                scheduled_at,
                trigger: SchedulerTrigger::Scheduled,
            },
        )?;
        runner.remove_task_exact(&task_id).await?;
        runner.add_task(task).await?;
        let _ = runner.drain_pending(None).await?;
        assert_eq!(fired.load(Ordering::SeqCst), 0);
        let dropped = runner
            .occurrence_authority
            .ledger
            .with_projection(|projection| projection.record(&occurrence_id).cloned())
            .ok_or_else(|| {
                echo_core::error::ReactError::Other("dropped occurrence missing".into())
            })?;
        assert_eq!(dropped.outcome, Some(DeliveryOutcome::Dropped));
        let _ = std::fs::remove_dir_all(root);
        Ok(())
    }

    #[tokio::test]
    async fn stale_callback_cannot_update_recreated_definition() -> echo_core::error::Result<()> {
        let root =
            std::env::temp_dir().join(format!("echo-scheduler-recreate-{}", uuid::Uuid::new_v4()));
        let store = CronTaskStore::new().with_path(root.join("tasks.json"));
        let fire_fn: FireFn = Arc::new(|_task| Box::pin(async { Ok("unused".to_string()) }));
        let runner = SchedulerRunner::new(store, CancellationToken::new(), fire_fn).await?;
        let old = CronTask::new("old", "*/5 * * * *", "old");
        let old_id = old.id.clone();
        runner.add_task(old).await?;
        let old = current_task(&runner, &old_id).await?;
        runner.remove_task_exact(&old_id).await?;

        runner.add_task(old.clone()).await?;
        assert!(
            runner
                .store
                .update_last_run_for_task(&old, "stale", None)
                .await
                .is_err()
        );
        let cached = runner
            .list_tasks()
            .await
            .into_iter()
            .find(|task| task.id == old_id)
            .ok_or_else(|| echo_core::error::ReactError::Other("recreated task missing".into()))?;
        assert_eq!(cached.last_result, None);
        let _ = std::fs::remove_dir_all(root);
        Ok(())
    }

    #[tokio::test]
    async fn live_runner_is_the_only_definition_mutation_owner() -> echo_core::error::Result<()> {
        let temp = tempfile::tempdir()?;
        let path = temp.path().join("tasks.json");
        let store = CronTaskStore::new().with_path(path.clone());
        let same_path = CronTaskStore::new().with_path(path);
        let fire_fn: FireFn = Arc::new(|_| Box::pin(async { Ok("done".into()) }));
        let runner = SchedulerRunner::new(store.clone(), CancellationToken::new(), fire_fn).await?;
        let task = CronTask::new("owned", "* * * * *", "run");
        runner.add_task(task.clone()).await?;

        assert!(
            store
                .set_status(&task.id, CronTaskStatus::Disabled)
                .await
                .is_err()
        );
        assert!(same_path.remove(&task.id).await.is_err());
        assert!(store.update_last_run(&task.id, "bypassed").await.is_err());
        assert!(
            same_path
                .add(CronTask::new("other", "* * * * *", "run"))
                .await
                .is_err()
        );
        assert_eq!(runner.list_tasks().await, store.load_all().await?);

        assert!(
            runner
                .set_status(&task.id, CronTaskStatus::Disabled)
                .await?
        );
        assert_eq!(runner.list_tasks().await, store.load_all().await?);
        drop(runner);
        assert!(store.remove(&task.id).await?);
        assert!(same_path.load_all().await?.is_empty());
        Ok(())
    }

    #[tokio::test]
    async fn runner_releases_definition_owner_before_reopen() -> echo_core::error::Result<()> {
        let temp = tempfile::tempdir()?;
        let store = CronTaskStore::new().with_path(temp.path().join("tasks.json"));
        let fire_fn: FireFn = Arc::new(|_| Box::pin(async { Ok("done".into()) }));
        let first = SchedulerRunner::new(
            store.clone(),
            CancellationToken::new(),
            Arc::clone(&fire_fn),
        )
        .await?;
        assert!(
            SchedulerRunner::new(
                store.clone(),
                CancellationToken::new(),
                Arc::clone(&fire_fn),
            )
            .await
            .is_err()
        );
        first.cancel.cancel();
        assert!(
            store
                .add(CronTask::new("blocked", "* * * * *", "run"))
                .await
                .is_err()
        );
        drop(first);
        let second = SchedulerRunner::new(store.clone(), CancellationToken::new(), fire_fn).await?;
        let task = CronTask::new("reopened", "* * * * *", "run");
        second.add_task(task.clone()).await?;
        assert_eq!(second.list_tasks().await, store.load_all().await?);
        drop(second);
        assert!(store.remove(&task.id).await?);
        Ok(())
    }

    #[tokio::test]
    async fn runner_claim_loads_a_committed_store_write_after_caller_cancellation()
    -> echo_core::error::Result<()> {
        let temp = tempfile::tempdir()?;
        let backend = Arc::new(ReloadFailureStore {
            inner: echo_state::memory::InMemoryStore::new(),
            fault: AtomicUsize::new(6),
            committed: Notify::new(),
            release: Notify::new(),
        });
        let store = CronTaskStore::with_backend_path_for_test(
            backend.clone(),
            temp.path().join("tasks.json"),
        );
        let task = CronTask::new("committed", "* * * * *", "run");
        let writer = tokio::spawn({
            let store = store.clone();
            let task = task.clone();
            async move { store.add(task).await }
        });
        backend.committed.notified().await;
        let fire_fn: FireFn = Arc::new(|_| Box::pin(async { Ok("done".into()) }));
        let mut constructor = tokio::spawn({
            let store = store.clone();
            async move { SchedulerRunner::new(store, CancellationToken::new(), fire_fn).await }
        });
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(20), &mut constructor)
                .await
                .is_err()
        );
        writer.abort();
        assert!(writer.await.is_err());
        let runner = constructor
            .await
            .map_err(|error| ledger_error("runner claim", error))??;
        assert_eq!(runner.list_tasks().await, store.load_all().await?);
        assert_eq!(runner.list_tasks().await.len(), 1);
        assert!(
            store
                .set_status(&task.id, CronTaskStatus::Disabled)
                .await
                .is_err()
        );
        Ok(())
    }

    #[tokio::test]
    async fn unanchored_store_clone_cannot_bypass_runner_owner() -> echo_core::error::Result<()> {
        let temp = tempfile::tempdir()?;
        let backend = Arc::new(echo_state::memory::InMemoryStore::new());
        let seeded = CronTaskStore::with_backend_path_for_test(
            backend.clone(),
            temp.path().join("tasks.json"),
        );
        let task = CronTask::new("owned", "* * * * *", "run");
        seeded.add(task.clone()).await?;
        let unanchored = CronTaskStore::with_store(backend.clone()).await?;
        let anchored = unanchored.clone().with_path(temp.path().join("tasks.json"));
        let fire_fn: FireFn = Arc::new(|_| Box::pin(async { Ok("done".into()) }));
        let runner = SchedulerRunner::new(anchored, CancellationToken::new(), fire_fn).await?;

        assert!(
            unanchored
                .set_status(&task.id, CronTaskStatus::Disabled)
                .await
                .is_err()
        );
        assert!(CronTaskStore::with_store(backend).await.is_err());
        assert_eq!(runner.list_tasks().await, unanchored.load_all().await?);
        Ok(())
    }

    #[tokio::test]
    async fn reanchored_store_clone_cannot_bypass_runner_owner() -> echo_core::error::Result<()> {
        let temp = tempfile::tempdir()?;
        let backend = Arc::new(echo_state::memory::InMemoryStore::new());
        let store = CronTaskStore::with_backend_path_for_test(
            backend,
            temp.path().join("first/tasks.json"),
        );
        let reanchored = store
            .clone()
            .with_path(temp.path().join("second/tasks.json"));
        let fire_fn: FireFn = Arc::new(|_| Box::pin(async { Ok("done".into()) }));
        let runner = SchedulerRunner::new(store, CancellationToken::new(), fire_fn).await?;
        let task = CronTask::new("owned", "* * * * *", "run");
        runner.add_task(task.clone()).await?;

        assert!(reanchored.remove(&task.id).await.is_err());
        assert_eq!(runner.list_tasks().await, reanchored.load_all().await?);
        Ok(())
    }

    #[tokio::test]
    async fn migration_cannot_remove_a_live_runner_definition_file() -> echo_core::error::Result<()>
    {
        let temp = tempfile::tempdir()?;
        let legacy_path = temp.path().join(".echo-agent/scheduler/tasks.json");
        let legacy = CronTaskStore::new().with_path(legacy_path.clone());
        let task = CronTask::new("owned", "* * * * *", "run");
        legacy.add(task).await?;
        let fire_fn: FireFn = Arc::new(|_| Box::pin(async { Ok("done".into()) }));
        let runner = SchedulerRunner::new(legacy, CancellationToken::new(), fire_fn).await?;
        let backend = Arc::new(echo_state::memory::InMemoryStore::new());
        let destination = CronTaskStore::with_backend_path_for_test(
            backend,
            temp.path().join("destination/tasks.json"),
        );

        assert!(destination.migrate_from_path(&legacy_path).await.is_err());
        assert!(legacy_path.exists());
        assert_eq!(runner.list_tasks().await.len(), 1);
        assert!(destination.load_all().await?.is_empty());

        destination
            .add(CronTask::new("existing target", "* * * * *", "run"))
            .await?;
        destination.migrate_from_path(&legacy_path).await?;
        assert!(legacy_path.exists());
        assert_eq!(destination.load_all().await?.len(), 1);
        Ok(())
    }

    #[tokio::test]
    async fn run_once_does_not_bypass_disabled_status() -> echo_core::error::Result<()> {
        let root =
            std::env::temp_dir().join(format!("echo-scheduler-run-once-{}", uuid::Uuid::new_v4()));
        let store = CronTaskStore::new().with_path(root.join("tasks.json"));
        let fired = Arc::new(AtomicUsize::new(0));
        let fired_for_fn = Arc::clone(&fired);
        let fire_fn: FireFn = Arc::new(move |_task| {
            let fired = Arc::clone(&fired_for_fn);
            Box::pin(async move {
                fired.fetch_add(1, Ordering::SeqCst);
                Ok("should not fire".to_string())
            })
        });
        let runner = SchedulerRunner::new(store, CancellationToken::new(), fire_fn).await?;
        let task = CronTask::new("disabled", "*/5 * * * *", "run");
        let task_id = task.id.clone();
        runner.add_task(task).await?;
        runner
            .set_status(&task_id, CronTaskStatus::Disabled)
            .await?;
        assert!(runner.run_once(&task_id).await.is_err());
        assert_eq!(fired.load(Ordering::SeqCst), 0);
        let _ = std::fs::remove_dir_all(root);
        Ok(())
    }

    #[tokio::test]
    async fn concurrent_manual_runs_admit_and_return_their_own_results()
    -> echo_core::error::Result<()> {
        let temp = tempfile::tempdir()?;
        let store = CronTaskStore::new().with_path(temp.path().join("tasks.json"));
        let entered = Arc::new(tokio::sync::Barrier::new(3));
        let fire_fn: OccurrenceFireFn = Arc::new({
            let entered = Arc::clone(&entered);
            move |invocation| {
                let entered = Arc::clone(&entered);
                Box::pin(async move {
                    entered.wait().await;
                    Ok(invocation.task.name)
                })
            }
        });
        let runner = Arc::new(
            SchedulerRunner::new_with_occurrence_context(store, CancellationToken::new(), fire_fn)
                .await?,
        );
        let first = CronTask::new("first result", "* * * * *", "first");
        let second = CronTask::new("second result", "* * * * *", "second");
        runner.add_task(first.clone()).await?;
        runner.add_task(second.clone()).await?;
        let first_id = first.id;
        let second_id = second.id;
        let first_run = tokio::spawn({
            let runner = Arc::clone(&runner);
            async move { runner.run_once(&first_id).await }
        });
        let second_run = tokio::spawn({
            let runner = Arc::clone(&runner);
            async move { runner.run_once(&second_id).await }
        });
        tokio::time::timeout(std::time::Duration::from_secs(2), entered.wait())
            .await
            .map_err(|_| ledger_error("manual run", "callbacks did not overlap"))?;
        assert_eq!(
            first_run
                .await
                .map_err(|error| ledger_error("first", error))??,
            "first result"
        );
        assert_eq!(
            second_run
                .await
                .map_err(|error| ledger_error("second", error))??,
            "second result"
        );
        Ok(())
    }

    #[tokio::test]
    async fn cancelling_manual_run_before_admission_leaves_no_manual_occurrence()
    -> echo_core::error::Result<()> {
        let temp = tempfile::tempdir()?;
        let store = CronTaskStore::new().with_path(temp.path().join("tasks.json"));
        let started = Arc::new(tokio::sync::Notify::new());
        let release = Arc::new(tokio::sync::Notify::new());
        let fire_fn: FireFn = Arc::new({
            let started = Arc::clone(&started);
            let release = release.clone();
            move |_| {
                let started = Arc::clone(&started);
                let release = release.clone();
                Box::pin(async move {
                    started.notify_one();
                    release.notified().await;
                    Ok("older original callback closed".into())
                })
            }
        });
        let runner =
            Arc::new(SchedulerRunner::new(store, CancellationToken::new(), fire_fn).await?);
        let task = CronTask::new("manual", "* * * * *", "run");
        runner.add_task(task.clone()).await?;
        let task = current_task(&runner, &task.id).await?;
        runner.occurrence_authority.enqueue_if_absent(
            "older",
            OccurrencePayload {
                protection: OccurrenceProtection::LegacyAtLeastOnce,
                control_revision: task.control_revision,
                task: task.clone(),
                scheduled_at: Utc::now(),
                trigger: SchedulerTrigger::Scheduled,
            },
        )?;
        let started_wait = started.notified();
        let manual = tokio::spawn({
            let runner = Arc::clone(&runner);
            async move { runner.run_once(&task.id).await }
        });
        tokio::time::timeout(std::time::Duration::from_secs(2), started_wait)
            .await
            .map_err(|_| ledger_error("manual cancellation", "older callback did not start"))?;
        manual.abort();
        assert!(manual.await.is_err_and(|error| error.is_cancelled()));
        let persisted_manual = runner
            .occurrence_authority
            .journal
            .replay_after(0, 128)?
            .into_iter()
            .any(|record| {
                matches!(record.event.as_ref(), OccurrenceEvent::Persisted { envelope, .. }
                    if envelope.message_id.starts_with("cron-manual:"))
            });
        release.notify_one();
        tokio::time::timeout(std::time::Duration::from_secs(2), async {
            loop {
                if runner
                    .occurrence_authority
                    .ledger
                    .with_projection(|projection| {
                        projection
                            .record("older")
                            .is_some_and(|record| record.terminal)
                    })
                {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .map_err(|_| ledger_error("older original callback", "not physically settled"))?;
        assert!(!persisted_manual);
        Ok(())
    }

    #[tokio::test]
    async fn terminal_commit_error_returns_to_manual_caller_without_replay()
    -> echo_core::error::Result<()> {
        let temp = tempfile::tempdir()?;
        let store = CronTaskStore::new().with_path(temp.path().join("tasks.json"));
        let calls = Arc::new(AtomicUsize::new(0));
        let fire_fn: OccurrenceFireFn = Arc::new({
            let calls = Arc::clone(&calls);
            move |_| {
                calls.fetch_add(1, Ordering::SeqCst);
                Box::pin(async { Ok("settled".into()) })
            }
        });
        let runner =
            SchedulerRunner::new_with_occurrence_context(store, CancellationToken::new(), fire_fn)
                .await?;
        let task = CronTask::new("manual", "* * * * *", "run");
        runner.add_task(task.clone()).await?;
        runner
            .occurrence_authority
            .settle_after_commit_fault
            .store(true, Ordering::Release);
        let result =
            tokio::time::timeout(std::time::Duration::from_secs(2), runner.run_once(&task.id))
                .await
                .map_err(|_| ledger_error("manual terminal", "caller waited forever"))?;
        assert!(result.is_err());
        runner.drain_pending(None).await?;
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        Ok(())
    }

    #[tokio::test]
    async fn generated_manual_ids_remain_unique_across_restart_without_cold_scan()
    -> echo_core::error::Result<()> {
        let temp = tempfile::tempdir()?;
        let store = CronTaskStore::new().with_path(temp.path().join("tasks.json"));
        let ids = Arc::new(StdMutex::new(Vec::<String>::new()));
        let fire_fn: OccurrenceFireFn = Arc::new({
            let ids = Arc::clone(&ids);
            move |invocation| {
                ids.lock()
                    .unwrap_or_else(|error| error.into_inner())
                    .push(invocation.occurrence_id);
                Box::pin(async { Ok("done".into()) })
            }
        });
        let runner = SchedulerRunner::new_with_occurrence_context(
            store.clone(),
            CancellationToken::new(),
            Arc::clone(&fire_fn),
        )
        .await?;
        let task = CronTask::new("manual identity", "* * * * *", "run");
        runner.add_task(task.clone()).await?;
        let generation = runner
            .occurrence_authority
            .journal
            .journal_identity()
            .to_string();
        assert_eq!(runner.run_once(&task.id).await?, "done");
        assert_eq!(runner.run_once(&task.id).await?, "done");
        assert_eq!(
            runner
                .occurrence_authority
                .cold_scans
                .load(Ordering::Acquire),
            0
        );
        drop(runner);
        let reopened =
            SchedulerRunner::new_with_occurrence_context(store, CancellationToken::new(), fire_fn)
                .await?;
        assert_eq!(reopened.run_once(&task.id).await?, "done");
        assert_eq!(
            reopened
                .occurrence_authority
                .cold_scans
                .load(Ordering::Acquire),
            0
        );
        let prefix = format!("cron-manual:{generation}:");
        let ids = ids.lock().unwrap_or_else(|error| error.into_inner());
        assert_eq!(ids.len(), 3);
        let sequences = ids
            .iter()
            .map(|id| {
                id.strip_prefix(&prefix)
                    .and_then(|sequence| sequence.parse::<u64>().ok())
                    .ok_or_else(|| ledger_error("manual identity", "invalid generation/sequence"))
            })
            .collect::<echo_core::error::Result<Vec<_>>>()?;
        assert!(sequences.windows(2).all(|pair| pair.first() < pair.get(1)));
        Ok(())
    }

    #[tokio::test]
    async fn manual_sequence_is_reused_only_when_no_occurrence_committed()
    -> echo_core::error::Result<()> {
        let temp = tempfile::tempdir()?;
        let store = CronTaskStore::new().with_path(temp.path().join("tasks.json"));
        let fire_fn: FireFn = Arc::new(|_| Box::pin(async { Ok("done".into()) }));
        let runner = SchedulerRunner::new(store, CancellationToken::new(), fire_fn).await?;
        let task = CronTask::new("manual sequence", "* * * * *", "run");
        runner.add_task(task.clone()).await?;
        let task = current_task(&runner, &task.id).await?;
        let payload = OccurrencePayload {
            protection: OccurrenceProtection::LegacyAtLeastOnce,
            control_revision: task.control_revision,
            task,
            scheduled_at: Utc::now(),
            trigger: SchedulerTrigger::Manual,
        };
        let before = runner.occurrence_authority.journal.next_sequence();
        let mut invalid = payload.clone();
        invalid.task.id.clear();
        assert!(
            runner
                .occurrence_authority
                .enqueue_generated_manual(invalid)
                .is_err()
        );
        assert_eq!(runner.occurrence_authority.journal.next_sequence(), before);

        let first_id = format!(
            "cron-manual:{}:{before}",
            runner.occurrence_authority.journal.journal_identity()
        );
        runner
            .occurrence_authority
            .enqueue_after_commit_fault
            .store(true, Ordering::Release);
        assert!(
            runner
                .occurrence_authority
                .enqueue_generated_manual(payload.clone())
                .is_err()
        );
        let second_id = runner
            .occurrence_authority
            .enqueue_generated_manual(payload.clone())?;
        assert_ne!(first_id, second_id);
        assert_eq!(
            runner
                .occurrence_authority
                .cold_scans
                .load(Ordering::Acquire),
            0
        );
        let legacy_id = "cron-manual:legacy-fixed";
        assert!(
            runner
                .occurrence_authority
                .enqueue_if_absent(legacy_id, payload.clone())?
        );
        assert!(
            !runner
                .occurrence_authority
                .enqueue_if_absent(legacy_id, payload)?
        );
        assert_eq!(
            runner
                .occurrence_authority
                .cold_scans
                .load(Ordering::Acquire),
            1
        );
        let persisted = runner
            .occurrence_authority
            .journal
            .replay_after(0, 128)?
            .into_iter()
            .filter_map(|record| match record.event.as_ref() {
                OccurrenceEvent::Persisted { envelope, .. } => Some(envelope.message_id.clone()),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(persisted, vec![first_id, second_id, legacy_id.to_string()]);
        Ok(())
    }

    #[tokio::test]
    async fn tick_persists_every_due_occurrence_before_waiting_on_callbacks()
    -> echo_core::error::Result<()> {
        let temp = tempfile::tempdir()?;
        let store = CronTaskStore::new().with_path(temp.path().join("tasks.json"));
        let started = Arc::new(tokio::sync::Notify::new());
        let release = Arc::new(tokio::sync::Notify::new());
        let fire_fn: OccurrenceFireFn = Arc::new({
            let started = Arc::clone(&started);
            let release = Arc::clone(&release);
            move |invocation| {
                let started = Arc::clone(&started);
                let release = Arc::clone(&release);
                Box::pin(async move {
                    if invocation.task.name == "slow" {
                        started.notify_one();
                        release.notified().await;
                    }
                    Ok(invocation.task.name)
                })
            }
        });
        let runner = Arc::new(
            SchedulerRunner::new_with_occurrence_context(store, CancellationToken::new(), fire_fn)
                .await?,
        );
        let slow = CronTask::new("slow", "* * * * *", "run");
        let fast = CronTask::new("fast", "* * * * *", "run");
        runner.add_task(slow.clone()).await?;
        runner.add_task(fast.clone()).await?;
        let slow = current_task(&runner, &slow.id).await?;
        let fast = current_task(&runner, &fast.id).await?;
        let previous = Utc::now() - chrono::Duration::seconds(95);
        let now = Utc::now();
        *runner.last_tick_at.write().await = previous;
        let fast_at = fast
            .next_run_after(&previous)
            .ok_or_else(|| ledger_error("tick", "fast task has no due scheduled timestamp"))?;
        let slow_at = slow
            .next_run_after(&previous)
            .ok_or_else(|| ledger_error("tick", "slow task has no due scheduled timestamp"))?;
        let started_wait = started.notified();
        let tick = tokio::spawn({
            let runner = Arc::clone(&runner);
            async move { runner.tick_at(now).await }
        });
        tokio::time::timeout(std::time::Duration::from_secs(2), started_wait)
            .await
            .map_err(|_| ledger_error("tick", "slow callback did not start"))?;
        for id in [
            scheduled_occurrence_id(&slow, slow_at)?,
            scheduled_occurrence_id(&fast, fast_at)?,
        ] {
            assert!(
                runner
                    .occurrence_authority
                    .ledger
                    .with_projection(|projection| { projection.record(&id).is_some() })
            );
        }
        release.notify_one();
        tick.await
            .map_err(|error| ledger_error("tick join", error))?;
        Ok(())
    }

    #[tokio::test]
    async fn evicted_terminal_occurrence_cannot_repeat_after_tick_or_restart()
    -> echo_core::error::Result<()> {
        let temp = tempfile::tempdir()?;
        let store = CronTaskStore::new().with_path(temp.path().join("tasks.json"));
        let calls = Arc::new(AtomicUsize::new(0));
        let fire_fn: FireFn = Arc::new({
            let calls = Arc::clone(&calls);
            move |_| {
                calls.fetch_add(1, Ordering::SeqCst);
                Box::pin(async { Err(echo_core::error::ReactError::Other("known failure".into())) })
            }
        });
        let runner = SchedulerRunner::new(
            store.clone(),
            CancellationToken::new(),
            Arc::clone(&fire_fn),
        )
        .await?;
        let task = CronTask::new("oversized", "* * * * *", &"x".repeat(300_000));
        runner.add_task(task.clone()).await?;
        let task = current_task(&runner, &task.id).await?;
        let now = Utc::now();
        let previous = now - chrono::Duration::seconds(95);
        let scheduled_at = task
            .next_run_after(&previous)
            .ok_or_else(|| ledger_error("oversized terminal", "task has no due occurrence"))?;
        let occurrence = ScheduledOccurrence {
            task: task.clone(),
            scheduled_at,
        };
        let occurrence_id = scheduled_occurrence_id(&task, occurrence.scheduled_at)?;
        assert!(runner.fire_task(occurrence.clone()).await.is_err());
        assert!(
            runner
                .occurrence_authority
                .ledger
                .with_projection(|projection| { projection.record(&occurrence_id).is_none() })
        );
        runner.fire_task(occurrence.clone()).await?;
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        *runner.last_tick_at.write().await = previous;
        runner.tick_at(now).await;
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        drop(runner);

        let reopened = SchedulerRunner::new(store, CancellationToken::new(), fire_fn).await?;
        *reopened.last_tick_at.write().await = previous;
        reopened.tick_at(now).await;
        reopened.fire_task(occurrence).await?;
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        Ok(())
    }

    #[tokio::test]
    async fn newer_scheduled_timestamp_avoids_cold_journal_lookup() -> echo_core::error::Result<()>
    {
        let temp = tempfile::tempdir()?;
        let store = CronTaskStore::new().with_path(temp.path().join("tasks.json"));
        let fire_fn: FireFn = Arc::new(|_| Box::pin(async { Ok("done".into()) }));
        let runner = SchedulerRunner::new(
            store.clone(),
            CancellationToken::new(),
            Arc::clone(&fire_fn),
        )
        .await?;
        let task = CronTask::new("watermark", "* * * * *", "run");
        let other = CronTask::new("second watermark", "* * * * *", "run");
        runner.add_task(task.clone()).await?;
        runner.add_task(other.clone()).await?;
        let task = current_task(&runner, &task.id).await?;
        let other = current_task(&runner, &other.id).await?;
        let first_at = Utc::now();
        for task in [task.clone(), other.clone()] {
            for scheduled_at in [first_at, first_at + chrono::Duration::minutes(1)] {
                runner
                    .fire_task(ScheduledOccurrence {
                        task: task.clone(),
                        scheduled_at,
                    })
                    .await?;
            }
        }
        assert_eq!(
            runner
                .occurrence_authority
                .cold_scans
                .load(Ordering::Acquire),
            0
        );
        assert_eq!(
            runner
                .occurrence_authority
                .latest_scheduled_at
                .lock()
                .map_err(|error| ledger_error("watermarks", error))?
                .len(),
            2
        );
        runner.remove_task_exact(&task.id).await?;
        assert_eq!(
            runner
                .occurrence_authority
                .latest_scheduled_at
                .lock()
                .map_err(|error| ledger_error("watermarks", error))?
                .len(),
            1
        );
        drop(runner);
        let reopened = SchedulerRunner::new(store, CancellationToken::new(), fire_fn).await?;
        assert_eq!(
            reopened
                .occurrence_authority
                .latest_scheduled_at
                .lock()
                .map_err(|error| ledger_error("reopened watermarks", error))?
                .len(),
            1
        );
        reopened
            .fire_task(ScheduledOccurrence {
                task: other,
                scheduled_at: first_at + chrono::Duration::minutes(2),
            })
            .await?;
        assert_eq!(
            reopened
                .occurrence_authority
                .cold_scans
                .load(Ordering::Acquire),
            0
        );
        Ok(())
    }

    #[tokio::test]
    async fn readded_definition_is_not_suppressed_by_prior_tick_reservation()
    -> echo_core::error::Result<()> {
        let temp = tempfile::tempdir()?;
        let store = CronTaskStore::new().with_path(temp.path().join("tasks.json"));
        let fired = Arc::new(AtomicUsize::new(0));
        let fire_fn: FireFn = Arc::new({
            let fired = Arc::clone(&fired);
            move |_task| {
                let fired = Arc::clone(&fired);
                Box::pin(async move {
                    fired.fetch_add(1, Ordering::SeqCst);
                    Ok("done".into())
                })
            }
        });
        let runner = SchedulerRunner::new(store, CancellationToken::new(), fire_fn).await?;
        let task = CronTask::new("readd", "* * * * *", "run");
        runner.add_task(task.clone()).await?;
        let previous = Utc::now() - chrono::Duration::seconds(95);
        let now = Utc::now();
        *runner.last_tick_at.write().await = previous;
        runner.tick_at(now).await;
        assert_eq!(fired.load(Ordering::SeqCst), 1);

        runner.remove_task_exact(&task.id).await?;
        runner.add_task(task).await?;
        *runner.last_tick_at.write().await = previous;
        runner.tick_at(now).await;
        assert_eq!(fired.load(Ordering::SeqCst), 2);
        Ok(())
    }
}
