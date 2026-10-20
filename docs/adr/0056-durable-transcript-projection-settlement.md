# ADR 0056: Durable Transcript Projection Settlement

## Status

Accepted

- Date: 2026-09-17
- Owners: `echo-core::memory`, `echo-state::memory`, `state`, `agent/snapshot`

## Context

`ContextManager` owns the active model window, `RuntimeStateStore` owns ReAct
checkpoints, and `ConversationStore` owns the user-visible transcript. The old
projection path performed `get_messages` followed by `save_messages`, logged
backend failures as warnings, and still allowed compaction or terminal
publication. A crash, timeout, or lost acknowledgement could therefore leave a
newer checkpoint beside an older transcript with no durable retry obligation.

Mature systems treat this uncertainty explicitly. Temporal requires stable
idempotency keys for retryable effects. Claude Code continuously persists
resumable transcripts, while the Claude Agent SDK requires ordered append/load
and recommends transactions, CAS, or locking for concurrent derived state.

References:

- <https://code.claude.com/docs/en/sessions>
- <https://code.claude.com/docs/en/agent-sdk/session-storage>
- <https://docs.temporal.io/activity-definition>

## Options Considered

1. Keep best-effort projection and add logging. Rejected because logs are
   observations, not recoverable debt.
2. Add a second transcript Outbox. Rejected because one serialized Agent
   generation needs at most one pending projection; another queue would
   duplicate retry, retention, and terminal authority.
3. Store pending runtime intent in `ConversationStore`. Rejected because it
   mixes uncommitted intent with committed transcript facts.
4. Extend the existing Store authorities and let one framework coordinator own
   prepare/apply/ack/recovery/retirement ordering. Selected.

## Decision

- Enabling `ConversationStore` requires `RuntimeStateStore`. Admission rejects
  a store-only or non-atomic pair before guard, trace, context, model, or Store
  side effects. Checkpoint-only and fully unconfigured Agents remain supported.
- `TranscriptProjectionBatch` binds schema, conversation epoch, runtime
  generation, ordinals, complete `StoredMessage` values, and original
  `created_at` fields into one deterministic operation identity.
- Runtime compare-and-save persists the complete checkpoint and one
  `PendingTranscriptProjection` before the transcript backend sees the effect.
- `apply_transcript_projection` atomically merges one batch. Identical replay
  returns `AlreadyApplied`; ordinal/content or epoch conflicts make no partial
  mutation.
- After `Applied` or `AlreadyApplied`, checkpoint CAS advances cursor-after and
  clears pending. Only then may in-memory cursor or compaction advance.
- Timeout after dispatch is outcome-unknown. Durable pending yields `Deferred`;
  admission and recovery retry the exact stored batch before model execution or
  `Hydrated` publication.
- `TranscriptProjectionSettlement` is emitted before the single execution
  terminal and may be copied to Trace. It is not another terminal or authority.
  `Blocked`/`Conflict` suppress the original business terminal; the stream then
  emits one persistence-classified `Error` terminal rather than claiming the
  original execution completed.
- One absolute settlement deadline is passed through framework, Host, and
  extension calls as non-authoritative call context. Stores must explicitly
  advertise `AbsoluteDeadlineV1` and recheck the deadline after queue/authority
  lock acquisition but before durable mutation. Retry changes the deadline
  without changing the durable effect identity.
- The framework defaults to a 10-second settlement budget. Applications set a
  non-zero `persistence_settlement_timeout` on `AgentConfig` or
  `ReactAgentBuilder`; the framework derives one absolute call deadline for
  each safe point. Managed store-backed clear/delete helpers also expose
  context-aware variants for caller-owned absolute deadlines. Legacy
  unmanaged helpers keep their compatibility entrypoints but do not advertise
  a hard cross-backend deadline; their context-aware variants fail closed.
- Exact retirement leaves generation tombstones. Product delete first stores a
  complete scope manifest, then advances the conversation epoch/tombstone,
  retires every captured generation, and stores an idempotent receipt.
- Exact clear uses the same store-backed settlement coordinator as Agent
  admission and recovery, then retires runtime authority without guessing a
  separate managed transcript delete identity. Managed product delete accepts a stable
  `ManagedConversationDelete` request; the legacy convenience helper refuses a
  managed pair rather than deriving a new identity from a possibly recreated
  current epoch.
- Managed import, metadata update, and delete use full-payload identities and
  expected epoch/revision. Legacy raw mutators only operate on unmanaged data.
- File and SQLite backends expose the same receipts. File uses atomic manifest
  replacement under its single-writer lease; SQLite uses immediate transactions.

## Retained and derived model-context occurrences

The original suffix/prefix cursor comparison duplicated retained messages when
compression evicted a final response or rewrote an interior tool group. A model
window may retain several separated transcript occurrences, add a synthetic
summary, and still retain a later original fact. Giving that summary a new fact
ordinal before the retained fact would break the existing ordered frontier.

Considered alternatives were global content deduplication (which would swallow
new same-text turns), allocating ordinals to every rewritten model message
(which invents transcript events), and storing a second context-history ledger
(which duplicates authority). The accepted extension keeps one checkpoint/cursor:

- `projected` remains the strictly ordered fact ordinal/digest frontier.
- Optional `context_only` positions/digests describe compression-derived active
  occurrences interleaved with those facts. They have no transcript ordinal and
  never acknowledge a new transcript event.
- Explicit compression starts after original facts settle. Ordered alignment
  retains unchanged occurrences, while unmatched transformed occurrences stay
  model-context-only. Ordinary later appends still allocate fresh ordinals even
  for repeated text. An empty active view retains the append frontier.
- Full messages and these bindings settle through the existing checkpoint CAS.
  Pending payloads validate their current model view against `cursor_after`;
  `cursor_before` retains its original shape and lineage checks.

Bound payloads use internal schema version 3. Earlier framework readers reject
that version through their existing gate instead of silently discarding binding
provenance. New readers retain the version 1/2 legacy path when bindings are
absent, without claiming that old payloads prove context-transform provenance.
A mislabeled version 1/2 payload carrying new bindings is rejected.

Managed preparation stages ContextManager messages, projection scopes and
accepted metrics, and defers `context_committed` observers. During Store I/O the
public Context contains the coherent pre-view and its mutex is released, so
read-only Store callbacks may reenter. Successful CAS publication installs the
frozen view/bindings/version without another await, then accepts its observers.
Errors and dropped futures restore the volatile footprint, invalidate runtime
hydration and require authoritative reload before another managed mutation.
They do not undo a possibly committed CAS or replay an external MemoryPromoter.
A changed System/protected/summary message still requires persistence even when
its visible fact bindings have not changed.

This separation follows the existing ADR references and two additional primary
observations: [Codex's compaction implementation](https://github.com/openai/codex/blob/89461ea6833832b0551ce24ee32205824d5c108d/codex-rs/core/src/compact.rs)
constructs replacement history outside normal conversation-item recording, and
[Claude Code's context description](https://code.claude.com/docs/en/how-claude-code-works)
and [session storage](https://code.claude.com/docs/en/sessions) distinguish a
reduced model window from the retained local transcript. The framework keeps
its own Store/CAS design and does not adopt another product's protocol.

The Rust aggregate checkpoint shape changes. Existing accepted SDK aggregate
contracts need their canonical generator's normal compatibility report and an
appropriate Host pin; newly introduced Rust fields/types are not automatically
new language operations. The opaque checkpoint carrier must preserve complete
version-3 values. No second Store, application policy or new remote DTO is added.
Focused evidence and the independent canonical-root refresh invariant remain
tracked separately from framework main and refreshed SDK consumer qualification.

## Canonical root prompt during model-view restore

A restored window can begin with a System summary or Global Objective before its
canonical root prompt. Refreshing working-directory rules or setting the root
prompt must use the existing `CanonicalContext.system_prompt` identity, rather
than replacing the first System message. `ContextManager::update_system` is the
shared owner for refresh, trait/inherent setters and skill injection. It matches
the complete pure canonical System payload, reuses an already present desired
root, and preserves independently owned System messages and projection scopes.
Without canonical context, its standalone first-System contract remains intact.
A real canonical message change notifies accepted observers once; equal payloads
and metadata-only root reconciliation do not invent another accepted change.
This independently discovered restore invariant has its own Finding and does
not reopen the separately repaired append ordinal invariant.

Root identity and root value are separate constraints. Live roots render the
existing `current_system_prompt()` value, where a runtime override takes priority
over the configured base. The shared private renderer keeps the original CoT,
model-profile suffix and project-rule ordering. Updating the base prompt or
injecting a skill does not clear an active override; `replace_system_prompt`
remains the explicit clearing operation, and the borrowed trait getter continues
to expose the base. Working-directory refresh uses the same effective renderer.
Reset installs its final rendered root and canonical metadata together under
the Context lock, clearing scopes through the existing `set_messages` path and
accepting only that final view. Sync try-lock and async awaited-lock contracts
are unchanged. This selection constraint is part of the same canonical-root
Finding, with its own later natural before/after evidence.

## Consequences

Consumers with only `ConversationStore` must add a compatible
`RuntimeStateStore` or disable transcript projection. New trait methods default
to `Unsupported`, preserving source compatibility for external adapters, but
admission fails closed when durable projection is requested.

An independent SDK can carry the operations, receipts, deadlines, and
settlement event without recreating retry or terminal logic. That consumer
adoption is verified and released by the SDK repository; it is not part of the
framework Finding's completion boundary.

## Verification

File/SQLite parity covers concurrent generations, replay, managed CAS,
tombstone retention, epoch recreation, stale-writer fencing, rollback, and
restart. Agent tests cover admission, prepare/apply/ack crash cuts, warm/cold
recovery, pre-compaction blocking, terminal classes, consumer disconnect, and
observation-before-terminal ordering. Issue #106 closes when these framework
paths, evidence, documentation, examples, and framework merge gates are
complete on framework `main`.
