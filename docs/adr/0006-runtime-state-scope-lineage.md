# ADR 0006: Durable Runtime-State Scope Lineage

- Status: Accepted
- Date: 2026-08-25
- Owners: `state`, `agent/snapshot`

## Context

A stable product conversation can intentionally rotate through multiple model
contexts. Channel timeout/reset is one example: the user-visible transcript and
Task history remain under one stable conversation, while each handler
incarnation receives a different `RuntimeStateStore` checkpoint identity.

Separating those identities prevents a new model context from restoring the old
checkpoint, but identity separation alone leaves old checkpoints on disk. A
later product delete also cannot enumerate hashed/opaque runtime IDs after a
process restart.

This is a general checkpoint-retention problem, not an EKO policy. LangGraph
similarly distinguishes thread-scoped checkpointers from cross-thread stores,
provides `delete_thread` for all checkpoints/writes under a thread, and
recommends pruning or retention because checkpoints grow without bound:

- <https://docs.langchain.com/oss/python/langgraph/persistence>
- <https://github.com/langchain-ai/langgraph/blob/main/libs/checkpoint/langgraph/checkpoint/base/__init__.py>

## Options Considered

1. Leave retired checkpoints unreadable but never delete them. Runtime behavior
   is correct, but disk use grows indefinitely and product deletion is partial.
2. Scan backend paths/tables during deletion. This leaks concrete backend
   layouts into callers and cannot reliably recover opaque ownership.
3. Keep a product-specific index in each application. This duplicates
   persistence authority and makes framework backends incomplete.
4. Add a stable scope-to-runtime lineage to `RuntimeStateStore`, with exact and
   whole-scope deletion. This keeps checkpoint ownership in its existing store
   and remains reusable outside channels.

## Decision

`RuntimeStateStore` owns a durable mapping from one stable `scope_id` to every
globally unique `runtime_state_id` saved for that scope.

- `save_checkpoint_for_scope` binds the runtime ID and saves its checkpoint.
- `runtime_state_ids` returns the sorted durable lineage.
- `clear_runtime_state` deletes one exact incarnation and its binding. A reset
  calls this after old foreground/resource settlement and before admitting the
  replacement model context.
- `clear_runtime_state_scope` deletes all indexed incarnations. It is
  idempotent and also reclaims a legacy checkpoint whose ID equals the scope.
- `clear_persisted_runtime_incarnation` also deletes any transcript written
  under the incarnation ID, while preserving the stable scope transcript.
- `delete_persisted_conversation` enumerates and deletes incarnation-keyed
  transcripts, clears the runtime scope, then deletes the stable transcript.

The framework does not derive product IDs, decide when a product reset is
allowed, or delete Task/application journals. Callers supply the stable scope
and exact runtime identity already carried by their invocation lifecycle.
Callers must close admission and settle foreground/resource owners before reset
or product deletion; that product admission barrier prevents a new incarnation
from being created between cross-store cleanup steps.

File storage has one authoritative record per runtime ID. `Active` records own
the scope binding and checkpoint in the same atomic JSON replacement;
`Deleting` records retain the exact cleanup obligation without checkpoint
payload. A retry can therefore finish every crash cut without choosing between
separate owner/index/checkpoint files. Scope-index files are rebuildable
projections and never decide ownership or whether an operation succeeded;
projection write/delete/fsync failure cannot block authoritative enumeration or
deletion.
Writes use temp-file fsync + rename + parent-directory fsync, deletions fsync the
parent even on an idempotent retry, and first directory creation publishes every
ancestor durably. One store-root lease plus a fixed set of in-process shards
replaces per-incarnation lock files, so lock metadata cannot grow with runtime
IDs. SQLite performs binding/checkpoint mutations in one transaction; all async
trait methods submit their complete connection/transaction closure to the
bounded keyed blocking owner. A foreign scope owning a runtime ID that equals
another scope's name disables only the legacy same-ID fallback; it cannot block
cleanup of rows genuinely owned by that other scope.

## Consequences

Reset remains a new empty model context, not a product-history wipe. It reclaims
the retired runtime checkpoint and any incarnation-keyed transcript while
preserving the stable transcript. Product delete removes the stable transcript,
incarnation transcripts, and every indexed runtime checkpoint.

The `RuntimeStateStore` public contract grows by four scope operations. Built-in
File and SQLite backends implement the same semantics; custom implementations
must provide durable indexing rather than an in-memory approximation.

Scope operations require runtime IDs to be globally unique. This matches UUID
and stable-hash invocation IDs and avoids ambiguous cross-scope ownership.

## Verification

Coverage includes multiple senders and incarnations, restart persistence,
sender-local exact reset, reset transcript retention, full product deletion,
every File `Active`/`Deleting`/unlink crash cut, competing scope claims, corrupt
projection repair, durable unlink retry, SQLite transactional lineage and Tokio
heartbeat, cross-backend same-name scope parity, invocation-aware checkpoint
restore/save symmetry, and snapshot checkpoint registration under distinct
product/runtime identities.
