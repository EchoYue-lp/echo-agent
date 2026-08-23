//! Criterion benchmarks for the sequenced event journal primitives.
//!
//! Run with: `cargo bench -p echo_agent --bench journal_bench`
//!
//! These numbers back the Task-5 performance gates at the primitive level:
//! append throughput, full replay throughput, and the checkpoint-compounded
//! recovery ratio (the reason `CheckpointedReducer` exists is that a 100k
//! recovery should replay only the tail, not the whole journal).

use criterion::{Criterion, criterion_group, criterion_main};
use echo_agent::workspace::state::journal::{
    CheckpointStore, CheckpointedReducer, EventJournal, EventReducer, FileCheckpointStore,
    FileEventJournal, MemoryCheckpointStore, MemoryEventJournal,
};
use echo_core::utils::fs::FileDurability;
use std::sync::Arc;

/// Reducer that folds counted events into a small state.
#[derive(Default, serde::Serialize, serde::Deserialize)]
struct BenchReducer {
    applied: u64,
    checksum: u64,
}

impl EventReducer for BenchReducer {
    type Event = u64;

    fn apply(&mut self, event: &u64) {
        self.applied = self.applied.saturating_add(1);
        self.checksum = self.checksum.wrapping_add(*event);
    }
}

fn bench_journal(c: &mut Criterion) {
    const RECOVERY_EVENTS: u64 = 105_321;
    const CHECKPOINT_CADENCE: u64 = 10_000;
    let mut group = c.benchmark_group("journal");

    group.bench_function("memory_append", |b| {
        b.iter(|| {
            let journal = MemoryEventJournal::<u64>::new();
            for value in 0..10_000u64 {
                journal.append(&value).expect("append");
            }
            journal.last_sequence()
        })
    });

    group.bench_function("memory_replay_after_zero_10k", |b| {
        b.iter_batched(
            || {
                let journal = MemoryEventJournal::<u64>::new();
                for value in 0..10_000u64 {
                    journal.append(&value).expect("append");
                }
                journal
            },
            |journal| journal.replay_after(0, usize::MAX).expect("replay").len(),
            criterion::BatchSize::LargeInput,
        )
    });

    group.bench_function("file_append_flush_10k", |b| {
        b.iter(|| {
            let dir = tempfile::tempdir().expect("tempdir");
            let journal = FileEventJournal::<u64>::open(
                dir.path().join("events.jsonl"),
                FileDurability::Flush,
            )
            .expect("open journal");
            for value in 0..10_000u64 {
                journal.append(&value).expect("append");
            }
            journal.last_sequence()
        })
    });

    group.bench_function("file_replay_after_zero_10k", |b| {
        b.iter_batched(
            || {
                let dir = tempfile::tempdir().expect("tempdir");
                let journal = FileEventJournal::<u64>::open(
                    dir.path().join("events.jsonl"),
                    FileDurability::Flush,
                )
                .expect("open journal");
                for value in 0..10_000u64 {
                    journal.append(&value).expect("append");
                }
                (dir, journal)
            },
            |(_dir, journal)| journal.replay_after(0, usize::MAX).expect("replay").len(),
            criterion::BatchSize::LargeInput,
        )
    });

    // The compounded cases keep a non-zero 5,321-event tail after the last 10k
    // checkpoint. This crosses multiple fixed recovery batches instead of
    // measuring an exactly-on-cadence or single-batch tail.
    let build_memory_journal = |size: u64| {
        let journal = Arc::new(MemoryEventJournal::<u64>::new());
        let writer = CheckpointedReducer::<_, BenchReducer>::new(
            Arc::clone(&journal),
            Arc::new(MemoryCheckpointStore::new()),
            CHECKPOINT_CADENCE,
        );
        for value in 0..size {
            writer.apply(&value).expect("apply");
        }
        journal
    };

    group.bench_function("recover_full_replay_105k", |b| {
        b.iter_batched(
            || build_memory_journal(RECOVERY_EVENTS),
            |journal| {
                let reducer = CheckpointedReducer::<_, BenchReducer>::new(
                    journal,
                    Arc::new(MemoryCheckpointStore::new()),
                    CHECKPOINT_CADENCE,
                );
                reducer.recover().expect("recover").last_applied_sequence
            },
            criterion::BatchSize::LargeInput,
        )
    });

    group.bench_function("recover_compounded_105k_tail_5321", |b| {
        b.iter_batched(
            || {
                // Real-world path: a writer compounded checkpoints every 10k
                // events; recovery loads the latest frame and replays only
                // the tail.
                let journal = Arc::new(MemoryEventJournal::<u64>::new());
                let checkpoints: Arc<dyn CheckpointStore<BenchReducer>> =
                    Arc::new(MemoryCheckpointStore::new());
                let writer = CheckpointedReducer::<_, BenchReducer>::new(
                    Arc::clone(&journal),
                    Arc::clone(&checkpoints),
                    CHECKPOINT_CADENCE,
                );
                for value in 0..RECOVERY_EVENTS {
                    writer.apply(&value).expect("apply");
                }
                (journal, checkpoints)
            },
            |(journal, checkpoints)| {
                let reducer = CheckpointedReducer::<_, BenchReducer>::new(
                    journal,
                    checkpoints,
                    CHECKPOINT_CADENCE,
                );
                let recovered = reducer.recover().expect("recover").last_applied_sequence;
                reducer.with_state(|state| {
                    assert_eq!(state.applied, recovered);
                });
                recovered
            },
            criterion::BatchSize::LargeInput,
        )
    });

    // Persistent recovery is the production-relevant gate. FileEventJournal
    // records byte offsets during its validated open and seeks directly to the
    // checkpoint suffix instead of scanning/deserializing the complete JSONL.
    let persistent_dir = tempfile::tempdir().expect("persistent tempdir");
    let persistent_journal = Arc::new(
        FileEventJournal::<u64>::open(
            persistent_dir.path().join("events.jsonl"),
            FileDurability::Flush,
        )
        .expect("open persistent journal"),
    );
    let persistent_checkpoints: Arc<dyn CheckpointStore<BenchReducer>> = Arc::new(
        FileCheckpointStore::open(persistent_dir.path().join("checkpoint.json")),
    );
    let persistent_writer = CheckpointedReducer::<_, BenchReducer>::new(
        Arc::clone(&persistent_journal),
        Arc::clone(&persistent_checkpoints),
        CHECKPOINT_CADENCE,
    );
    for value in 0..RECOVERY_EVENTS {
        persistent_writer.apply(&value).expect("persistent apply");
    }
    group.bench_function("file_recover_compounded_105k_tail_5321", |b| {
        b.iter(|| {
            let reducer = CheckpointedReducer::<_, BenchReducer>::new(
                Arc::clone(&persistent_journal),
                Arc::clone(&persistent_checkpoints),
                CHECKPOINT_CADENCE,
            );
            reducer
                .recover()
                .expect("persistent recover")
                .last_applied_sequence
        })
    });

    group.finish();
}

criterion_group!(benches, bench_journal);
criterion_main!(benches);
