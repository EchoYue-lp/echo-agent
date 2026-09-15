# ADR 0055: Bind Checkpoints to Journal Generation Identity

## Status

Accepted

- Date: 2026-09-15
- Owner: `echo-state::journal`

## Context

`CheckpointedReducer` previously persisted only reducer state and an applied
sequence. A checkpoint produced from Journal B could therefore pass every
range check when paired with Journal A at the same or a later sequence. The
reducer would report `Loaded` and expose a projection that was never derived
from Journal A. After Journal A pruned its prefix, the invalid state could no
longer be rebuilt from the retained facts.

Sequence is a position inside an authority, not a globally meaningful
identity. The same constraint appears in mature checkpoint systems:

- [LangGraph checkpointers](https://docs.langchain.com/oss/python/langgraph/checkpointers)
  key state by `thread_id`, `checkpoint_ns`, and `checkpoint_id`; subgraphs use
  distinct namespaces rather than treating a checkpoint number as global.
- [Apache Flink savepoints](https://nightlies.apache.org/flink/flink-docs-stable/docs/ops/state/savepoints/)
  map state by stable operator UID and reject state that cannot be mapped by
  default. Flink explicitly warns that generated identity drift can associate
  state with the wrong operator.
- [Axon streaming processors](https://docs.axoniq.io/axon-framework-reference/4.12/events/event-processors/streaming/)
  bind tracking progress to a processor and segment, and expose a unique Token
  Store storage identifier so shared storage locations remain distinguishable.

The common rule is that a replay position must be validated together with the
identity of the fact source or state owner.

## Options Considered

1. Continue validating sequence only. Rejected because it preserves the known
   cross-Journal corruption path.
2. Derive identity from a file path or the first batch. Rejected because paths
   can move and the same prepared batch can be committed to two independent
   journals.
3. Add a caller-selected product scope. Rejected as the only authority because
   a stable scope does not distinguish replacement generations and would move
   product naming policy into the framework.
4. Allocate an opaque Journal generation identity, persist it with Journal
   frames and retention metadata, and copy it into checkpoints. Selected.

## Decision

`EventJournal` owns one `JournalIdentity` for the lifetime of a generation.
`MemoryEventJournal`, `FileEventJournal`, and `SegmentedFileEventJournal`
allocate it when a new authority is created. File-backed journals include the
identity in every digest-protected batch frame. Segmented retention markers
also persist it so the identity survives removal of the first physical segment.

`CheckpointStore::save` requires the Journal identity, and `CheckpointFrame`
returns it on load. `FileCheckpointStore` includes the identity in the
checkpoint integrity digest. Append receipts also carry the committing Journal
identity, so `apply_committed` cannot fold a receipt from another physical
authority and then save that foreign state under the local identity.
`CheckpointedReducer` is the only component that pairs the two authorities:

- matching identity and valid sequence may load normally;
- mismatched identity with a complete Journal discards the checkpoint, replays
  the authoritative Journal from sequence zero, and repairs the checkpoint;
- mismatched identity after prefix pruning fails closed because the missing
  facts cannot be reconstructed.

The identity is a generation fence, not a product stream, tenant, workflow, or
UI identifier. Applications still choose paths, retention policy, and payload
scope. Workflow checkpoints remain a separate authority.

## Compatibility

Journal batch frames, file checkpoints, and segmented retention markers move
to schema version 2. Version 1 did not contain enough information to prove a
source binding, so accepting or guessing an identity for it would retain the
bug. The framework is currently in pre-stable development and these private
disk frames have no migration contract; version 1 files are rejected with the
existing strict decode/schema boundary and must be recreated from their owning
source. No legacy reader or parallel checkpoint path is retained.

The public `EventJournal` and `CheckpointStore` traits gain the identity
contract. Custom persistent backends must persist one stable generation UUID
and include it when saving checkpoints. This deliberate pre-1.0 API break keeps
custom implementations subject to the same invariant as built-in backends.

## Consequences

- Same-sequence checkpoints from another Journal or a replaced generation can
  no longer be reported as loaded.
- File and segmented Journal integrity now detects mixed generation frames.
- A new empty file-backed Journal has no durable facts until its first batch;
  that first frame persists the already allocated identity.
- Prefix-pruned recovery remains available only when the checkpoint identity
  matches the retained Journal generation.
- Digests and serialized fixtures change because identity participates in the
  integrity input.

## Verification

Focused tests cover cross-Journal same-sequence recovery, same-path file
generation replacement, mixed batch generations, identity tampering, identity
survival through segmented pruning/reopen, foreign committed receipts,
identity-free schema version 1 rejection, cross-generation segment mixing, and
rejection of a foreign checkpoint when the retained floor prevents a full
rebuild.
