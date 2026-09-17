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

## Consequences

Consumers with only `ConversationStore` must add a compatible
`RuntimeStateStore` or disable transcript projection. New trait methods default
to `Unsupported`, preserving source compatibility for external adapters, but
admission fails closed when durable projection is requested.

The independent SDK must carry the new operations, receipts, deadlines, and
settlement event without recreating retry or terminal logic. Product surfaces
consume the same framework status.

## Verification

File/SQLite parity covers concurrent generations, replay, managed CAS,
tombstone retention, epoch recreation, stale-writer fencing, rollback, and
restart. Agent tests cover admission, prepare/apply/ack crash cuts, warm/cold
recovery, pre-compaction blocking, terminal classes, consumer disconnect, and
observation-before-terminal ordering. Framework and SDK pass their own merge
gates before Issue #106 closes.
