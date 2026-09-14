# ADR 0033: Cancellation-safe Subagent factory publication

- Date: 2026-09-13
- Owners: `agent/subagent/registry`

## Status

Accepted.

## Context

`SubagentRegistry` owns the current definition, optional lazy factory, cached
Agent instance, and monotonically increasing registration revision for each
Subagent name. The previous lazy path coordinated creation through a
registry-wide `HashSet<String>`, polled every 50 milliseconds, and imposed a
30-second timeout only on waiters.

That coordination had two lifecycle defects. Cancelling the task awaiting
`AgentFactory::create` skipped marker cleanup, so later resolutions could only
poll and time out. Successful creation removed the marker before acquiring the
registry state lock and publishing the instance, so another thread could start
a second factory for the same revision in that gap.

Tokio 1.53.1 documents that `OnceCell::get_or_try_init` coalesces concurrent
initializers and leaves the cell uninitialized when the initializer returns an
error, is cancelled, or panics, allowing a waiting caller to retry:
<https://docs.rs/tokio/1.53.1/tokio/sync/struct.OnceCell.html#method.get_or_try_init>.

## Options Considered

1. Keep the global marker and add asynchronous cleanup plus stricter lock
   ordering. A future can be dropped at every await, so correct cleanup needs a
   second task or custom guard protocol; publication would still span two
   state authorities.
2. Add a per-name async mutex and a custom result/waiter state machine. This
   removes the global marker but must define failure fan-out, cancellation,
   poisoning, timeout, and generation replacement itself.
3. Store one async `OnceCell<Arc<dyn Agent>>` in each `RegistryEntry` and use
   the entry revision as the generation fence. This uses an existing,
   cancellation-safe primitive and makes successful construction and cached
   publication one operation.

## Decision

Choose option 3.

- Every `RegistryEntry` owns a revision-scoped `Arc<OnceCell<Arc<dyn Agent>>>`.
  Pre-built registration initializes the cell synchronously; definition-only
  and factory registration start with an empty cell.
- `get_agent` captures the current cell, factory, and revision, then calls
  `get_or_try_init`. Concurrent callers for that entry share one successful
  initialization. The value is visible from the cell before any waiter can
  start another factory for the same entry.
- After initialization, `get_agent` re-reads registry state and accepts the
  value only when both the revision and cell identity still match. Remove or
  re-register replaces the entry and therefore fences out the old result.
- Factory errors are not cached. Cancellation or panic also leaves the cell
  empty according to the Tokio contract; a later waiter may perform the next
  serialized attempt.
- The registry no longer owns an arbitrary construction deadline. Callers and
  runtime layers apply their own cancellation or timeout around `get_agent`.
  A factory must not recursively resolve its own registration because Tokio
  documents recursive initialization as a deadlock.
- `create_fresh_agent` remains outside the cached single-flight path: its
  contract intentionally creates a fresh instance for every request.
- Factory registration already publishes the executable definition. Filling
  its cached cell does not change that definition or increment the executable
  catalog revision; `get` reads `has_instance` directly from the cell.

## Consequences

- Cancelling a lazy creation no longer leaves process-global ownership behind;
  the next resolution can retry immediately.
- A registration revision has at most one successfully cached Agent instance,
  and no marker-to-state publication gap exists.
- Re-registering a name can proceed without waiting for an obsolete factory.
  Its old result is dropped rather than entering the new entry.
- The registry removes one global set, one notifier, the polling loop, and the
  registry-specific waiter timeout. Deadline policy remains at the caller that
  also owns the surrounding dispatch lifecycle.
- Concurrent callers waiting on a factory that fails may serialize a later
  retry, matching `get_or_try_init`; failure is not a permanent poison value.

## Compatibility And Rollback

Public method signatures, registration names, definitions, events, wire
formats, and SDK identities do not change. The observable correction is that a
cancelled attempt is retryable and a concurrent same-revision resolution
cannot create a second instance. Code that relied on the hidden 30-second
waiter timeout must instead apply an explicit caller timeout.

The change is isolated to the registry entry representation and can be rolled
back by reverting its single repair commit. No persisted state or migration is
involved.

## Verification

Deterministic tests abort an in-flight initializer and require the next
resolution to enter the factory. A test-only publication boundary pauses the
first resolver after the cache publication point and proves that a concurrent
resolver reuses the same `Arc` without a second factory call. Additional tests
cover error retry, remove/re-register generation fencing, existing pre-built
and definition-only registration, and executor consumers. Semantic repair,
verification, and independent rereview evidence close only the two factory
Findings.
