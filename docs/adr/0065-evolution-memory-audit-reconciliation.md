# ADR 0065: Recover Layered Memory Mutations with Durable Audit Identities

- Date: 2026-09-19
- Owners: `src::evolution`, `echo-state::journal`

## Status

Accepted

## Context

`MemoryLayerManager` previously wrote warm Store values or hot `MEMORY.md`
before recording the corresponding `ChangeLog` entry. A failed audit append,
cancelled future, or process crash could leave a visible mutation with no audit
fact. Promotion, demotion, and approved merge can touch more than one storage
location. The framework `Store` contract offers independent `put` and `delete`
but no transaction spanning Store, `MEMORY.md`, and `ChangeLog`.

[ADR 0007](0007-atomic-journal-batch-commits.md) already owns prepared batch
identities, durable append, unknown-outcome classification, replay, and the
file-journal lease. [ADR 0053](0053-trace-audit-persistence-visibility.md)
separates optional diagnostic delivery from required business audit, and
[ADR 0058](0058-background-review-settlement-ownership.md) makes an attempted
memory write with unknown outcome visible to its caller. This decision binds
those contracts to memory mutation without changing their owners.

The [AWS transactional outbox pattern](https://docs.aws.amazon.com/prescriptive-guidance/latest/cloud-design-patterns/transactional-outbox.html)
uses a shared transaction and an idempotent consumer to close dual writes.
The [PostgreSQL WAL contract](https://www.postgresql.org/docs/current/wal-intro.html)
flushes a recoverable log before changing data pages. Our arbitrary Store
implementations cannot join a transaction with a file, so the operation log
must be the recovery authority rather than claiming the same atomic visibility
as a relational outbox.

## Options

1. Append audit before mutation. Rejected: an unsuccessful Store write would
   appear in the committed business audit.
2. Mutate first and compensate in memory if audit fails. Rejected: a crash
   loses the compensation and a Store error may already have persisted bytes.
3. Require every Store to support cross-resource transactions. Rejected: this
   removes valid framework Store implementations and still does not cover the
   hot file.
4. Durably prepare a complete operation, project it forward, append the same
   business audit identities idempotently, then settle the operation. Chosen.

## Decision

`MemoryLayerManager` is the sole owner for layered write, delete, status
revival/archival, hot/warm movement, budget demotion, and approved warm merge.
`MemoryMerger` retains deterministic merge policy but delegates the complete
group to this owner. An operation batch contains stable IDs, all affected keys,
their before/after warm values and hot entries, and complete `ChangeEntry`
payloads. `echo-state::FileEventJournal` stores one SyncData-backed Prepared
fact before any projection write; a group uses one physical prepared frame.

The manager verifies that each current target equals its captured before or
after value. It applies destinations before source removal, then calls
`ChangeLog::record_idempotent` for every member and appends a Settled fact.
Promotion, demotion, revival, and delete decisions made from a prior read
carry that expected key/layer/value into the root-serialized prepare section.
If another manager changed the key between decision and prepare, no new
operation is prepared and the stale decision fails instead of overwriting the
newer value. Explicit writes remain last-writer-wins under the same serial.
If a Store write or audit/settlement append fails, the prepared fact is durable
debt; returning `Err` never claims that no mutation occurred. Reopen plus
`reconcile_pending` replays the same target values and change IDs. A committed
audit followed by failed settlement is replayed without another audit line.
`FileStore` can also publish a candidate and return success after its own
durability barrier degraded. Consequently startup reconciliation checks the
last projection for **every** journaled key, including settled operations:
if Store or the hot file reverted to a historical before/after state, the
journal's latest target is reapplied. Unknown external values fail closed.
The operation journal remains the recovery authority after settlement;
`ChangeLog` remains the business audit authority.
Unknown journal append outcomes are never retried blindly: the live poisoned
authority refuses further mutations; after handles close, verified reopen and
replay determine whether the prepared or settled frame exists. A degraded
durability receipt must pass `sync_data` before any projection write, and replay
also establishes that barrier before applying a pending fact.

Managers for one canonical journal path share an in-process serial lock across
prepare, projection, audit, settlement, and reconciliation. The file journal
holds a process-lifetime exclusive cross-process writer lease, so a second
process fails to construct a usable manager while the first owns that root;
it cannot interleave two otherwise independent transactions. Public manager reads either
reconcile first or report pending debt. Synchronous hot reads return an error
while an operation runs or before startup reconciliation of existing journal
history. `try_new` rejects a broken
journal at construction, and the runtime builder's async reconciled variant
completes recovery before returning the manager.

The operation journal is a crash-recovery authority, not a second business
audit or a rollback API. `ChangeLog` remains the queryable audit; its stable
change IDs deduplicate committed entries. Observer callbacks run once only
after live settlement. Recovery does not replay observer callbacks because
their external effects have no durable consumer acknowledgement.

Hot `MEMORY.md` keeps the existing plain bullet representation for ordinary
single-line content. Entries whose content contains a newline or leading or
trailing whitespace set `content_json: true` in the YAML frontmatter and store
the exact content as a JSON string on one bullet line. Only tagged entries
decode that string. This preserves legacy human-edited bullets and makes the
prepared `hot_after` byte-for-byte equal to the value read after persistence;
demotion back to warm reconstructs the original text without trimming.

This changes a public constructor from
`MemoryMerger::new(&typed_store, &change_log)` to
`MemoryMerger::new(&layer_manager)`. `merge_group` and `MergeResult` retain
their public behavior and deterministic primary selection. Framework consumers
must construct a manager with their existing Store and ChangeLog, reconcile it
at startup, and pass that manager to the merger. This deliberately removes the
second write authority; a thin adapter preserving the old constructor would
still permit unaudited partial merges.

## Consequences

Post-merge closure review on `cb4ee9ed3826fd8055f027e84f69b94dbc329267` independently
confirmed the layered-memory contract; later rollback remains Issue #52.

There is no cross-file atomic visibility. An independent holder of the raw
`Store`, or a reader opening `MEMORY.md` directly, can observe a prepared
intermediate projection before reconciliation. Consumers needing a settled
read must use the manager or run `reconcile_pending` before exposing the raw
Store. An external writer changing a journaled key to an unrelated value makes
replay fail closed instead of overwriting it. If the new value coincides with
a known historical state, provenance cannot be distinguished; the journal
target wins. Recall count and last-recalled time are diagnostic metadata:
they are excluded from semantic conflict comparison, and newer values are
preserved when the projection must be repaired. The journal can rebuild
ephemeral stores for every retained operation; retaining the complete history
is therefore required until a future, explicitly verified compaction scheme.

The journal retains prepared/settled history and full target values. Reconcile
currently scans the full history on each manager operation/read to prove that
even an already-settled Store projection did not roll back; this is linear in
retained journal length and can become expensive. Any bounded checkpoint must
prove its own durable generation/sequence binding and retained recovery window
before replacing that scan. Journal growth, retention, multi-key later rollback, raw Store readers, and mutations
outside the layered manager have separate ownership; this ADR does not close
the `ChangeLog` later-rollback Finding (#52). Existing `ChangeLog` audit
content remains unchanged. Framework mechanics live in `echo-agent`; EKO
startup/scheduling/UI policy stays in the application.

## Verification

Focused tests exercise prepare refusal with zero mutation, audit failure
followed by restart and duplicate-free reconciliation, settled projection
rollback, partial warm/hot moves, delete and metadata updates, two managers
sharing a root, a second merge
member's audit failure, public manager read fencing, and the real runtime
builder. Evolution, tool, promoter, trigger, dreaming, example, formatting,
lint, and workspace gates remain the delivery boundary.
Additional two-manager interleavings prove that a stale warm promotion, hot
demotion, or warm archival decision cannot overwrite a newer write; unusual
whitespace and multiline content survive hot read, restart recovery, demotion,
and the next operation.
