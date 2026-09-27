# ADR 0080: Managed Transcript Import Seeds One Projection Generation

## Status

Accepted

## Context

`ManagedConversationImport` atomically replaces a conversation transcript and advances its epoch, but clears all projection ordinals. A caller that restores the imported messages into an Agent's runtime checkpoint then projects the whole prefix again because the Store has no generation-local digest frontier. A caller that skips runtime hydration loses the imported model context. This affects any consumer that forks, rewinds, or restores a managed conversation; it is not an EKO UI concept.

LangGraph's official [persistence guide](https://docs.langchain.com/oss/python/langgraph/persistence) separates thread-scoped checkpoints from durable stores and requires a stable thread identity for continuation. Claude Code's official [subagent documentation](https://code.claude.com/docs/en/sub-agents) treats forked work as an independent context rather than a UI-only transcript copy. The shared requirement is that imported visible history and the resumed execution cursor describe the same logical prefix.

## Options

1. Let hosts copy rows with legacy `save_messages` and call `ReactAgent::load_messages`. Rejected: managed stores and Agents now reject both bypasses.
2. Import rows, then let the next turn reproject them. Rejected: duplicate visible messages and ordinal drift.
3. Bind an optional runtime generation to the exact managed import request and atomically seed its digest frontier in the transcript Store (chosen). A proof-bearing checkpoint CAS binds the new epoch; the host creates or repairs the separate runtime checkpoint.

## Decision

- `ManagedConversationImport` may bind one non-empty `generation_id`. The ID participates in the request digest, so a retry cannot change the runtime generation under one operation identity.
- File and SQLite transcript Stores replace rows, advance the epoch, and seed ordinals `0..N` for that generation in the same Store transaction. Store replay uses exact imported-row digests, as ordinary projection batches do.
- The same transaction retains a small `ManagedConversationImportLocator` with the original expected epoch/revision, generation, operation identity and applied authority. A metadata-only revision may advance afterward; querying the locator and replaying the exact import still returns the original applied authority. A later transcript replacement supersedes the locator. This is a projection of the Store receipt, not a new state authority.
- Generation-bound imports validate that each row can be restored into a runtime Message before Store mutation. A caller constructs a `TranscriptProjectionCheckpoint` with `next_ordinal=N` and canonical runtime digests from `managed_import_projection_digests`. These digests intentionally differ from Store replay digests: the runtime cursor excludes backend IDs, timestamps, and UI-only metadata.
- `RuntimeCheckpointCasRequest.managed_import` carries the exact import request and applied/idempotent receipt. File and SQLite runtime Stores accept a one-step live epoch advance only when this proof matches the scope, generation, checkpoint history, and cursor and the expected scope/state revisions still match. Ordinary CAS remains fenced across epoch changes. The transcript Store, runtime CAS, and import receipt remain the authorities; no EKO-specific state enters the framework.
- If import commits but checkpoint CAS is interrupted, the caller must recover from the durable locator and imported rows before another turn. The Store can replay the same import request idempotently even after a metadata-only update; a mismatched generation or changed rows fail closed. This ADR does not authorize blind overwrite of a live run.

## Consequences

- Framework consumers gain a generic managed fork/rewind primitive without using legacy writes.
- Hosts still own admission, identity locks, cancellation, and checkpoint recovery policy. EKO's Side Conversation visibility and UI tree remain entirely application-owned.
- The new public request fields and cursor helper require File/SQLite parity tests, facade example coverage, feature-matrix compilation, and a full framework gate.
