# ADR 0007: Atomic Journal Batch Commits

- Status: Accepted
- Date: 2026-08-26
- Owners: `echo-state/journal`

## Context

An event journal is the fact authority for consumers that rebuild a projection
after restart. Several related events may describe one logical transition. If a
writer loops over single-event appends, concurrent writers can interleave those
events and a crash can expose only a prefix. The projection then observes a
state that no caller committed.

The journal must also distinguish visibility from durability. A complete frame
may be visible and recoverable even when its requested `sync_data` barrier
reports an error. Retrying that committed payload as new work would duplicate
effects, while treating every write error as committed would lose absent work.

Mature storage systems converge on an explicit commit unit. RocksDB
`WriteBatch` applies a group atomically, and KurrentDB/EventStoreDB appends an
ordered event batch under an expected stream revision. Kafka likewise separates
the immutable log from the boundary consumers are allowed to observe. The
shared pattern is that a batch has one identity and one visibility decision;
it is not a convenience loop around independent writes.

References:

- <https://github.com/facebook/rocksdb/wiki/Basic-Operations#atomic-updates>
- <https://docs.kurrent.io/clients/grpc/appending-events.html>
- <https://kafka.apache.org/documentation/#design_transactional_semantics>
- <https://doc.rust-lang.org/std/fs/struct.File.html#method.sync_data>

## Options Considered

1. Keep `append(event)` as the only primitive and let callers loop. This cannot
   prevent interleaving or partial visibility.
2. Add a product-specific transaction around each EKO journal. This duplicates
   sequence, integrity, retry, and recovery rules outside the framework.
3. Make one prepared batch frame the framework journal commit boundary, with a
   typed receipt for committed, absent, conflicting, and unknown outcomes.

## Decision

`EventJournal::append_batch` is the canonical multi-event commit primitive.
`append` remains a one-event adapter over the same mechanism.

- `PreparedJournalBatch` serializes and hashes the payload before I/O, assigns a
  stable batch identity, rejects empty or oversized inputs, and validates that
  an interiorly mutable payload has not changed before append or lookup.
- A physical JSONL line contains the complete digest-protected batch frame.
  Records receive one contiguous global sequence range while the journal's
  append authority is held. A reader never observes a record prefix.
- Memory, file, and segmented journals share the same identity and receipt
  contract. Segmentation is physical layout only; a batch never crosses a
  segment boundary.
- A complete frame whose durability barrier fails returns a committed receipt
  with degraded durability. A proven zero/short write returns the prepared
  payload for a safe retry. An ambiguous result returns `OutcomeUnknown` and
  requires reopen plus `lookup_batch` reconciliation before retry.
- Reusing an identity with the same digest is idempotent. Reusing it with a
  different payload is a conflict and poisons the live authority until reopen.
- Complete corruption fails closed. Only a torn trailing frame is truncated on
  reopen, which keeps the incomplete batch invisible.

Sequencing, frame integrity, durability classification, and reconciliation are
generic persistence mechanisms, so they remain in `echo-state`. Applications
choose stream roots, retention, UI projections, and which domain events belong
in a logical batch. They must not add a second sequence or commit authority.

## Consequences

Consumers can project every committed transition without seeing an interleaved
or partial batch. Unknown write outcomes require an explicit reopen/lookup step,
which is more work than blind retry but prevents duplicate effects. File formats
now use batch frames rather than one independently committed record per line.

Checkpoint compounding continues after the complete batch is folded. A
checkpoint failure degrades the receipt but does not roll back authoritative
journal records; recovery can rebuild from the journal.

## Verification

The conformance suite covers contiguous concurrent batches, non-interleaving,
idempotent identity reuse, identity conflicts, interior payload mutation,
serialization and sequence overflow, complete/short/zero/ambiguous write fault
matrices, reopen reconciliation, torn-tail repair, complete corruption
rejection, segment rollover, checkpoint degradation, and non-`Clone` events.
The journal benchmark compares batch sizes across memory, file, and segmented
implementations and measures checkpoint-compounded recovery.
