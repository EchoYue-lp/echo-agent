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
//! - Classic event sourcing snapshots: fold events into state and periodically
//!   persist `(state, applied_sequence)` so recovery replays only the tail.
//!
//! # Layering decision
//!
//! Journal sequencing, durable append, tolerant replay, and
//! checkpoint compounding are generic mechanisms needed by any agent runtime,
//! so they live in the framework (`echo-state`). Directory layout, retention,
//! and product projections stay with consumers. Migrating EKO's duplicated
//! file algorithms onto this module is planned follow-up work and deletes the
//! duplicated algorithms at that point; nothing here depends on it.
//!
//! # Invariants
//!
//! - Sequences are 1-based and contiguous (`1, 2, 3, ...`). A journal load
//!   that observes a gap or a non-monotonic record fails loudly instead of
//!   silently mis-replaying.
//! - Journals are single-writer within a process. Cross-process writers are
//!   out of scope for the local personal-assistant threat model.
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
//! reducer.apply(&"one".to_string())?;
//! reducer.apply(&"two".to_string())?; // checkpoint compounding fires here
//! reducer.apply(&"three".to_string())?;
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

pub use file::{FileCheckpointStore, FileEventJournal};

use echo_core::error::{ReactError, Result};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use std::marker::PhantomData;
use std::sync::{Arc, Mutex};

/// Event payload accepted by an [`EventJournal`].
///
/// A blanket impl covers every `serde`-capable, cloneable, thread-safe type.
pub trait JournalEvent:
    Serialize + DeserializeOwned + Clone + Send + Sync + std::fmt::Debug + 'static
{
}

impl<T> JournalEvent for T where
    T: Serialize + DeserializeOwned + Clone + Send + Sync + std::fmt::Debug + 'static
{
}

/// One durable journal entry with its assigned sequence.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JournalRecord<E> {
    /// 1-based contiguous sequence assigned by the journal.
    pub sequence: u64,
    /// The persisted event payload.
    pub event: E,
}

/// Append-only sequenced journal of events.
///
/// Implementations assign contiguous 1-based sequences at append time and can
/// replay any suffix of the journal in order.
pub trait EventJournal<E: JournalEvent>: Send + Sync {
    /// Durably append one event and return the record with its sequence.
    fn append(&self, event: &E) -> Result<JournalRecord<E>>;

    /// Sequence that the next append will assign.
    fn next_sequence(&self) -> u64;

    /// Last durably persisted sequence (`0` when the journal is empty).
    fn last_sequence(&self) -> u64;

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
    fn append(&self, event: &E) -> Result<JournalRecord<E>> {
        let mut inner = self.inner.lock().unwrap_or_else(|error| error.into_inner());
        let next_sequence = inner.next_sequence.checked_add(1).ok_or_else(|| {
            ReactError::Other("journal sequence exhausted before append".to_string())
        })?;
        let record = JournalRecord {
            sequence: inner.next_sequence,
            event: event.clone(),
        };
        inner.next_sequence = next_sequence;
        inner.records.push(record.clone());
        Ok(record)
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
    /// Sequence durably assigned to the event.
    pub sequence: u64,
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
    pub fn apply(&self, event: &R::Event) -> Result<ApplyReceipt> {
        let mut inner = self.inner.lock().unwrap_or_else(|error| error.into_inner());
        let record = self.journal.append(event)?;
        inner.state.apply(&record.event);
        inner.last_applied = record.sequence;
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
            sequence: record.sequence,
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
        let loaded = self.checkpoints.load();
        let (mut state, mut last_applied, mut checkpoint_sequence, mut repair_reason) = match loaded
        {
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
            Ok(None) => (R::default(), 0, 0, None),
            Err(error) => (
                R::default(),
                0,
                0,
                Some(format!("checkpoint load failed: {error}")),
            ),
        };
        let records = self.journal.replay_after(last_applied, usize::MAX)?;
        for record in records {
            state.apply(&record.event);
            last_applied = record.sequence;
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
        let first = journal.append(&10).expect("append");
        let second = journal.append(&20).expect("append");
        assert_eq!(first.sequence, 1);
        assert_eq!(second.sequence, 2);
        assert_eq!(journal.last_sequence(), 2);
        assert_eq!(journal.next_sequence(), 3);
    }

    #[test]
    fn replay_after_respects_boundary_and_limit() {
        let journal = MemoryEventJournal::new();
        for value in 0..10 {
            journal.append(&value).expect("append");
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
        assert_eq!(fixture.reducer.apply(&5).expect("apply").sequence, 1);
        assert_eq!(fixture.reducer.apply(&7).expect("apply").sequence, 2);
        assert_eq!(fixture.journal.last_sequence(), 2);
        fixture.reducer.with_state(|state| {
            assert_eq!(state.total, 12);
            assert_eq!(state.events, vec![5, 7]);
        });
    }

    #[test]
    fn automatic_checkpoint_fires_on_cadence() {
        let fixture = reducer_with(2);
        fixture.reducer.apply(&1).expect("apply");
        assert!(fixture.checkpoints.load().expect("load").is_none());
        fixture.reducer.apply(&2).expect("apply");
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
            fixture.reducer.apply(&value).expect("apply");
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
            fixture.reducer.apply(&value).expect("apply");
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
    fn manual_checkpoint_round_trips_through_store() {
        let fixture = reducer_with(0);
        for value in 1..=3 {
            fixture.reducer.apply(&value).expect("apply");
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

        let first = reducer.apply(&7).expect("event append must commit");
        assert_eq!(first.sequence, 1);
        assert!(matches!(
            first.checkpoint,
            CheckpointApplyStatus::Degraded { .. }
        ));
        assert_eq!(journal.last_sequence(), 1);
        reducer.with_state(|state| assert_eq!(state.events, vec![7]));

        let second = reducer.apply(&9).expect("later append retries checkpoint");
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
                reducer.apply(&value).map(|receipt| receipt.sequence)
            }));
        }
        for handle in handles {
            assert!(handle.join().is_ok_and(|result| result.is_ok()));
        }

        let journal_events = journal
            .replay_after(0, usize::MAX)
            .expect("journal replay")
            .into_iter()
            .map(|record| record.event)
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
        journal.append(&3).expect("append");
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
}
