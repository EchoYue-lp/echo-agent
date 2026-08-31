//! Sequenced event journal with checkpoint-compounded reduction.
//!
//! # Purpose
//!
//! This module is the framework-side generic primitive for the
//! "append-only sequenced journal + checkpointed projection" pattern that EKO
//! currently implements twice (ordinary-chat `ChatEventLog` and the TaskRuntime
//! `events.jsonl` + `checkpoint.json` pair). It is deliberately free of EKO
//! product types: any consumer that needs durable, replayable, ordered facts
//! with a bounded hot-path read can compose these pieces.
//!
//! # Industry basis
//!
//! - OpenAI Codex rollout recorder: the complete rollout is appended
//!   independently and durably; consumers derive bounded views from stable
//!   boundaries instead of re-deriving state from scratch.
//! - LangGraph persistence: execution progress is saved as
//!   checkpoint/thread-identity pairs, and resume continues from the persisted
//!   fact rather than replaying the whole history.
//! - EventStoreDB/KurrentDB expected-version appends: a batch is accepted as
//!   one ordered commit or rejected without exposing a record prefix.
//! - RocksDB `WriteBatch`: one batch is the physical atomicity boundary, not a
//!   loop over independently visible writes.
//! - Apache Kafka's log and commit-marker model: immutable segments divide
//!   physical I/O while commit boundaries control visibility in one global
//!   offset space.
//! - Rust [`std::fs::File::sync_data`]: a completed write and its requested
//!   durability barrier are separate outcomes, which is why receipts can be
//!   committed but durability-degraded.
//! - Classic event sourcing snapshots: fold events into state and periodically
//!   persist `(state, applied_sequence)` so recovery replays only the tail.
//!
//! # Layering decision
//!
//! Journal sequencing, durable append, tolerant replay, and
//! checkpoint compounding are generic mechanisms needed by any agent runtime,
//! so they live in the framework (`echo-state`). The segmented implementation
//! owns only its internal file naming and integrity contract. Data-root and
//! stream-directory selection, retention counts, pins, and product projections
//! stay with consumers. Migrating EKO's duplicated file algorithms onto this
//! module is planned follow-up work and deletes the duplicated algorithms at
//! that point; nothing here depends on it.
//!
//! # Invariants
//!
//! - Sequences are 1-based and contiguous (`1, 2, 3, ...`). A journal load
//!   that observes a gap or a non-monotonic record fails loudly instead of
//!   silently mis-replaying.
//! - Journal implementations serialize their append authority. File-backed
//!   implementations additionally hold an exclusive process-lifetime lease.
//! - Every physical line is one digest-protected batch frame. A torn trailing
//!   frame makes the complete batch invisible and is truncated on next open;
//!   corruption in any complete frame fails closed.
//! - [`EventReducer::apply`] is infallible: journal events are validated before
//!   append, and projection code must remain total over its own event type.
//! - One [`CheckpointedReducer`] is the single projection owner for its journal.
//!   It serializes append, fold, checkpoint, and recovery as one transaction so
//!   concurrent callers cannot reorder committed sequences.
//!
//! # Example
//!
//! ```
//! use echo_state::journal::{
//!     CheckpointStore, CheckpointedReducer, EventReducer, MemoryCheckpointStore,
//!     MemoryEventJournal, PreparedJournalBatch,
//! };
//! use std::sync::Arc;
//!
//! #[derive(Default, serde::Serialize, serde::Deserialize)]
//! struct CountingReducer {
//!     applied: u64,
//! }
//!
//! impl EventReducer for CountingReducer {
//!     type Event = String;
//!     fn apply(&mut self, _event: &String) {
//!         self.applied = self.applied.saturating_add(1);
//!     }
//! }
//!
//! # fn main() -> echo_core::error::Result<()> {
//! let journal = Arc::new(MemoryEventJournal::<String>::new());
//! let checkpoints: Arc<dyn CheckpointStore<CountingReducer>> =
//!     Arc::new(MemoryCheckpointStore::new());
//! let reducer = CheckpointedReducer::new(Arc::clone(&journal), Arc::clone(&checkpoints), 2);
//!
//! let batch = PreparedJournalBatch::new(vec!["one".to_string(), "two".to_string()])
//!     .map_err(|error| echo_core::error::ReactError::Other(error.to_string()))?;
//! let committed = reducer
//!     .apply_batch(batch)
//!     .map_err(|error| echo_core::error::ReactError::Other(error.to_string()))?;
//! assert_eq!(committed.record_count, 2); // one physical commit frame
//! reducer
//!     .apply("three".to_string())
//!     .map_err(|error| echo_core::error::ReactError::Other(error.to_string()))?;
//! assert_eq!(reducer.last_applied_sequence(), 3);
//!
//! // Recovery loads the checkpoint and replays only the tail.
//! let recovered = CheckpointedReducer::new(journal, checkpoints, 2);
//! assert_eq!(recovered.recover()?.last_applied_sequence, 3);
//! recovered.with_state(|state| assert_eq!(state.applied, 3));
//! # Ok(())
//! }
//! ```

pub mod file;
pub mod segmented;

pub use file::{FileCheckpointStore, FileEventJournal};
pub use segmented::{
    JournalPhysicalCleanupStatus, JournalPhysicalSegmentMetadata, JournalPruneCommitStatus,
    JournalPruneReceipt, JournalRetentionMetadata, JournalSegmentMetadata,
    SegmentedFileEventJournal,
};

use echo_core::error::{ReactError, Result};
use echo_core::utils::canonical_json::canonical_json_bytes;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::fmt;
use std::marker::PhantomData;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, Weak};

#[cfg(test)]
pub(crate) type TestError = Box<dyn std::error::Error + Send + Sync>;

#[cfg(test)]
pub(crate) type TestResult<T = ()> = std::result::Result<T, TestError>;

#[cfg(test)]
pub(crate) fn test_failure(message: impl Into<String>) -> TestError {
    Box::new(std::io::Error::other(message.into()))
}

#[cfg(test)]
pub(crate) trait TestContext<T> {
    fn test_context(self, context: &str) -> TestResult<T>;
}

#[cfg(test)]
impl<T, E: fmt::Debug> TestContext<T> for std::result::Result<T, E> {
    fn test_context(self, context: &str) -> TestResult<T> {
        match self {
            Ok(value) => Ok(value),
            Err(error) => Err(test_failure(format!("{context}: {error:?}"))),
        }
    }
}

#[cfg(test)]
impl<T> TestContext<T> for Option<T> {
    fn test_context(self, context: &str) -> TestResult<T> {
        match self {
            Some(value) => Ok(value),
            None => Err(test_failure(format!("{context}: value was absent"))),
        }
    }
}

const JOURNAL_BATCH_SCHEMA_VERSION: u16 = 1;

// File-backed runtime caches normally keep far fewer authorities live. The
// soft range scans at a fixed cadence; the hard limit scans immediately to
// bound dead paths. This is lifecycle hygiene, not an eviction policy: live
// authorities are never removed.
pub(super) const WEAK_REGISTRY_PRUNE_THRESHOLD: usize = 128;
pub(super) const WEAK_REGISTRY_PRUNE_INTERVAL: usize = 32;
pub(super) const WEAK_REGISTRY_HARD_LIMIT: usize = 256;

struct WeakRegistryEntry<T> {
    authority: Weak<T>,
    journal_handle_count: usize,
}

pub(super) struct WeakRegistry<T> {
    entries: HashMap<PathBuf, WeakRegistryEntry<T>>,
    opens_since_prune: usize,
}

impl<T> WeakRegistry<T> {
    pub(super) fn new() -> Self {
        Self {
            entries: HashMap::new(),
            opens_since_prune: 0,
        }
    }

    pub(super) fn prune_dead_if_due(&mut self) {
        self.opens_since_prune = self.opens_since_prune.saturating_add(1);
        let len = self.entries.len();
        let hard_limit_due = len >= WEAK_REGISTRY_HARD_LIMIT;
        let cadence_due = (WEAK_REGISTRY_PRUNE_THRESHOLD..WEAK_REGISTRY_HARD_LIMIT).contains(&len)
            && self.opens_since_prune >= WEAK_REGISTRY_PRUNE_INTERVAL;
        if !hard_limit_due && !cadence_due {
            return;
        }
        self.entries
            .retain(|_, entry| entry.authority.strong_count() > 0);
        self.opens_since_prune = 0;
    }

    pub(super) fn upgrade(&self, path: &Path) -> Option<Arc<T>> {
        self.entries
            .get(path)
            .and_then(|entry| entry.authority.upgrade())
    }

    pub(super) fn insert(&mut self, path: PathBuf, authority: &Arc<T>) {
        self.entries.insert(
            path,
            WeakRegistryEntry {
                authority: Arc::downgrade(authority),
                journal_handle_count: 1,
            },
        );
    }

    pub(super) fn add_handle(&mut self, path: &Path, authority: &Arc<T>) -> bool {
        let authority = Arc::downgrade(authority);
        let Some(entry) = self.entries.get_mut(path) else {
            return false;
        };
        if !entry.authority.ptr_eq(&authority) {
            return false;
        }
        let Some(next) = entry.journal_handle_count.checked_add(1) else {
            return false;
        };
        entry.journal_handle_count = next;
        true
    }

    /// Release one successfully returned journal handle. `true` means the
    /// exact registry entry reached zero and was removed.
    pub(super) fn release_handle(&mut self, path: &Path, authority: &Arc<T>) -> bool {
        let authority = Arc::downgrade(authority);
        let Some(entry) = self.entries.get_mut(path) else {
            return false;
        };
        if !entry.authority.ptr_eq(&authority) || entry.journal_handle_count == 0 {
            return false;
        }
        entry.journal_handle_count = entry.journal_handle_count.saturating_sub(1);
        if entry.journal_handle_count != 0 {
            return false;
        }
        self.entries.remove(path);
        true
    }

    #[cfg(test)]
    pub(super) fn paths_beneath(&self, root: &Path) -> usize {
        self.entries
            .keys()
            .filter(|path| path.starts_with(root))
            .count()
    }
}

/// Event payload accepted by an [`EventJournal`].
///
/// A blanket impl covers every `serde`-capable, thread-safe type. Events do not
/// need to be [`Clone`]: append consumes the value and records share it through
/// an [`Arc`] for receipts and in-memory replay.
pub trait JournalEvent:
    Serialize + DeserializeOwned + Send + Sync + std::fmt::Debug + 'static
{
}

impl<T> JournalEvent for T where
    T: Serialize + DeserializeOwned + Send + Sync + std::fmt::Debug + 'static
{
}

/// One journal entry with its assigned sequence.
#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct JournalRecord<E> {
    /// Identity of the physical batch frame containing this record.
    batch_id: String,
    /// 1-based contiguous sequence assigned by the journal.
    pub sequence: u64,
    /// The persisted event payload.
    pub event: Arc<E>,
}

impl<E> Clone for JournalRecord<E> {
    fn clone(&self) -> Self {
        Self {
            batch_id: self.batch_id.clone(),
            sequence: self.sequence,
            event: Arc::clone(&self.event),
        }
    }
}

impl<E> JournalRecord<E> {
    /// Build a record while preserving the physical batch identity and
    /// sequence assigned by another typed journal adapter.
    pub fn from_parts(batch_id: impl Into<String>, sequence: u64, event: E) -> Self {
        Self {
            batch_id: batch_id.into(),
            sequence,
            event: Arc::new(event),
        }
    }

    /// Stable identity of the physical batch frame containing this record.
    pub fn batch_id(&self) -> &str {
        &self.batch_id
    }
}

#[derive(Debug, Serialize)]
struct BatchIntegrity<'a, E> {
    schema_version: u16,
    batch_id: &'a str,
    first_sequence: u64,
    records: &'a [JournalRecord<E>],
}

#[derive(Debug, Serialize)]
struct StoredBatchFrameRef<'a, E> {
    schema_version: u16,
    batch_id: &'a str,
    first_sequence: u64,
    records: &'a [JournalRecord<E>],
    digest: &'a str,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct StoredBatchFrame<E> {
    schema_version: u16,
    batch_id: String,
    first_sequence: u64,
    records: Vec<JournalRecord<E>>,
    digest: String,
    #[serde(skip)]
    payload_digest: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct BatchIdentity {
    pub first_sequence: u64,
    pub record_count: u64,
    pub payload_digest: String,
    pub frame_digest: String,
}

/// A stable, prevalidated batch that owns non-clone event payloads.
///
/// Prepare once, then pass the value to [`EventJournal::append_batch`]. A
/// [`JournalBatchAppendError::NotCommitted`] returns this exact value, including
/// its original `batch_id` and payload `Arc`s, so retry never invents a new
/// identity or requires `E: Clone`.
pub struct PreparedJournalBatch<E> {
    batch_id: String,
    events: Arc<[Arc<E>]>,
    payload_digest: String,
}

impl<E> Clone for PreparedJournalBatch<E> {
    fn clone(&self) -> Self {
        Self {
            batch_id: self.batch_id.clone(),
            events: Arc::clone(&self.events),
            payload_digest: self.payload_digest.clone(),
        }
    }
}

impl<E: fmt::Debug> fmt::Debug for PreparedJournalBatch<E> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("PreparedJournalBatch")
            .field("batch_id", &self.batch_id)
            .field("event_count", &self.events.len())
            .field("payload_digest", &self.payload_digest)
            .finish()
    }
}

pub(super) struct PreparedJournalFrame<E> {
    pub next_sequence: u64,
    pub records: Arc<[JournalRecord<E>]>,
    pub line: Vec<u8>,
    pub identity: BatchIdentity,
}

fn lower_hex_digit(nibble: u8) -> char {
    match nibble {
        0..=9 => char::from(b'0'.saturating_add(nibble)),
        _ => char::from(b'a'.saturating_add(nibble.saturating_sub(10))),
    }
}

fn batch_digest<E: Serialize>(
    batch_id: &str,
    first_sequence: u64,
    records: &[JournalRecord<E>],
) -> Result<String> {
    let bytes = canonical_json_bytes(&BatchIntegrity {
        schema_version: JOURNAL_BATCH_SCHEMA_VERSION,
        batch_id,
        first_sequence,
        records,
    })
    .map_err(|error| {
        ReactError::Other(format!(
            "failed to encode journal batch integrity input: {error}"
        ))
    })?;
    let digest = Sha256::digest(bytes);
    let mut encoded = String::with_capacity(digest.len().saturating_mul(2));
    for byte in digest {
        encoded.push(lower_hex_digit(byte >> 4));
        encoded.push(lower_hex_digit(byte & 0x0f));
    }
    Ok(encoded)
}

/// Failure to form a stable batch before any journal mutation is attempted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JournalBatchPrepareError {
    batch_id: String,
    error: String,
}

impl JournalBatchPrepareError {
    /// Stable identity allocated to the batch whose preflight failed.
    pub fn batch_id(&self) -> &str {
        &self.batch_id
    }

    /// Human-readable preflight failure.
    pub fn error(&self) -> &str {
        &self.error
    }
}

impl fmt::Display for JournalBatchPrepareError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "journal batch {} failed preflight: {}",
            self.batch_id, self.error
        )
    }
}

impl std::error::Error for JournalBatchPrepareError {}

#[derive(Serialize)]
struct PreparedPayloads<'a, E> {
    batch_id: &'a str,
    events: &'a [Arc<E>],
}

fn payload_digest<E: Serialize>(batch_id: &str, events: &[Arc<E>]) -> Result<String> {
    let bytes = canonical_json_bytes(&PreparedPayloads { batch_id, events }).map_err(|error| {
        ReactError::Other(format!("failed to encode journal batch payloads: {error}"))
    })?;
    let digest = Sha256::digest(bytes);
    let mut encoded = String::with_capacity(digest.len().saturating_mul(2));
    for byte in digest {
        encoded.push(lower_hex_digit(byte >> 4));
        encoded.push(lower_hex_digit(byte & 0x0f));
    }
    Ok(encoded)
}

impl<E: JournalEvent> PreparedJournalBatch<E> {
    /// Own a non-empty ordered event set and prevalidate its serialization.
    pub fn new(events: Vec<E>) -> std::result::Result<Self, JournalBatchPrepareError> {
        Self::with_identity(uuid::Uuid::new_v4().to_string(), events)
    }

    /// Own an event set under an existing batch identity.
    ///
    /// Adapters use this to preserve a journal frame identity while converting
    /// an event payload at a typed boundary. The payload digest is recomputed
    /// for the converted event type, so lookup remains deterministic.
    pub fn with_identity(
        batch_id: impl Into<String>,
        events: Vec<E>,
    ) -> std::result::Result<Self, JournalBatchPrepareError> {
        let batch_id = batch_id.into();
        if batch_id.trim().is_empty() {
            return Err(JournalBatchPrepareError {
                batch_id,
                error: "journal batch identity must not be empty".to_string(),
            });
        }
        if events.is_empty() {
            return Err(JournalBatchPrepareError {
                batch_id,
                error: "journal batch must contain at least one event".to_string(),
            });
        }
        u64::try_from(events.len()).map_err(|_| JournalBatchPrepareError {
            batch_id: batch_id.clone(),
            error: "journal batch event count exceeds supported range".to_string(),
        })?;
        let events: Arc<[Arc<E>]> = events.into_iter().map(Arc::new).collect::<Vec<_>>().into();
        let payload_digest = payload_digest(&batch_id, events.as_ref()).map_err(|error| {
            JournalBatchPrepareError {
                batch_id: batch_id.clone(),
                error: error.to_string(),
            }
        })?;
        Ok(Self {
            batch_id,
            events,
            payload_digest,
        })
    }

    /// Stable idempotency identity retained across retry and reconciliation.
    pub fn batch_id(&self) -> &str {
        &self.batch_id
    }

    /// Number of ordered payloads in this indivisible batch.
    pub fn len(&self) -> usize {
        self.events.len()
    }

    /// Whether this batch is empty. Successful preparation always returns false.
    pub fn is_empty(&self) -> bool {
        self.events.is_empty()
    }

    /// Digest of the prevalidated payload view and batch identity.
    pub fn payload_digest(&self) -> &str {
        &self.payload_digest
    }

    /// Read-only ordered payload view.
    ///
    /// Interior mutation must not alter serialization; append and lookup
    /// recompute the digest and reject a changed payload before idempotency.
    pub fn events(&self) -> &[Arc<E>] {
        self.events.as_ref()
    }

    /// Verify that a committed receipt contains this exact prepared payload.
    ///
    /// Physical commit authorities may retry or reopen independently, but a
    /// reducer must never fold a receipt for a different batch identity.
    pub fn matches_receipt(&self, receipt: &JournalBatchAppendReceipt<E>) -> Result<bool> {
        if self.batch_id != receipt.batch_id() || self.len() != receipt.records().len() {
            return Ok(false);
        }
        let events = receipt
            .records()
            .iter()
            .map(|record| Arc::clone(&record.event))
            .collect::<Vec<_>>();
        Ok(payload_digest(&self.batch_id, events.as_slice())? == self.payload_digest)
    }

    fn validate_payload_integrity(&self) -> Result<()> {
        let current = payload_digest(&self.batch_id, self.events.as_ref())?;
        if current != self.payload_digest {
            return Err(ReactError::Other(format!(
                "prepared journal batch {} payload changed after preflight",
                self.batch_id
            )));
        }
        Ok(())
    }

    #[cfg(test)]
    fn with_test_identity(
        batch_id: String,
        events: Vec<E>,
    ) -> std::result::Result<Self, JournalBatchPrepareError> {
        Self::with_identity(batch_id, events)
    }
}

pub(super) fn prepare_journal_frame<E: JournalEvent>(
    prepared: &PreparedJournalBatch<E>,
    first_sequence: u64,
) -> Result<PreparedJournalFrame<E>> {
    prepared.validate_payload_integrity()?;
    let count = u64::try_from(prepared.events.len()).map_err(|_| {
        ReactError::Other("journal batch event count exceeds supported range".to_string())
    })?;
    let next_sequence = first_sequence.checked_add(count).ok_or_else(|| {
        ReactError::Other("journal sequence exhausted before batch append".to_string())
    })?;
    let mut records = Vec::with_capacity(prepared.events.len());
    for (index, event) in prepared.events.iter().enumerate() {
        let index = u64::try_from(index).map_err(|_| {
            ReactError::Other("journal batch index exceeds supported range".to_string())
        })?;
        let sequence = first_sequence.checked_add(index).ok_or_else(|| {
            ReactError::Other("journal sequence exhausted while preparing batch".to_string())
        })?;
        records.push(JournalRecord {
            batch_id: prepared.batch_id.clone(),
            sequence,
            event: Arc::clone(event),
        });
    }
    let records: Arc<[JournalRecord<E>]> = records.into();
    let digest = batch_digest(&prepared.batch_id, first_sequence, records.as_ref())?;
    let stored = StoredBatchFrameRef {
        schema_version: JOURNAL_BATCH_SCHEMA_VERSION,
        batch_id: &prepared.batch_id,
        first_sequence,
        records: records.as_ref(),
        digest: &digest,
    };
    let mut line = serde_json::to_vec(&stored)
        .map_err(|error| ReactError::Other(format!("failed to encode journal batch: {error}")))?;
    line.push(b'\n');
    u64::try_from(line.len())
        .map_err(|_| ReactError::Other("journal batch frame exceeds supported size".to_string()))?;
    Ok(PreparedJournalFrame {
        next_sequence,
        records,
        line,
        identity: BatchIdentity {
            first_sequence,
            record_count: count,
            payload_digest: prepared.payload_digest.clone(),
            frame_digest: digest,
        },
    })
}

pub(super) fn decode_journal_batch<E: JournalEvent>(
    context: &str,
    line: &[u8],
) -> Result<StoredBatchFrame<E>> {
    let frame: StoredBatchFrame<E> = serde_json::from_slice(line)
        .map_err(|error| ReactError::Other(format!("{context}: corrupt journal batch: {error}")))?;
    if frame.schema_version != JOURNAL_BATCH_SCHEMA_VERSION {
        return Err(ReactError::Other(format!(
            "{context}: unsupported journal batch schema {}",
            frame.schema_version
        )));
    }
    if uuid::Uuid::parse_str(&frame.batch_id).is_err() {
        return Err(ReactError::Other(format!(
            "{context}: journal batch id is not a UUID"
        )));
    }
    if frame.records.is_empty() {
        return Err(ReactError::Other(format!(
            "{context}: journal batch contains no records"
        )));
    }
    for (index, record) in frame.records.iter().enumerate() {
        if record.batch_id != frame.batch_id {
            return Err(ReactError::Other(format!(
                "{context}: record batch identity does not match frame {}",
                frame.batch_id
            )));
        }
        let index = u64::try_from(index).map_err(|_| {
            ReactError::Other(format!(
                "{context}: journal batch index exceeds supported range"
            ))
        })?;
        let expected = frame.first_sequence.checked_add(index).ok_or_else(|| {
            ReactError::Other(format!("{context}: journal sequence exhausted in batch"))
        })?;
        if record.sequence != expected {
            return Err(ReactError::Other(format!(
                "{context}: journal batch sequence gap, expected {expected} but found {}",
                record.sequence
            )));
        }
    }
    let expected_digest = batch_digest(&frame.batch_id, frame.first_sequence, &frame.records)?;
    if frame.digest != expected_digest {
        return Err(ReactError::Other(format!(
            "{context}: journal batch digest mismatch at sequence {}",
            frame.first_sequence
        )));
    }
    let events = frame
        .records
        .iter()
        .map(|record| Arc::clone(&record.event))
        .collect::<Vec<_>>();
    let computed_payload_digest = payload_digest(&frame.batch_id, &events)?;
    Ok(StoredBatchFrame {
        payload_digest: computed_payload_digest,
        ..frame
    })
}

impl<E> StoredBatchFrame<E> {
    pub(super) fn batch_id(&self) -> &str {
        &self.batch_id
    }

    pub(super) fn identity(&self) -> Result<BatchIdentity> {
        let record_count = u64::try_from(self.records.len()).map_err(|_| {
            ReactError::Other("journal batch count exceeds supported range".to_string())
        })?;
        Ok(BatchIdentity {
            first_sequence: self.first_sequence,
            record_count,
            payload_digest: self.payload_digest.clone(),
            frame_digest: self.digest.clone(),
        })
    }
    pub(super) fn records(&self) -> &[JournalRecord<E>] {
        &self.records
    }

    pub(super) fn into_records(self) -> Vec<JournalRecord<E>> {
        self.records
    }
}

pub(super) fn verify_journal_batch_sequence<E>(
    context: &str,
    expected_sequence: u64,
    frame: &StoredBatchFrame<E>,
) -> Result<()> {
    if frame.first_sequence != expected_sequence {
        return Err(ReactError::Other(format!(
            "{context}: journal sequence gap, expected {expected_sequence} but batch starts at {}",
            frame.first_sequence
        )));
    }
    Ok(())
}

/// A batch append failure with an explicit retry contract.
#[derive(Debug)]
pub enum JournalBatchAppendError<E> {
    /// No record in the batch committed; retrying the whole batch is safe.
    NotCommitted {
        batch: PreparedJournalBatch<E>,
        error: String,
    },
    /// The writer cannot prove whether the batch committed. The authority is
    /// poisoned and callers must reopen and inspect recovery before retrying.
    OutcomeUnknown {
        batch: PreparedJournalBatch<E>,
        error: String,
    },
    /// The identity is already bound to a different payload or shape. Reusing
    /// it can never be retried safely, and the live authority is poisoned.
    IdentityConflict {
        batch: PreparedJournalBatch<E>,
        existing_first_sequence: u64,
        existing_record_count: u64,
        error: String,
    },
    /// Interior mutation changed a prepared payload after preflight. The
    /// stable identity remains reserved for the original digest and this value
    /// must not be retried.
    PreparedMutation {
        batch: PreparedJournalBatch<E>,
        error: String,
    },
    /// The live authority cannot accept writes until it is closed and reopened.
    AuthorityPoisoned {
        batch: PreparedJournalBatch<E>,
        error: String,
    },
}

impl<E> JournalBatchAppendError<E> {
    /// Construct a retryable pre-mutation failure that returns ownership.
    pub fn not_committed(batch: PreparedJournalBatch<E>, error: impl Into<String>) -> Self {
        Self::NotCommitted {
            batch,
            error: error.into(),
        }
    }

    /// Construct an unknown outcome that retains ownership and requires reopen.
    pub fn outcome_unknown(batch: PreparedJournalBatch<E>, error: impl Into<String>) -> Self {
        Self::OutcomeUnknown {
            batch,
            error: error.into(),
        }
    }

    /// Construct a permanent collision with an existing committed identity.
    pub fn identity_conflict(
        batch: PreparedJournalBatch<E>,
        existing_first_sequence: u64,
        existing_record_count: u64,
        error: impl Into<String>,
    ) -> Self {
        Self::IdentityConflict {
            batch,
            existing_first_sequence,
            existing_record_count,
            error: error.into(),
        }
    }

    /// Construct a non-retryable payload mutation failure.
    pub fn prepared_mutation(batch: PreparedJournalBatch<E>, error: impl Into<String>) -> Self {
        Self::PreparedMutation {
            batch,
            error: error.into(),
        }
    }

    /// Construct a refusal from a live authority that must be reopened.
    pub fn authority_poisoned(batch: PreparedJournalBatch<E>, error: impl Into<String>) -> Self {
        Self::AuthorityPoisoned {
            batch,
            error: error.into(),
        }
    }

    /// Stable identity involved in this failure.
    pub fn batch_id(&self) -> &str {
        match self {
            Self::NotCommitted { batch, .. } => &batch.batch_id,
            Self::OutcomeUnknown { batch, .. }
            | Self::IdentityConflict { batch, .. }
            | Self::PreparedMutation { batch, .. }
            | Self::AuthorityPoisoned { batch, .. } => &batch.batch_id,
        }
    }

    /// Whether the returned prepared batch may be retried immediately.
    pub fn is_retry_safe(&self) -> bool {
        matches!(self, Self::NotCommitted { .. })
    }

    /// Borrow the retained prepared batch for diagnostics or lookup.
    pub fn prepared(&self) -> Option<&PreparedJournalBatch<E>> {
        match self {
            Self::NotCommitted { batch, .. }
            | Self::OutcomeUnknown { batch, .. }
            | Self::IdentityConflict { batch, .. }
            | Self::PreparedMutation { batch, .. }
            | Self::AuthorityPoisoned { batch, .. } => Some(batch),
        }
    }

    /// Recover ownership. Non-retryable variants still require the action
    /// indicated by [`Self::requires_reopen`] or a new identity.
    pub fn into_prepared(self) -> Option<PreparedJournalBatch<E>> {
        match self {
            Self::NotCommitted { batch, .. }
            | Self::OutcomeUnknown { batch, .. }
            | Self::IdentityConflict { batch, .. }
            | Self::PreparedMutation { batch, .. }
            | Self::AuthorityPoisoned { batch, .. } => Some(batch),
        }
    }

    /// Whether this result must be resolved with a fresh authority and
    /// [`EventJournal::lookup_batch`] before any retry.
    pub fn requires_reopen(&self) -> bool {
        matches!(
            self,
            Self::OutcomeUnknown { .. }
                | Self::IdentityConflict { .. }
                | Self::AuthorityPoisoned { .. }
        )
    }
}

impl<E> fmt::Display for JournalBatchAppendError<E> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotCommitted { batch, error } => {
                write!(
                    formatter,
                    "journal batch {} was not committed: {error}",
                    batch.batch_id
                )
            }
            Self::OutcomeUnknown { batch, error } => write!(
                formatter,
                "journal batch {} has unknown commit outcome: {error}; reopen before retry",
                batch.batch_id
            ),
            Self::IdentityConflict {
                batch,
                existing_first_sequence,
                existing_record_count,
                error,
            } => write!(
                formatter,
                "journal batch {} conflicts with the committed batch at sequence {} ({} records): {error}",
                batch.batch_id, existing_first_sequence, existing_record_count
            ),
            Self::PreparedMutation { batch, error } => write!(
                formatter,
                "prepared journal batch {} changed after preflight: {error}",
                batch.batch_id
            ),
            Self::AuthorityPoisoned { batch, error } => write!(
                formatter,
                "journal authority is poisoned for batch {}: {error}; reopen required",
                batch.batch_id
            ),
        }
    }
}

impl<E: fmt::Debug> std::error::Error for JournalBatchAppendError<E> {}

pub type JournalBatchAppendResult<E> =
    std::result::Result<JournalBatchAppendReceipt<E>, JournalBatchAppendError<E>>;

/// Durability result for one complete batch frame present in the journal.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum JournalDurabilityStatus {
    /// A read-only lookup proved the frame identity but intentionally did not
    /// execute a new durability barrier.
    Unconfirmed,
    /// The append and requested durability operation both completed.
    Confirmed,
    /// The complete batch frame is present and owns all assigned sequences, but
    /// the requested durability barrier reported an error. Callers must not
    /// retry the batch.
    Degraded { error: String },
}

/// Whether an append created a frame or resolved an existing identity.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JournalBatchCommitStatus {
    /// This call committed the physical frame.
    Committed,
    /// The identity was already committed; no second frame was written.
    AlreadyCommitted,
}

/// Receipt for one committed journal append.
#[derive(Debug)]
pub struct JournalAppendReceipt<E> {
    batch_id: String,
    pub record: JournalRecord<E>,
    pub durability: JournalDurabilityStatus,
    pub commit: JournalBatchCommitStatus,
}

impl<E> Clone for JournalAppendReceipt<E> {
    fn clone(&self) -> Self {
        Self {
            batch_id: self.batch_id.clone(),
            record: self.record.clone(),
            durability: self.durability.clone(),
            commit: self.commit,
        }
    }
}

impl<E> JournalAppendReceipt<E> {
    /// Stable batch identity for this single-record receipt.
    pub fn batch_id(&self) -> &str {
        &self.batch_id
    }
}

/// Receipt for one atomically committed journal batch.
#[derive(Debug)]
pub struct JournalBatchAppendReceipt<E> {
    /// Stable identity of the physical commit frame.
    batch_id: String,
    /// Ordered records committed by the frame. Payloads remain shared through
    /// [`Arc`] and never require `E: Clone`.
    records: Arc<[JournalRecord<E>]>,
    /// One durability outcome covers the complete frame.
    durability: JournalDurabilityStatus,
    commit: JournalBatchCommitStatus,
}

impl<E> Clone for JournalBatchAppendReceipt<E> {
    fn clone(&self) -> Self {
        Self {
            batch_id: self.batch_id.clone(),
            records: Arc::clone(&self.records),
            durability: self.durability.clone(),
            commit: self.commit,
        }
    }
}

impl<E> JournalBatchAppendReceipt<E> {
    /// Rebuild a receipt after mapping records across a typed adapter boundary.
    pub fn from_parts(
        batch_id: impl Into<String>,
        records: Vec<JournalRecord<E>>,
        durability: JournalDurabilityStatus,
        commit: JournalBatchCommitStatus,
    ) -> std::result::Result<Self, String> {
        if records.is_empty() {
            return Err("journal receipt must contain at least one record".to_string());
        }
        Ok(Self {
            batch_id: batch_id.into(),
            records: records.into(),
            durability,
            commit,
        })
    }

    /// Stable committed or idempotently resolved batch identity.
    pub fn batch_id(&self) -> &str {
        &self.batch_id
    }

    /// Ordered committed records as a read-only shared slice.
    pub fn records(&self) -> &[JournalRecord<E>] {
        self.records.as_ref()
    }

    /// Durability knowledge produced by append or read-only lookup.
    pub fn durability(&self) -> &JournalDurabilityStatus {
        &self.durability
    }

    /// Whether this call created or idempotently resolved the frame.
    pub fn commit_status(&self) -> JournalBatchCommitStatus {
        self.commit
    }
}

/// Read-only identity reconciliation result. File-backed lookup never writes
/// and never executes a durability barrier.
#[derive(Debug)]
pub enum JournalBatchLookup<E> {
    /// No retained frame owns this identity and digest.
    Absent,
    /// The unique retained frame matches and supplies its original sequences.
    AlreadyCommitted(JournalBatchAppendReceipt<E>),
    /// Identity, shape, digest, or current prepared serialization conflicts.
    Conflict { error: String },
}

/// Typed single-event failure preserving prepared ownership after mutation starts.
#[derive(Debug)]
pub enum JournalAppendError<E> {
    /// Preparation failed before reaching a journal authority.
    Prepare(JournalBatchPrepareError),
    /// The single-element prepared batch reached the journal authority.
    Commit(JournalBatchAppendError<E>),
}

impl<E> JournalAppendError<E> {
    /// Stable identity allocated for this single event.
    pub fn batch_id(&self) -> &str {
        match self {
            Self::Prepare(error) => &error.batch_id,
            Self::Commit(error) => error.batch_id(),
        }
    }

    /// Whether the retained prepared value may be retried immediately.
    pub fn is_retry_safe(&self) -> bool {
        matches!(self, Self::Commit(error) if error.is_retry_safe())
    }

    /// Recover the single-element prepared batch after a commit-stage error.
    pub fn into_prepared(self) -> Option<PreparedJournalBatch<E>> {
        match self {
            Self::Prepare(_) => None,
            Self::Commit(error) => error.into_prepared(),
        }
    }

    /// Whether the live authority must be reopened before reconciliation.
    pub fn requires_reopen(&self) -> bool {
        matches!(self, Self::Commit(error) if error.requires_reopen())
    }
}

impl<E> fmt::Display for JournalAppendError<E> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Prepare(error) => error.fmt(formatter),
            Self::Commit(error) => error.fmt(formatter),
        }
    }
}

impl<E: fmt::Debug> std::error::Error for JournalAppendError<E> {}

pub type JournalAppendResult<E> =
    std::result::Result<JournalAppendReceipt<E>, JournalAppendError<E>>;

/// Typed checkpointed-reducer failure.
#[derive(Debug)]
pub enum CheckpointedApplyError<E> {
    /// Preparation failed before journal mutation.
    Prepare(JournalBatchPrepareError),
    /// The journal returned a typed ownership-preserving outcome.
    Journal(JournalBatchAppendError<E>),
    /// A committed receipt violated projection sequence invariants.
    CommittedInvariant { batch_id: String, error: String },
}

impl<E> CheckpointedApplyError<E> {
    /// Stable identity involved in this reducer apply.
    pub fn batch_id(&self) -> &str {
        match self {
            Self::Prepare(error) => &error.batch_id,
            Self::Journal(error) => error.batch_id(),
            Self::CommittedInvariant { batch_id, .. } => batch_id,
        }
    }

    /// Recover the prepared batch from a journal-stage failure.
    pub fn into_prepared(self) -> Option<PreparedJournalBatch<E>> {
        match self {
            Self::Journal(error) => error.into_prepared(),
            Self::Prepare(_) | Self::CommittedInvariant { .. } => None,
        }
    }

    /// Whether the journal authority must be reopened before reconciliation.
    pub fn requires_reopen(&self) -> bool {
        matches!(self, Self::Journal(error) if error.requires_reopen())
    }
}

impl<E> fmt::Display for CheckpointedApplyError<E> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Prepare(error) => error.fmt(formatter),
            Self::Journal(error) => error.fmt(formatter),
            Self::CommittedInvariant { batch_id, error } => write!(
                formatter,
                "committed journal batch {batch_id} violated reducer receipt invariant: {error}"
            ),
        }
    }
}

impl<E: fmt::Debug> std::error::Error for CheckpointedApplyError<E> {}

/// Append-only sequenced journal with atomic batch commits.
///
/// Implementations assign contiguous 1-based sequences at append time and can
/// replay any suffix of the journal in order.
pub trait EventJournal<E: JournalEvent>: Send + Sync {
    /// Commit a non-empty ordered event batch as one physical frame.
    ///
    /// [`JournalBatchAppendError::NotCommitted`] permits retrying the complete
    /// batch. [`JournalBatchAppendError::OutcomeUnknown`] poisons file-backed
    /// authorities and forbids blind retry until verified reopen.
    fn append_batch(&self, batch: PreparedJournalBatch<E>) -> JournalBatchAppendResult<E>;

    /// Reconcile a prepared identity after an unknown commit outcome without
    /// writing. `Absent` permits retrying the retained prepared value;
    /// `AlreadyCommitted` returns the original sequence range.
    fn lookup_batch(&self, batch: &PreparedJournalBatch<E>) -> Result<JournalBatchLookup<E>>;

    /// Consume one event and report both its shared owned record and durability
    /// outcome. A [`JournalDurabilityStatus::Degraded`] receipt means the full
    /// record owns its sequence and must not be retried.
    fn append(&self, event: E) -> JournalAppendResult<E> {
        let batch = PreparedJournalBatch::new(vec![event]).map_err(JournalAppendError::Prepare)?;
        let appended = self
            .append_batch(batch)
            .map_err(JournalAppendError::Commit)?;
        let record = appended.records.first().cloned().ok_or_else(|| {
            JournalAppendError::Prepare(JournalBatchPrepareError {
                batch_id: appended.batch_id.clone(),
                error: format!(
                    "journal batch {} committed without a record",
                    appended.batch_id
                ),
            })
        })?;
        Ok(JournalAppendReceipt {
            batch_id: appended.batch_id,
            record,
            durability: appended.durability,
            commit: appended.commit,
        })
    }

    /// Sequence that the next append will assign.
    fn next_sequence(&self) -> u64;

    /// Last committed sequence present in the journal (`0` when empty).
    /// Consult the append receipt to distinguish confirmed durability from a
    /// full-write degraded commit.
    fn last_sequence(&self) -> u64;

    /// Earliest sequence that remains logically replayable.
    ///
    /// Non-pruning journals retain their complete history and inherit `1`.
    fn retained_floor(&self) -> u64 {
        1
    }

    /// Replay up to `limit` records with `sequence > after_sequence`, in order.
    fn replay_after(&self, after_sequence: u64, limit: usize) -> Result<Vec<JournalRecord<E>>>;
}

/// In-memory [`EventJournal`] for tests and ephemeral consumers.
#[derive(Debug)]
pub struct MemoryEventJournal<E> {
    inner: Mutex<MemoryInner<E>>,
}

#[derive(Debug, Default)]
struct MemoryInner<E> {
    next_sequence: u64,
    records: Vec<JournalRecord<E>>,
    batches: HashMap<String, MemoryCommittedBatch<E>>,
    poison: Option<String>,
}

#[derive(Debug)]
struct MemoryCommittedBatch<E> {
    payload_digest: String,
    receipt: JournalBatchAppendReceipt<E>,
}

impl<E: JournalEvent> Default for MemoryEventJournal<E> {
    fn default() -> Self {
        Self::new()
    }
}

impl<E: JournalEvent> MemoryEventJournal<E> {
    pub fn new() -> Self {
        Self {
            inner: Mutex::new(MemoryInner {
                next_sequence: 1,
                records: Vec::new(),
                batches: HashMap::new(),
                poison: None,
            }),
        }
    }
}

impl<E: JournalEvent> EventJournal<E> for MemoryEventJournal<E> {
    fn append_batch(&self, batch: PreparedJournalBatch<E>) -> JournalBatchAppendResult<E> {
        if let Err(error) = batch.validate_payload_integrity() {
            return Err(JournalBatchAppendError::prepared_mutation(
                batch,
                error.to_string(),
            ));
        }
        let mut inner = self.inner.lock().unwrap_or_else(|error| error.into_inner());
        if let Some(reason) = &inner.poison {
            return Err(JournalBatchAppendError::authority_poisoned(
                batch,
                format!("memory journal is poisoned: {reason}"),
            ));
        }
        if let Some(existing) = inner.batches.get(batch.batch_id()) {
            if existing.payload_digest == batch.payload_digest()
                && existing.receipt.records().len() == batch.len()
            {
                let mut receipt = existing.receipt.clone();
                receipt.commit = JournalBatchCommitStatus::AlreadyCommitted;
                return Ok(receipt);
            }
            let reason = format!(
                "batch identity {} conflicts with an existing committed payload",
                batch.batch_id()
            );
            let existing_first_sequence = existing
                .receipt
                .records()
                .first()
                .map(|record| record.sequence)
                .unwrap_or(0);
            let existing_record_count =
                u64::try_from(existing.receipt.records().len()).unwrap_or(u64::MAX);
            inner.poison = Some(reason.clone());
            return Err(JournalBatchAppendError::identity_conflict(
                batch,
                existing_first_sequence,
                existing_record_count,
                reason,
            ));
        }
        let prepared = match prepare_journal_frame(&batch, inner.next_sequence) {
            Ok(prepared) => prepared,
            Err(error) => {
                return Err(JournalBatchAppendError::not_committed(
                    batch,
                    error.to_string(),
                ));
            }
        };
        inner.next_sequence = prepared.next_sequence;
        inner.records.extend(prepared.records.iter().cloned());
        let receipt = JournalBatchAppendReceipt {
            batch_id: batch.batch_id,
            records: prepared.records,
            durability: JournalDurabilityStatus::Confirmed,
            commit: JournalBatchCommitStatus::Committed,
        };
        inner.batches.insert(
            receipt.batch_id.clone(),
            MemoryCommittedBatch {
                payload_digest: batch.payload_digest,
                receipt: receipt.clone(),
            },
        );
        Ok(receipt)
    }

    fn lookup_batch(&self, batch: &PreparedJournalBatch<E>) -> Result<JournalBatchLookup<E>> {
        if let Err(error) = batch.validate_payload_integrity() {
            return Ok(JournalBatchLookup::Conflict {
                error: error.to_string(),
            });
        }
        let mut inner = self.inner.lock().unwrap_or_else(|error| error.into_inner());
        let Some(existing) = inner.batches.get(batch.batch_id()) else {
            return Ok(JournalBatchLookup::Absent);
        };
        if existing.payload_digest == batch.payload_digest()
            && existing.receipt.records().len() == batch.len()
        {
            let mut receipt = existing.receipt.clone();
            receipt.commit = JournalBatchCommitStatus::AlreadyCommitted;
            return Ok(JournalBatchLookup::AlreadyCommitted(receipt));
        }
        let reason = format!(
            "batch identity {} conflicts with an existing committed payload",
            batch.batch_id()
        );
        inner.poison = Some(reason.clone());
        Ok(JournalBatchLookup::Conflict { error: reason })
    }

    fn next_sequence(&self) -> u64 {
        self.inner
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .next_sequence
    }

    fn last_sequence(&self) -> u64 {
        self.inner
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .next_sequence
            .saturating_sub(1)
    }

    fn replay_after(&self, after_sequence: u64, limit: usize) -> Result<Vec<JournalRecord<E>>> {
        let inner = self.inner.lock().unwrap_or_else(|error| error.into_inner());
        let start = usize::try_from(after_sequence).map_err(|_| {
            ReactError::Other("journal sequence exceeds supported memory index".to_string())
        })?;
        let suffix = inner.records.get(start..).unwrap_or(&[]);
        Ok(suffix.iter().take(limit).cloned().collect())
    }
}

/// State projection folded from journal events.
///
/// `apply` must be total over the events this reducer's domain produces; see
/// the module invariants.
pub trait EventReducer: Default + Serialize + DeserializeOwned + Send + Sync + 'static {
    type Event: JournalEvent;

    fn apply(&mut self, event: &Self::Event);

    /// Fold an authoritative journal record, including its assigned sequence.
    ///
    /// Reducers that only need the payload inherit this default. Sequence-aware
    /// projections override the hook instead of preallocating a sequence into
    /// the event before append.
    fn apply_record(&mut self, record: &JournalRecord<Self::Event>) {
        self.apply(record.event.as_ref());
    }
}

/// Result of automatic checkpoint compounding after an append.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CheckpointApplyStatus {
    /// The configured cadence has not been reached.
    NotDue,
    /// The projection was checkpointed through this append.
    Saved,
    /// The event and projection committed, but the derived checkpoint did not.
    /// Callers must not retry the event; a later append or recovery retries the
    /// discardable checkpoint.
    Degraded { error: String },
}

/// Commit receipt for one journaled projection update.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApplyReceipt {
    /// Identity of the one-record physical batch frame.
    pub batch_id: String,
    /// Sequence committed to the authoritative journal.
    pub sequence: u64,
    /// Outcome of the authoritative journal append.
    pub journal: JournalDurabilityStatus,
    pub commit: JournalBatchCommitStatus,
    /// Outcome of optional checkpoint compounding.
    pub checkpoint: CheckpointApplyStatus,
}

/// Commit receipt for one journaled projection batch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApplyBatchReceipt {
    pub batch_id: String,
    pub first_sequence: u64,
    pub last_sequence: u64,
    pub record_count: u64,
    pub journal: JournalDurabilityStatus,
    pub commit: JournalBatchCommitStatus,
    pub checkpoint: CheckpointApplyStatus,
}

/// How recovery obtained and repaired its derived checkpoint.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CheckpointRecoveryStatus {
    /// A valid checkpoint was loaded and no repair was needed.
    Loaded,
    /// No checkpoint existed and the replay tail did not reach the cadence.
    Missing,
    /// A missing, corrupt, stale, or behind checkpoint was rebuilt from the
    /// authoritative journal.
    Rebuilt { reason: String },
    /// Journal recovery succeeded, but repairing the derived checkpoint failed.
    Degraded { reason: String, error: String },
}

/// Recovery result for a checkpointed projection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecoveryReceipt {
    /// Last journal sequence folded into the recovered state.
    pub last_applied_sequence: u64,
    /// Checkpoint load/repair outcome.
    pub checkpoint: CheckpointRecoveryStatus,
}

/// Persisted checkpoint pairing a reducer state with its applied sequence.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CheckpointFrame<S> {
    /// Journal sequence covered by this state snapshot.
    pub sequence: u64,
    /// Reducer state at that sequence.
    pub state: S,
}

/// Storage for [`CheckpointFrame`]s written by [`CheckpointedReducer`].
pub trait CheckpointStore<S>: Send + Sync {
    fn save(&self, state: &S, through_sequence: u64) -> Result<()>;

    fn load(&self) -> Result<Option<CheckpointFrame<S>>>;
}

#[derive(Debug)]
struct StoredCheckpoint {
    sequence: u64,
    state: serde_json::Value,
}

/// In-memory [`CheckpointStore`] for tests and ephemeral consumers.
///
/// The state is stored in its serialized form so projections never need to be
/// `Clone` just to be checkpointed.
#[derive(Debug)]
pub struct MemoryCheckpointStore<S> {
    latest: Mutex<Option<StoredCheckpoint>>,
    _state: PhantomData<S>,
}

impl<S> Default for MemoryCheckpointStore<S> {
    fn default() -> Self {
        Self {
            latest: Mutex::new(None),
            _state: PhantomData,
        }
    }
}

impl<S> MemoryCheckpointStore<S> {
    pub fn new() -> Self {
        Self::default()
    }
}

impl<S: Serialize + DeserializeOwned + Send + Sync + 'static> CheckpointStore<S>
    for MemoryCheckpointStore<S>
{
    fn save(&self, state: &S, through_sequence: u64) -> Result<()> {
        let encoded = serde_json::to_value(state).map_err(|error| {
            ReactError::Other(format!("failed to encode checkpoint state: {error}"))
        })?;
        *self
            .latest
            .lock()
            .unwrap_or_else(|error| error.into_inner()) = Some(StoredCheckpoint {
            sequence: through_sequence,
            state: encoded,
        });
        Ok(())
    }

    fn load(&self) -> Result<Option<CheckpointFrame<S>>> {
        let latest = self
            .latest
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        match latest.as_ref() {
            None => Ok(None),
            Some(stored) => {
                let state = serde_json::from_value(stored.state.clone()).map_err(|error| {
                    ReactError::Other(format!("failed to decode checkpoint state: {error}"))
                })?;
                Ok(Some(CheckpointFrame {
                    sequence: stored.sequence,
                    state,
                }))
            }
        }
    }
}

struct ReducerInner<R> {
    state: R,
    last_applied: u64,
    since_checkpoint: u64,
}

/// Maximum records materialized by one recovery replay operation.
const RECOVER_BATCH: usize = 512;

/// Folds journal events into a reducer state and compounds checkpoints.
///
/// `apply` appends to the journal first (durability before projection), then
/// folds all returned records in order. When a batch crosses
/// `checkpoint_every`, the current state is saved once at the batch boundary so
/// later recovery replays only the tail. `checkpoint_every == 0` disables automatic compounding; call
/// [`Self::checkpoint`] manually instead.
pub struct CheckpointedReducer<J, R> {
    journal: Arc<J>,
    checkpoints: Arc<dyn CheckpointStore<R>>,
    checkpoint_every: u64,
    inner: Mutex<ReducerInner<R>>,
}

impl<J, R> CheckpointedReducer<J, R>
where
    R: EventReducer,
    J: EventJournal<R::Event>,
{
    pub fn new(
        journal: Arc<J>,
        checkpoints: Arc<dyn CheckpointStore<R>>,
        checkpoint_every: u64,
    ) -> Self {
        Self {
            journal,
            checkpoints,
            checkpoint_every,
            inner: Mutex::new(ReducerInner {
                state: R::default(),
                last_applied: 0,
                since_checkpoint: 0,
            }),
        }
    }

    /// Append a non-empty batch, fold committed records in order, and compound
    /// at most one checkpoint at the batch boundary.
    pub fn apply_batch(
        &self,
        batch: PreparedJournalBatch<R::Event>,
    ) -> std::result::Result<ApplyBatchReceipt, CheckpointedApplyError<R::Event>> {
        let expected = batch.clone();
        let appended = self
            .journal
            .append_batch(batch)
            .map_err(CheckpointedApplyError::Journal)?;
        self.apply_committed(&expected, appended)
    }

    /// Fold a batch receipt that was committed by an external physical
    /// authority. This keeps projection ownership in the framework while
    /// allowing an application to supply a journal with custom reopen and
    /// durability handling.
    pub fn apply_committed(
        &self,
        expected: &PreparedJournalBatch<R::Event>,
        appended: JournalBatchAppendReceipt<R::Event>,
    ) -> std::result::Result<ApplyBatchReceipt, CheckpointedApplyError<R::Event>> {
        let mut inner = self.inner.lock().unwrap_or_else(|error| error.into_inner());
        let batch_id = appended.batch_id().to_string();
        let receipt_matches = expected.matches_receipt(&appended).map_err(|error| {
            CheckpointedApplyError::CommittedInvariant {
                batch_id: batch_id.clone(),
                error: error.to_string(),
            }
        })?;
        if !receipt_matches {
            return Err(CheckpointedApplyError::CommittedInvariant {
                batch_id,
                error: "journal receipt does not match the prepared batch".to_string(),
            });
        }
        let records = appended.records();
        let record_count = u64::try_from(records.len()).map_err(|_| {
            CheckpointedApplyError::CommittedInvariant {
                batch_id: batch_id.clone(),
                error: "record count exceeds supported range".to_string(),
            }
        })?;
        let first_sequence = records
            .first()
            .map(|record| record.sequence)
            .ok_or_else(|| CheckpointedApplyError::CommittedInvariant {
                batch_id: batch_id.clone(),
                error: "receipt contains no records".to_string(),
            })?;
        let last_sequence = records
            .last()
            .map(|record| record.sequence)
            .ok_or_else(|| CheckpointedApplyError::CommittedInvariant {
                batch_id: batch_id.clone(),
                error: "receipt contains no records".to_string(),
            })?;
        if first_sequence == 0 {
            return Err(CheckpointedApplyError::CommittedInvariant {
                batch_id,
                error: "receipt sequence must be positive".to_string(),
            });
        }
        let mut expected_sequence = first_sequence;
        for record in records {
            if record.batch_id() != appended.batch_id() || record.sequence != expected_sequence {
                return Err(CheckpointedApplyError::CommittedInvariant {
                    batch_id: batch_id.clone(),
                    error: format!(
                        "receipt is not one contiguous batch at sequence {expected_sequence}"
                    ),
                });
            }
            expected_sequence = expected_sequence.checked_add(1).ok_or_else(|| {
                CheckpointedApplyError::CommittedInvariant {
                    batch_id: batch_id.clone(),
                    error: "receipt sequence exhausted".to_string(),
                }
            })?;
        }
        if last_sequence <= inner.last_applied
            && appended.commit_status() != JournalBatchCommitStatus::AlreadyCommitted
        {
            return Err(CheckpointedApplyError::CommittedInvariant {
                batch_id: batch_id.clone(),
                error: "new commit is entirely behind the applied projection".to_string(),
            });
        }
        let first_unapplied = records
            .iter()
            .position(|record| record.sequence > inner.last_applied);
        let folded_count = match first_unapplied {
            None => 0,
            Some(index) => {
                let required = inner.last_applied.checked_add(1).ok_or_else(|| {
                    CheckpointedApplyError::CommittedInvariant {
                        batch_id: batch_id.clone(),
                        error: "projection sequence exhausted before receipt suffix".to_string(),
                    }
                })?;
                let first_suffix = records.get(index).ok_or_else(|| {
                    CheckpointedApplyError::CommittedInvariant {
                        batch_id: batch_id.clone(),
                        error: "receipt suffix index is missing".to_string(),
                    }
                })?;
                if first_suffix.sequence != required {
                    return Err(CheckpointedApplyError::CommittedInvariant {
                        batch_id: batch_id.clone(),
                        error: format!(
                            "projection gap: expected sequence {required} but receipt suffix starts at {}",
                            first_suffix.sequence
                        ),
                    });
                }
                if index > 0
                    && appended.commit_status() != JournalBatchCommitStatus::AlreadyCommitted
                {
                    return Err(CheckpointedApplyError::CommittedInvariant {
                        batch_id: batch_id.clone(),
                        error: "new commit illegally overlaps the applied projection".to_string(),
                    });
                }
                let mut folded = 0_u64;
                for record in records.iter().skip(index) {
                    inner.state.apply_record(record);
                    inner.last_applied = record.sequence;
                    folded = folded.saturating_add(1);
                }
                folded
            }
        };
        inner.since_checkpoint = inner.since_checkpoint.saturating_add(folded_count);
        let checkpoint = if folded_count != 0
            && self.checkpoint_every != 0
            && inner.since_checkpoint >= self.checkpoint_every
        {
            match self.checkpoints.save(&inner.state, inner.last_applied) {
                Ok(()) => {
                    inner.since_checkpoint = 0;
                    CheckpointApplyStatus::Saved
                }
                Err(error) => CheckpointApplyStatus::Degraded {
                    error: error.to_string(),
                },
            }
        } else {
            CheckpointApplyStatus::NotDue
        };
        Ok(ApplyBatchReceipt {
            batch_id,
            first_sequence,
            last_sequence,
            record_count,
            journal: appended.durability().clone(),
            commit: appended.commit_status(),
            checkpoint,
        })
    }

    /// Append one event through the batch authority, fold it, and maybe
    /// compound a checkpoint.
    pub fn apply(
        &self,
        event: R::Event,
    ) -> std::result::Result<ApplyReceipt, CheckpointedApplyError<R::Event>> {
        let batch =
            PreparedJournalBatch::new(vec![event]).map_err(CheckpointedApplyError::Prepare)?;
        let receipt = self.apply_batch(batch)?;
        Ok(ApplyReceipt {
            batch_id: receipt.batch_id,
            sequence: receipt.first_sequence,
            journal: receipt.journal,
            commit: receipt.commit,
            checkpoint: receipt.checkpoint,
        })
    }

    /// Persist the current state as a checkpoint through the applied sequence.
    pub fn checkpoint(&self) -> Result<()> {
        let mut inner = self.inner.lock().unwrap_or_else(|error| error.into_inner());
        self.checkpoints.save(&inner.state, inner.last_applied)?;
        inner.since_checkpoint = 0;
        Ok(())
    }

    /// Load the latest checkpoint (or a default state) and replay the tail.
    ///
    /// Returns the last applied sequence and checkpoint repair status.
    pub fn recover(&self) -> Result<RecoveryReceipt> {
        let mut inner = self.inner.lock().unwrap_or_else(|error| error.into_inner());
        let journal_last = self.journal.last_sequence();
        let retained_floor = self.journal.retained_floor();
        let required_checkpoint = retained_floor.saturating_sub(1);
        let loaded = self.checkpoints.load();
        let (mut state, mut last_applied, mut checkpoint_sequence, mut repair_reason) = match loaded
        {
            Ok(Some(frame)) if frame.sequence < required_checkpoint => {
                return Err(ReactError::Other(format!(
                    "checkpoint sequence {} is behind retained journal floor {retained_floor}; expected at least {required_checkpoint}",
                    frame.sequence
                )));
            }
            Ok(Some(frame)) if retained_floor > 1 && frame.sequence > journal_last => {
                return Err(ReactError::Other(format!(
                    "checkpoint sequence {} is ahead of journal sequence {journal_last} after prefix pruning",
                    frame.sequence
                )));
            }
            Ok(Some(frame)) if frame.sequence <= journal_last => {
                (frame.state, frame.sequence, frame.sequence, None)
            }
            Ok(Some(frame)) => (
                R::default(),
                0,
                0,
                Some(format!(
                    "checkpoint sequence {} is ahead of journal sequence {journal_last}",
                    frame.sequence
                )),
            ),
            Ok(None) if retained_floor > 1 => {
                return Err(ReactError::Other(format!(
                    "checkpoint is required through sequence {required_checkpoint} because journal retention starts at {retained_floor}"
                )));
            }
            Ok(None) => (R::default(), 0, 0, None),
            Err(error) if retained_floor > 1 => {
                return Err(ReactError::Other(format!(
                    "checkpoint load failed for retained journal floor {retained_floor}: {error}"
                )));
            }
            Err(error) => (
                R::default(),
                0,
                0,
                Some(format!("checkpoint load failed: {error}")),
            ),
        };
        loop {
            let records = self.journal.replay_after(last_applied, RECOVER_BATCH)?;
            if records.is_empty() {
                break;
            }
            for record in records {
                state.apply_record(&record);
                last_applied = record.sequence;
            }
        }
        let replayed_since_checkpoint = last_applied.saturating_sub(checkpoint_sequence);
        let checkpoint_due =
            self.checkpoint_every != 0 && replayed_since_checkpoint >= self.checkpoint_every;
        let checkpoint = if repair_reason.is_some() || checkpoint_due {
            let reason = repair_reason.take().unwrap_or_else(|| {
                format!(
                    "replayed {replayed_since_checkpoint} events after checkpoint sequence {checkpoint_sequence}"
                )
            });
            match self.checkpoints.save(&state, last_applied) {
                Ok(()) => {
                    checkpoint_sequence = last_applied;
                    CheckpointRecoveryStatus::Rebuilt { reason }
                }
                Err(error) => CheckpointRecoveryStatus::Degraded {
                    reason,
                    error: error.to_string(),
                },
            }
        } else if checkpoint_sequence == 0 {
            CheckpointRecoveryStatus::Missing
        } else {
            CheckpointRecoveryStatus::Loaded
        };
        inner.state = state;
        inner.last_applied = last_applied;
        inner.since_checkpoint = last_applied.saturating_sub(checkpoint_sequence);
        Ok(RecoveryReceipt {
            last_applied_sequence: last_applied,
            checkpoint,
        })
    }

    /// Last journal sequence folded into the current state.
    pub fn last_applied_sequence(&self) -> u64 {
        self.inner
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .last_applied
    }

    /// Read the projected state under the reducer lock.
    pub fn with_state<T>(&self, f: impl FnOnce(&R) -> T) -> T {
        let inner = self.inner.lock().unwrap_or_else(|error| error.into_inner());
        f(&inner.state)
    }

    /// Mutably inspect the in-memory projection without bypassing the reducer lock.
    pub fn with_state_mut<T>(&self, f: impl FnOnce(&mut R) -> T) -> T {
        let mut inner = self.inner.lock().unwrap_or_else(|error| error.into_inner());
        f(&mut inner.state)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Barrier;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[test]
    fn weak_registry_prunes_on_soft_cadence_and_immediate_hard_limit() {
        let root = PathBuf::from("registry-test");
        let mut registry = WeakRegistry::<()>::new();
        for index in 0..WEAK_REGISTRY_PRUNE_THRESHOLD {
            let authority = Arc::new(());
            registry.insert(root.join(format!("soft-{index}")), &authority);
        }
        for _ in 1..WEAK_REGISTRY_PRUNE_INTERVAL {
            registry.prune_dead_if_due();
        }
        assert_eq!(registry.paths_beneath(&root), WEAK_REGISTRY_PRUNE_THRESHOLD);
        registry.prune_dead_if_due();
        assert_eq!(registry.paths_beneath(&root), 0);

        for index in 0..WEAK_REGISTRY_HARD_LIMIT {
            let authority = Arc::new(());
            registry.insert(root.join(format!("hard-{index}")), &authority);
        }
        registry.prune_dead_if_due();
        assert_eq!(registry.paths_beneath(&root), 0);

        let mut live = Vec::new();
        for index in 0..WEAK_REGISTRY_HARD_LIMIT.saturating_add(8) {
            let authority = Arc::new(());
            registry.insert(root.join(format!("live-{index}")), &authority);
            live.push(authority);
        }
        registry.prune_dead_if_due();
        assert_eq!(
            registry.paths_beneath(&root),
            WEAK_REGISTRY_HARD_LIMIT.saturating_add(8)
        );
        drop(live);
        registry.prune_dead_if_due();
        assert_eq!(registry.paths_beneath(&root), 0);
    }

    #[derive(Default, Serialize, Deserialize, Debug)]
    struct SumReducer {
        total: i64,
        events: Vec<i32>,
    }

    #[derive(Debug, Deserialize)]
    struct FailingEvent;

    impl Serialize for FailingEvent {
        fn serialize<S>(&self, _serializer: S) -> std::result::Result<S::Ok, S::Error>
        where
            S: serde::Serializer,
        {
            Err(<S::Error as serde::ser::Error>::custom(
                "injected event serialization failure",
            ))
        }
    }

    #[derive(Debug)]
    struct MutableEvent {
        value: AtomicUsize,
    }

    impl Serialize for MutableEvent {
        fn serialize<S>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error>
        where
            S: serde::Serializer,
        {
            serializer
                .serialize_u64(u64::try_from(self.value.load(Ordering::SeqCst)).unwrap_or(u64::MAX))
        }
    }

    impl<'de> Deserialize<'de> for MutableEvent {
        fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
        where
            D: serde::Deserializer<'de>,
        {
            let value = usize::deserialize(deserializer)?;
            Ok(Self {
                value: AtomicUsize::new(value),
            })
        }
    }

    impl EventReducer for SumReducer {
        type Event = i32;

        fn apply(&mut self, event: &i32) {
            self.total = self.total.saturating_add(i64::from(*event));
            self.events.push(*event);
        }
    }

    type SumHarness = CheckpointedReducer<MemoryEventJournal<i32>, SumReducer>;

    struct SumFixture {
        reducer: SumHarness,
        journal: Arc<MemoryEventJournal<i32>>,
        checkpoints: Arc<MemoryCheckpointStore<SumReducer>>,
    }

    struct TrackingJournal {
        inner: MemoryEventJournal<i32>,
        replay_limits: Mutex<Vec<usize>>,
    }

    impl TrackingJournal {
        fn new() -> Self {
            Self {
                inner: MemoryEventJournal::new(),
                replay_limits: Mutex::new(Vec::new()),
            }
        }
    }

    impl EventJournal<i32> for TrackingJournal {
        fn append_batch(&self, batch: PreparedJournalBatch<i32>) -> JournalBatchAppendResult<i32> {
            self.inner.append_batch(batch)
        }

        fn lookup_batch(
            &self,
            batch: &PreparedJournalBatch<i32>,
        ) -> Result<JournalBatchLookup<i32>> {
            self.inner.lookup_batch(batch)
        }

        fn next_sequence(&self) -> u64 {
            self.inner.next_sequence()
        }

        fn last_sequence(&self) -> u64 {
            self.inner.last_sequence()
        }

        fn replay_after(
            &self,
            after_sequence: u64,
            limit: usize,
        ) -> Result<Vec<JournalRecord<i32>>> {
            self.replay_limits
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .push(limit);
            self.inner.replay_after(after_sequence, limit)
        }
    }

    fn reducer_with(checkpoint_every: u64) -> SumFixture {
        let journal = Arc::new(MemoryEventJournal::new());
        let checkpoints = Arc::new(MemoryCheckpointStore::new());
        let reducer = CheckpointedReducer::new(
            Arc::clone(&journal),
            Arc::clone(&checkpoints) as Arc<dyn CheckpointStore<SumReducer>>,
            checkpoint_every,
        );
        SumFixture {
            reducer,
            journal,
            checkpoints,
        }
    }

    fn batch<E: JournalEvent>(events: Vec<E>) -> TestResult<PreparedJournalBatch<E>> {
        PreparedJournalBatch::new(events).test_context("prepare test batch")
    }

    #[test]
    fn memory_journal_assigns_contiguous_sequences_from_one() {
        let journal = MemoryEventJournal::new();
        assert_eq!(journal.last_sequence(), 0);
        assert_eq!(journal.next_sequence(), 1);
        let first = journal.append(10).expect("append");
        let second = journal.append(20).expect("append");
        assert_eq!(first.record.sequence, 1);
        assert_eq!(second.record.sequence, 2);
        assert_eq!(first.durability, JournalDurabilityStatus::Confirmed);
        assert_eq!(second.durability, JournalDurabilityStatus::Confirmed);
        assert_eq!(journal.last_sequence(), 2);
        assert_eq!(journal.next_sequence(), 3);
    }

    #[test]
    fn memory_batch_is_one_ordered_commit_and_empty_is_zero_write() -> TestResult {
        let journal = MemoryEventJournal::new();
        let Err(empty) = PreparedJournalBatch::new(Vec::<i32>::new()) else {
            return Err(test_failure("empty batch unexpectedly passed preflight"));
        };
        assert!(empty.error.contains("at least one"));
        assert_eq!(journal.next_sequence(), 1);

        let receipt = journal
            .append_batch(batch(vec![10, 20, 30])?)
            .test_context("append memory batch")?;
        assert!(uuid::Uuid::parse_str(&receipt.batch_id).is_ok());
        assert_eq!(receipt.records.len(), 3);
        assert_eq!(
            receipt
                .records
                .iter()
                .map(|record| record.sequence)
                .collect::<Vec<_>>(),
            vec![1, 2, 3]
        );
        let replay = journal
            .replay_after(0, usize::MAX)
            .test_context("replay batch")?;
        assert_eq!(replay.len(), 3);
        assert_eq!(
            journal
                .replay_after(1, 1)
                .test_context("replay within batch")?
                .first()
                .map(|record| *record.event),
            Some(20)
        );
        assert!(
            receipt
                .records
                .iter()
                .zip(replay.iter())
                .all(|(committed, replayed)| Arc::ptr_eq(&committed.event, &replayed.event))
        );
        Ok(())
    }

    #[test]
    fn memory_batch_identity_is_idempotent_and_conflicts_poison() -> TestResult {
        let journal = MemoryEventJournal::new();
        let original = batch(vec![10, 20])?;
        let batch_id = original.batch_id().to_string();
        let committed = journal
            .append_batch(original)
            .test_context("initial commit")?;
        assert_eq!(
            committed.commit_status(),
            JournalBatchCommitStatus::Committed
        );

        let duplicate = PreparedJournalBatch::with_test_identity(batch_id.clone(), vec![10, 20])
            .test_context("same identity payload")?;
        let idempotent = journal
            .append_batch(duplicate)
            .test_context("idempotent append")?;
        assert_eq!(
            idempotent.commit_status(),
            JournalBatchCommitStatus::AlreadyCommitted
        );
        assert_eq!(
            idempotent
                .records()
                .iter()
                .map(|record| record.sequence)
                .collect::<Vec<_>>(),
            vec![1, 2]
        );
        assert_eq!(journal.last_sequence(), 2);

        let conflict = PreparedJournalBatch::with_test_identity(batch_id, vec![10, 99])
            .test_context("conflicting identity payload")?;
        let Err(error) = journal.append_batch(conflict) else {
            return Err(test_failure("identity conflict unexpectedly committed"));
        };
        assert!(error.to_string().contains("conflicts"));
        assert!(!error.is_retry_safe());
        let Err(refused) = journal.append_batch(batch(vec![30])?) else {
            return Err(test_failure(
                "poisoned memory authority unexpectedly accepted a retry",
            ));
        };
        assert!(refused.to_string().contains("poisoned"));
        assert!(!refused.is_retry_safe());
        assert!(refused.requires_reopen());
        Ok(())
    }

    #[test]
    fn memory_rejects_interior_mutation_before_idempotent_lookup() -> TestResult {
        let journal = MemoryEventJournal::new();
        let original = batch(vec![MutableEvent {
            value: AtomicUsize::new(1),
        }])?;
        let batch_id = original.batch_id().to_string();
        journal
            .append_batch(original)
            .test_context("commit original payload")?;

        let mutated = PreparedJournalBatch::with_test_identity(
            batch_id.clone(),
            vec![MutableEvent {
                value: AtomicUsize::new(1),
            }],
        )
        .test_context("prepare mutable duplicate")?;
        if let Some(event) = mutated.events().first() {
            event.value.store(2, Ordering::SeqCst);
        }
        assert!(matches!(
            journal
                .lookup_batch(&mutated)
                .test_context("lookup mutated payload")?,
            JournalBatchLookup::Conflict { .. }
        ));
        let Err(mutation) = journal.append_batch(mutated) else {
            return Err(test_failure(
                "mutated prepared payload unexpectedly matched the committed batch",
            ));
        };
        assert!(matches!(
            mutation,
            JournalBatchAppendError::PreparedMutation { .. }
        ));
        assert!(!mutation.is_retry_safe());
        assert_eq!(journal.last_sequence(), 1);

        let original_lookup = PreparedJournalBatch::with_test_identity(
            batch_id,
            vec![MutableEvent {
                value: AtomicUsize::new(1),
            }],
        )
        .test_context("prepare original digest lookup")?;
        assert!(matches!(
            journal
                .lookup_batch(&original_lookup)
                .test_context("lookup original payload")?,
            JournalBatchLookup::AlreadyCommitted(_)
        ));
        Ok(())
    }

    #[test]
    fn memory_batch_preflight_rejects_sequence_overflow_without_mutation() -> TestResult {
        let journal = MemoryEventJournal::new();
        journal
            .inner
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .next_sequence = u64::MAX;
        let Err(error) = journal.append_batch(batch(vec![1])?) else {
            return Err(test_failure("sequence overflow unexpectedly committed"));
        };
        assert!(matches!(
            error,
            JournalBatchAppendError::NotCommitted { .. }
        ));
        assert_eq!(journal.next_sequence(), u64::MAX);
        assert!(
            journal
                .replay_after(0, usize::MAX)
                .test_context("replay")?
                .is_empty()
        );
        Ok(())
    }

    #[test]
    fn memory_batch_serialization_failure_is_not_committed() -> TestResult {
        let journal = MemoryEventJournal::<FailingEvent>::new();
        let Err(error) = PreparedJournalBatch::new(vec![FailingEvent]) else {
            return Err(test_failure(
                "serialization failure unexpectedly produced a prepared batch",
            ));
        };
        assert!(error.error.contains("serialization failure"));
        assert_eq!(journal.next_sequence(), 1);
        assert!(
            journal
                .replay_after(0, usize::MAX)
                .test_context("replay")?
                .is_empty()
        );
        Ok(())
    }

    #[test]
    fn replay_after_respects_boundary_and_limit() {
        let journal = MemoryEventJournal::new();
        for value in 0..10 {
            journal.append(value).expect("append");
        }
        let records = journal.replay_after(7, 2).expect("replay");
        assert_eq!(
            records.iter().map(|r| r.sequence).collect::<Vec<_>>(),
            vec![8, 9]
        );
        let rest = journal.replay_after(7, usize::MAX).expect("replay");
        assert_eq!(rest.len(), 3);
        let all = journal.replay_after(0, usize::MAX).expect("replay");
        assert_eq!(all.len(), 10);
    }

    #[test]
    fn apply_folds_state_and_returns_sequence() {
        let fixture = reducer_with(0);
        assert_eq!(fixture.reducer.apply(5).expect("apply").sequence, 1);
        assert_eq!(fixture.reducer.apply(7).expect("apply").sequence, 2);
        assert_eq!(fixture.journal.last_sequence(), 2);
        fixture.reducer.with_state(|state| {
            assert_eq!(state.total, 12);
            assert_eq!(state.events, vec![5, 7]);
        });
    }

    #[test]
    fn automatic_checkpoint_fires_on_cadence() {
        let fixture = reducer_with(2);
        fixture.reducer.apply(1).expect("apply");
        assert!(fixture.checkpoints.load().expect("load").is_none());
        fixture.reducer.apply(2).expect("apply");
        let frame = fixture
            .checkpoints
            .load()
            .expect("load")
            .expect("checkpoint saved");
        assert_eq!(frame.sequence, 2);
        assert_eq!(frame.state.total, 3);
    }

    #[test]
    fn batch_crossing_checkpoint_cadence_folds_then_saves_at_batch_end() -> TestResult {
        let fixture = reducer_with(3);
        fixture.reducer.apply(1).test_context("seed event")?;
        let receipt = fixture
            .reducer
            .apply_batch(batch(vec![2, 3, 4])?)
            .test_context("apply batch")?;
        assert_eq!(receipt.first_sequence, 2);
        assert_eq!(receipt.last_sequence, 4);
        assert_eq!(receipt.record_count, 3);
        assert_eq!(receipt.checkpoint, CheckpointApplyStatus::Saved);
        let frame = fixture
            .checkpoints
            .load()
            .test_context("load checkpoint")?
            .test_context("batch checkpoint")?;
        assert_eq!(frame.sequence, 4);
        assert_eq!(frame.state.events, vec![1, 2, 3, 4]);
        Ok(())
    }

    #[test]
    fn already_committed_partial_overlap_folds_only_contiguous_suffix() -> TestResult {
        let journal = Arc::new(MemoryEventJournal::new());
        let original = batch(vec![1, 2, 3])?;
        let batch_id = original.batch_id().to_string();
        let committed = journal
            .append_batch(original)
            .test_context("commit source batch")?;
        let reducer = CheckpointedReducer::<_, SumReducer>::new(
            Arc::clone(&journal),
            Arc::new(MemoryCheckpointStore::new()),
            0,
        );
        {
            let mut inner = reducer
                .inner
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            if let Some(first) = committed.records().first() {
                inner.state.apply_record(first);
                inner.last_applied = first.sequence;
                inner.since_checkpoint = 1;
            }
        }
        let duplicate = PreparedJournalBatch::with_test_identity(batch_id, vec![1, 2, 3])
            .test_context("prepare idempotent overlap")?;
        let receipt = reducer
            .apply_batch(duplicate)
            .test_context("fold idempotent suffix")?;
        assert_eq!(receipt.commit, JournalBatchCommitStatus::AlreadyCommitted);
        assert_eq!(reducer.last_applied_sequence(), 3);
        reducer.with_state(|state| assert_eq!(state.events, vec![1, 2, 3]));
        Ok(())
    }

    #[test]
    fn recover_replays_from_checkpoint_then_tail() {
        let fixture = reducer_with(2);
        for value in 1..=5 {
            fixture.reducer.apply(value).expect("apply");
        }
        assert_eq!(fixture.reducer.last_applied_sequence(), 5);

        // A second reducer sharing the journal and checkpoint store recovers
        // by loading the checkpoint and replaying only the tail.
        let second: SumHarness = CheckpointedReducer::new(
            fixture.journal,
            fixture.checkpoints as Arc<dyn CheckpointStore<SumReducer>>,
            2,
        );
        assert_eq!(second.recover().expect("recover").last_applied_sequence, 5);
        second.with_state(|state| {
            assert_eq!(state.total, 15);
            assert_eq!(state.events, (1..=5).collect::<Vec<_>>());
        });
    }

    #[test]
    fn recover_without_checkpoint_replays_everything() {
        let fixture = reducer_with(0);
        for value in 1..=4 {
            fixture.reducer.apply(value).expect("apply");
        }
        let second: SumHarness = CheckpointedReducer::new(
            fixture.journal,
            Arc::new(MemoryCheckpointStore::<SumReducer>::new()),
            0,
        );
        assert_eq!(second.recover().expect("recover").last_applied_sequence, 4);
        second.with_state(|state| assert_eq!(state.total, 10));
    }

    #[test]
    fn recovery_replays_large_missing_checkpoint_tail_in_fixed_batches() {
        const EVENTS: i32 = 1_537;
        let journal = Arc::new(TrackingJournal::new());
        for value in 0..EVENTS {
            journal.append(value).expect("append");
        }
        let reducer = CheckpointedReducer::<_, SumReducer>::new(
            Arc::clone(&journal),
            Arc::new(MemoryCheckpointStore::new()),
            0,
        );

        let recovered = reducer.recover().expect("recover");
        assert_eq!(
            recovered.last_applied_sequence,
            u64::try_from(EVENTS).unwrap_or(u64::MAX)
        );
        let limits = journal
            .replay_limits
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        assert!(limits.len() >= 4);
        assert!(limits.iter().all(|limit| *limit == RECOVER_BATCH));
    }

    #[test]
    fn manual_checkpoint_round_trips_through_store() {
        let fixture = reducer_with(0);
        for value in 1..=3 {
            fixture.reducer.apply(value).expect("apply");
        }
        fixture.reducer.checkpoint().expect("checkpoint");
        let frame = fixture
            .checkpoints
            .load()
            .expect("load")
            .expect("checkpoint present");
        assert_eq!(frame.sequence, 3);
        assert_eq!(frame.state.total, 6);
    }

    struct FailOnceCheckpointStore {
        remaining_failures: AtomicUsize,
        inner: MemoryCheckpointStore<SumReducer>,
    }

    impl FailOnceCheckpointStore {
        fn new() -> Self {
            Self {
                remaining_failures: AtomicUsize::new(1),
                inner: MemoryCheckpointStore::new(),
            }
        }
    }

    impl CheckpointStore<SumReducer> for FailOnceCheckpointStore {
        fn save(&self, state: &SumReducer, through_sequence: u64) -> Result<()> {
            if self
                .remaining_failures
                .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |remaining| {
                    remaining.checked_sub(1)
                })
                .is_ok()
            {
                return Err(ReactError::Other("injected checkpoint failure".to_string()));
            }
            self.inner.save(state, through_sequence)
        }

        fn load(&self) -> Result<Option<CheckpointFrame<SumReducer>>> {
            self.inner.load()
        }
    }

    #[test]
    fn checkpoint_failure_returns_committed_batch_with_degraded_checkpoint() -> TestResult {
        let journal = Arc::new(MemoryEventJournal::new());
        let checkpoints = Arc::new(FailOnceCheckpointStore::new());
        let reducer = CheckpointedReducer::new(
            Arc::clone(&journal),
            Arc::clone(&checkpoints) as Arc<dyn CheckpointStore<SumReducer>>,
            2,
        );

        let first = reducer
            .apply_batch(batch(vec![7, 8, 9])?)
            .test_context("event batch must commit")?;
        assert_eq!(first.first_sequence, 1);
        assert_eq!(first.last_sequence, 3);
        assert!(matches!(
            first.checkpoint,
            CheckpointApplyStatus::Degraded { .. }
        ));
        assert_eq!(journal.last_sequence(), 3);
        reducer.with_state(|state| assert_eq!(state.events, vec![7, 8, 9]));

        let second = reducer
            .apply(10)
            .test_context("later append retries checkpoint")?;
        assert_eq!(second.sequence, 4);
        assert_eq!(second.checkpoint, CheckpointApplyStatus::Saved);
        let frame = checkpoints
            .load()
            .test_context("checkpoint load")?
            .test_context("checkpoint repaired")?;
        assert_eq!(frame.sequence, 4);
        assert_eq!(frame.state.events, vec![7, 8, 9, 10]);
        Ok(())
    }

    #[test]
    fn concurrent_apply_preserves_journal_order_in_projection() {
        const THREADS: usize = 32;
        let journal = Arc::new(MemoryEventJournal::new());
        let reducer = Arc::new(CheckpointedReducer::<_, SumReducer>::new(
            Arc::clone(&journal),
            Arc::new(MemoryCheckpointStore::new()),
            0,
        ));
        let barrier = Arc::new(Barrier::new(THREADS));
        let mut handles = Vec::new();
        for value in 0..THREADS {
            let reducer = Arc::clone(&reducer);
            let barrier = Arc::clone(&barrier);
            handles.push(std::thread::spawn(move || {
                barrier.wait();
                let value = i32::try_from(value).unwrap_or(i32::MAX);
                reducer.apply(value).map(|receipt| receipt.sequence)
            }));
        }
        for handle in handles {
            assert!(handle.join().is_ok_and(|result| result.is_ok()));
        }

        let journal_events = journal
            .replay_after(0, usize::MAX)
            .expect("journal replay")
            .into_iter()
            .map(|record| *record.event)
            .collect::<Vec<_>>();
        reducer.with_state(|state| assert_eq!(state.events, journal_events));
        assert_eq!(
            reducer.last_applied_sequence(),
            u64::try_from(THREADS).unwrap_or(u64::MAX)
        );
    }

    #[test]
    fn concurrent_memory_batches_never_interleave_records() -> TestResult {
        const THREADS: usize = 16;
        let journal = Arc::new(MemoryEventJournal::new());
        let barrier = Arc::new(Barrier::new(THREADS));
        let mut handles = Vec::new();
        for index in 0..THREADS {
            let journal = Arc::clone(&journal);
            let barrier = Arc::clone(&barrier);
            handles.push(std::thread::spawn(move || -> TestResult<_> {
                barrier.wait();
                let base = i32::try_from(index).unwrap_or(i32::MAX).saturating_mul(10);
                journal
                    .append_batch(batch(vec![
                        base,
                        base.saturating_add(1),
                        base.saturating_add(2),
                    ])?)
                    .test_context("append concurrent memory batch")
            }));
        }
        let mut receipts = Vec::new();
        for handle in handles {
            let receipt = handle.join().test_context("join concurrent batch")??;
            receipts.push(receipt);
        }
        let replay = journal.replay_after(0, usize::MAX).test_context("replay")?;
        for receipt in receipts {
            let first = receipt
                .records
                .first()
                .map(|record| record.sequence)
                .test_context("batch first sequence")?;
            let record_count = u64::try_from(receipt.records.len()).unwrap_or(u64::MAX);
            let values = replay
                .iter()
                .filter(|record| {
                    record.sequence >= first && record.sequence < first.saturating_add(record_count)
                })
                .map(|record| *record.event)
                .collect::<Vec<_>>();
            assert_eq!(values.len(), 3);
            assert!(values.windows(2).all(|pair| {
                pair.first()
                    .zip(pair.get(1))
                    .is_some_and(|(left, right)| left.saturating_add(1) == *right)
            }));
        }
        Ok(())
    }

    #[test]
    fn checkpoint_ahead_of_journal_is_rebuilt_from_authoritative_events() {
        let journal = Arc::new(MemoryEventJournal::new());
        journal.append(3).expect("append");
        let checkpoints = Arc::new(MemoryCheckpointStore::new());
        checkpoints
            .save(
                &SumReducer {
                    total: 999,
                    events: vec![999],
                },
                99,
            )
            .expect("seed invalid checkpoint");
        let reducer = CheckpointedReducer::new(
            journal,
            Arc::clone(&checkpoints) as Arc<dyn CheckpointStore<SumReducer>>,
            10,
        );

        let receipt = reducer.recover().expect("recover from journal");
        assert_eq!(receipt.last_applied_sequence, 1);
        assert!(matches!(
            receipt.checkpoint,
            CheckpointRecoveryStatus::Rebuilt { .. }
        ));
        reducer.with_state(|state| {
            assert_eq!(state.total, 3);
            assert_eq!(state.events, vec![3]);
        });
        assert_eq!(
            checkpoints
                .load()
                .expect("load")
                .expect("repaired checkpoint")
                .sequence,
            1
        );
    }

    #[derive(Default, Serialize, Deserialize)]
    struct SequenceAwareReducer {
        sequences: Vec<u64>,
    }

    impl EventReducer for SequenceAwareReducer {
        type Event = String;

        fn apply(&mut self, _event: &String) {}

        fn apply_record(&mut self, record: &JournalRecord<Self::Event>) {
            self.sequences.push(record.sequence);
        }
    }

    #[test]
    fn reducer_receives_authoritative_sequence_on_apply_and_recovery() {
        let journal = Arc::new(MemoryEventJournal::new());
        let writer = CheckpointedReducer::<_, SequenceAwareReducer>::new(
            Arc::clone(&journal),
            Arc::new(MemoryCheckpointStore::new()),
            0,
        );
        writer.apply("one".to_string()).expect("first apply");
        writer.apply("two".to_string()).expect("second apply");
        writer.with_state(|state| assert_eq!(state.sequences, vec![1, 2]));

        let recovered = CheckpointedReducer::<_, SequenceAwareReducer>::new(
            journal,
            Arc::new(MemoryCheckpointStore::new()),
            0,
        );
        recovered.recover().expect("recover");
        recovered.with_state(|state| assert_eq!(state.sequences, vec![1, 2]));
    }
}
