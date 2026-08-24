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

File storage writes the scope index before the checkpoint. Exact/scope deletion
removes checkpoint data before removing index entries. A crash can therefore
leave a harmless tombstone that a retry can reclaim, but not an unindexed new
checkpoint. SQLite performs index/checkpoint mutations in one transaction.

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
File crash-tombstone recovery, SQLite transactional lineage, and snapshot
checkpoint registration under distinct product/runtime identities.
