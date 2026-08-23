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
//! - Apache Kafka's log storage: immutable segment files divide physical I/O
//!   while records retain one global ordered offset space.
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
//! - A torn trailing record (partial write followed by a crash) is tolerated
//!   and truncated on the next open. Corruption before the trailing record is
//!   an error.
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
//!     MemoryEventJournal,
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
//! reducer.apply("one".to_string())?;
//! reducer.apply("two".to_string())?; // checkpoint compounding fires here
//! reducer.apply("three".to_string())?;
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
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::marker::PhantomData;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, Weak};

// File-backed runtime caches normally keep far fewer authorities live. Above
// this headroom, scan at a fixed cadence so dead paths stay bounded without an
// O(n) registry walk on every open. This is lifecycle hygiene, not an eviction
// policy: live authorities are never removed.
pub(super) const WEAK_REGISTRY_PRUNE_THRESHOLD: usize = 128;
pub(super) const WEAK_REGISTRY_PRUNE_INTERVAL: usize = 32;

pub(super) struct WeakRegistry<T> {
    entries: HashMap<PathBuf, Weak<T>>,
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
        if self.entries.len() < WEAK_REGISTRY_PRUNE_THRESHOLD
            || self.opens_since_prune < WEAK_REGISTRY_PRUNE_INTERVAL
        {
            return;
        }
        self.entries
            .retain(|_, authority| authority.strong_count() > 0);
        self.opens_since_prune = 0;
    }

    pub(super) fn upgrade(&self, path: &Path) -> Option<Arc<T>> {
        self.entries.get(path).and_then(Weak::upgrade)
    }

    pub(super) fn insert(&mut self, path: PathBuf, authority: &Arc<T>) {
        self.entries.insert(path, Arc::downgrade(authority));
    }

    #[cfg(test)]
    pub(super) fn len(&self) -> usize {
        self.entries.len()
    }

    #[cfg(test)]
    pub(super) fn dead_len(&self) -> usize {
        self.entries
            .values()
            .filter(|authority| authority.strong_count() == 0)
            .count()
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

/// One durable journal entry with its assigned sequence.
#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct JournalRecord<E> {
    /// 1-based contiguous sequence assigned by the journal.
    pub sequence: u64,
    /// The persisted event payload.
    pub event: Arc<E>,
}

impl<E> Clone for JournalRecord<E> {
    fn clone(&self) -> Self {
        Self {
            sequence: self.sequence,
            event: Arc::clone(&self.event),
        }
    }
}

/// Durability result for one record that is present in the journal file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum JournalDurabilityStatus {
    /// The append and requested durability operation both completed.
    Confirmed,
    /// The complete record is present and owns its sequence, but the requested
    /// durability barrier reported an error. Callers must not retry the event.
    Degraded { error: String },
}

/// Receipt for one committed journal append.
#[derive(Debug)]
pub struct JournalAppendReceipt<E> {
    pub record: JournalRecord<E>,
    pub durability: JournalDurabilityStatus,
}

impl<E> Clone for JournalAppendReceipt<E> {
    fn clone(&self) -> Self {
        Self {
            record: self.record.clone(),
            durability: self.durability.clone(),
        }
    }
}

/// Append-only sequenced journal of events.
///
/// Implementations assign contiguous 1-based sequences at append time and can
/// replay any suffix of the journal in order.
pub trait EventJournal<E: JournalEvent>: Send + Sync {
    /// Consume one event and report both its shared owned record and durability
    /// outcome. A [`JournalDurabilityStatus::Degraded`] receipt means the full
    /// record owns its sequence and must not be retried.
    fn append(&self, event: E) -> Result<JournalAppendReceipt<E>>;

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
            }),
        }
    }
}

impl<E: JournalEvent> EventJournal<E> for MemoryEventJournal<E> {
    fn append(&self, event: E) -> Result<JournalAppendReceipt<E>> {
        let mut inner = self.inner.lock().unwrap_or_else(|error| error.into_inner());
        let next_sequence = inner.next_sequence.checked_add(1).ok_or_else(|| {
            ReactError::Other("journal sequence exhausted before append".to_string())
        })?;
        let record = JournalRecord {
            sequence: inner.next_sequence,
            event: Arc::new(event),
        };
        inner.next_sequence = next_sequence;
        inner.records.push(record.clone());
        Ok(JournalAppendReceipt {
            record,
            durability: JournalDurabilityStatus::Confirmed,
        })
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
    /// Sequence committed to the authoritative journal.
    pub sequence: u64,
    /// Outcome of the authoritative journal append.
    pub journal: JournalDurabilityStatus,
    /// Outcome of optional checkpoint compounding.
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
/// folds the returned record. Every `checkpoint_every` applies, the current
/// state is saved through the checkpoint store so later recovery replays only
/// the tail. `checkpoint_every == 0` disables automatic compounding; call
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

    /// Append one event, fold it, and maybe compound a checkpoint.
    ///
    /// Returns the journal sequence assigned to the event.
    pub fn apply(&self, event: R::Event) -> Result<ApplyReceipt> {
        let mut inner = self.inner.lock().unwrap_or_else(|error| error.into_inner());
        let appended = self.journal.append(event)?;
        inner.state.apply_record(&appended.record);
        inner.last_applied = appended.record.sequence;
        inner.since_checkpoint = inner.since_checkpoint.saturating_add(1);
        let checkpoint =
            if self.checkpoint_every != 0 && inner.since_checkpoint >= self.checkpoint_every {
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
        Ok(ApplyReceipt {
            sequence: appended.record.sequence,
            journal: appended.durability,
            checkpoint,
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
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Barrier;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[derive(Default, Serialize, Deserialize, Debug)]
    struct SumReducer {
        total: i64,
        events: Vec<i32>,
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
        fn append(&self, event: i32) -> Result<JournalAppendReceipt<i32>> {
            self.inner.append(event)
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
    fn checkpoint_failure_returns_committed_degraded_receipt_without_retrying_event() {
        let journal = Arc::new(MemoryEventJournal::new());
        let checkpoints = Arc::new(FailOnceCheckpointStore::new());
        let reducer = CheckpointedReducer::new(
            Arc::clone(&journal),
            Arc::clone(&checkpoints) as Arc<dyn CheckpointStore<SumReducer>>,
            1,
        );

        let first = reducer.apply(7).expect("event append must commit");
        assert_eq!(first.sequence, 1);
        assert!(matches!(
            first.checkpoint,
            CheckpointApplyStatus::Degraded { .. }
        ));
        assert_eq!(journal.last_sequence(), 1);
        reducer.with_state(|state| assert_eq!(state.events, vec![7]));

        let second = reducer.apply(9).expect("later append retries checkpoint");
        assert_eq!(second.sequence, 2);
        assert_eq!(second.checkpoint, CheckpointApplyStatus::Saved);
        let frame = checkpoints
            .load()
            .expect("checkpoint load")
            .expect("checkpoint repaired");
        assert_eq!(frame.sequence, 2);
        assert_eq!(frame.state.events, vec![7, 9]);
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
