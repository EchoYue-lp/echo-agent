# ADR 0042: Scheduler Occurrence Authority

## Status

Accepted

## Context

`CronTaskStore` is the durable authority for scheduled task definitions, while
`SchedulerRunner` keeps an in-memory list for polling.  The old implementation
could let those representations diverge after a callback updated the store,
could let a disabled or removed task fire after a tick had copied it, and
silently accepted duplicate task IDs.  A callback result could then be written
to a task definition that had already been removed and recreated with the same
ID.

The scheduler is a framework capability. It must not acquire a second Task DAG
or an application-specific retry policy. Findings #85 and #86 required a local
definition/admission repair. Finding #84 additionally requires the callback
boundary to reuse the framework's existing product-neutral `DeliveryLedger`
instead of inventing a scheduler-specific claim reducer.

## Industry Basis

- Kubernetes documents CronJob creation as approximate: two Jobs or no Job may
  occur for one schedule, so Job handlers should be idempotent. It also exposes
  the original scheduled timestamp as stable occurrence context.
- Temporal documents the ambiguous crash window where an Activity completes
  but its owner dies before reporting completion. The Activity is retried, may
  execute more than once, and should pass a stable idempotency key to the
  external service.

Both systems separate scheduling/delivery facts from exactly-once external
effects. This framework follows that mature boundary: a durably persisted occurrence is
delivered at least once, while the callback owns effect idempotency.
SQLite's isolation documentation also describes serializing writes through a
single writer. The scheduler applies that ownership principle at the public
definition Store boundary while a runner is live.

## Options

1. Treat every task copied by `tick` as irrevocably fired.  This preserves the
   old race and makes a successful disable/remove unable to prevent a callback
   that has not started.
2. Add a pre-callback admission gate and a stable definition identity.  A
   control operation that commits before admission suppresses the occurrence;
   once admission wins, the callback is allowed to settle.  This closes the
   local race without claiming a cross-crash delivery guarantee.
3. Compose the existing framework `DeliveryLedger` with scheduler occurrence
   identity and choose at-least-once crash recovery. This preserves one generic
   claim/attempt/settlement reducer and exposes the idempotency identity to the
   callback.
4. For definition writes, either publish every public Store mutation into the
   runner cache or assign the live runner a single mutation permit. A permit
   keeps the existing cancellation-safe runner write path and avoids a second
   cache publication path for standalone Store callers.

## Decision

Choose option 2 for the #85/#86 repair, option 3 for durable #84 delivery,
and option 4 for the remaining #84 definition writes:

- The store remains the single durable authority for task definitions and
  `last_run` projection.  The runner list is a derived cache and is refreshed
  from successful store mutations; it is never updated ahead of the store.
- A runner claims a Store mutation permit under the Store's existing mutation
  lock before loading its initial cache. Public Store writes through retained
  clones, same-path handles, or Store-backed handles sharing one backend instance
  fail while that runner is live. Backend identity survives `with_path()` and
  is shared by independently constructed handles using the same `Arc<Store>`;
  legacy migration checks the destination backend owner first and the default
  file definition path owner before reading or removing the source when a
  migration is actually needed.
  Runner management
  and last-run writes carry the private permit through their owned settlement
  task. Read-only Store calls remain available. Once the runner and any
  in-flight owned mutations release the permit, standalone Store writes resume.
  The shared path and backend owner registries make this an in-process
  contract; direct backend edits, distinct backend objects for the same
  physical store, and other processes still require explicit reload or
  external coordination.
- `CronTask.id` is unique within a store.  Add and load/migration paths reject
  duplicate IDs instead of merging, partially updating, or deleting an
  ambiguous set of definitions. Every add assigns a fresh store-owned
  `definition_id`, so a caller cannot resurrect an old incarnation by adding
  an exact clone after removal. Legacy definitions without this field use
  `created_at` until re-added. A store-backed migration never overwrites an
  existing destination value.
- A tick reserves an occurrence using task definition identity (`id` plus the
  immutable `definition_id`, with `created_at` as the legacy fallback), its
  durable `control_revision`, and its scheduled timestamp.
  Every status control increments the persisted revision. Immediately before
  invoking the callback, the runner takes the task lock and verifies that the
  same definition is enabled at the captured revision. Disable followed by
  enable before this admission gate therefore suppresses the stale occurrence
  instead of passing an ABA check. The callback is admitted while the same lock
  is held; a later control operation does not retract an admitted invocation.
- Callback settlement updates the store only for the captured definition
  identity, then applies the returned snapshot to the cache if that definition
  is still present.  A removed and recreated definition cannot receive a stale
  callback result.
- The scheduler composes `echo_state::delivery::DeliveryLedger` over a
  `SyncData` file journal and checkpoint. It does not define another scheduler
  ledger enum, store, or reducer. The in-process authority registry rejects a
  second live runner for the same journal so independent projections cannot
  race. This single-owner fence is not the durability or exactly-once
  mechanism.
- A scheduled occurrence uses `CronTask.id` as its route. Its payload retains
  the captured definition, trigger, scheduled timestamp, and durable control
  revision. Its stable
  `occurrence_id` is the SHA-256 identity of the schema version, public task ID,
  store-owned definition ID, legacy creation timestamp, and scheduled timestamp. Manual
  occurrences use a fresh UUID. `DeliveryLedger` remains the sole owner of the
  monotonic attempt and opaque attempt ID.
- Persist, claim, and `EffectStarted` are confirmed through the journal's
  durability barrier before the callback future is created. Callback success
  and known failure are terminal settlements. Shutdown or recovery of an
  `EffectStarted` record writes `OutcomeUnknown` with a retry settlement; the
  next claim keeps the occurrence ID and receives a new attempt ID. A dropped
  callback future is detected by an in-process owner guard and enters the same
  durable reconciliation path instead of wedging the FIFO.
- A cancelled runner does not claim pending occurrences or construct new
  callbacks. Cancellation is checked again after acquiring the control lock;
  unstarted durable work remains recoverable by the next runner.
- `SchedulerInvocation` exposes the occurrence and attempt identities through
  `OccurrenceFireFn`. The old `FireFn(CronTask)` constructor is a thin source
  compatibility adapter over the same ledger and settlement path; it does not
  own another terminal state.
- File-backed stores derive journal/checkpoint names by appending a suffix to
  the complete definition filename, so different extensions cannot alias.
  The generic `Store` trait has no stable backend identity, so Store-backed
  schedulers must supply an explicit stable path anchor with `with_path()`;
  missing identity fails closed rather than sharing the default home journal.
- `last_run_at` and `last_result` remain task-definition projections. They are
  updated before the terminal ledger settlement. If that projection fails
  after the callback is known to have completed, the occurrence still settles
  from the known callback result and is not falsely replayed as unknown.
- The guarantee is deliberately at-least-once for occurrences already
  committed to the ledger. A crash after an external effect but before terminal
  settlement can invoke the callback again. Exactly-once requires the callback
  or its target system to deduplicate by `occurrence_id`; neither a process
  lock nor `last_fired` can provide it.
- Cron polling remains approximate and does not replay arbitrary schedules
  while the scheduler was offline. A separate misfire/deadline policy would be
  required before changing that behavior.

## Consequences

Task listing and scheduler polling observe the same committed definition and
last-run state in the normal runner path. Control operations have a clear
linearization point relative to callback admission, and duplicate IDs fail
closed at startup, import, and programmatic add. A callback that was already
admitted may still complete after disable/remove, by design.
Removing and adding an exact `CronTask` clone creates a new definition
incarnation: queued old occurrences are dropped before admission, and an old
callback result cannot update the replacement's last-run projection.

After an occurrence is committed, startup drains its FIFO frontier before the
first 30-second cron tick. Owner loss is observable as `OutcomeUnknown`; replay
retains the stable occurrence identity and advances the attempt identity.
Known callback failure remains terminal to preserve existing scheduler policy.
Applications that need retries for a returned business failure implement that
policy outside this generic delivery contract.

`echo-orchestration` now depends one way on `echo-state`; `echo-state` continues
to depend only on `echo-core`, so no reverse dependency or feature cycle is
introduced. The journal is append-only; physical compaction remains a follow-up
operational concern and does not weaken recovery correctness.

The new `CronTask.definition_id`, `CronTask.control_revision`,
`SchedulerInvocation`, `SchedulerTrigger`,
`OccurrenceFireFn`, and occurrence-aware constructor are intentional public
framework contracts. The SDK has been extracted into an independent repository;
its owner must update the consumer contracts there. No extracted SDK files or
shared generated inventories are restored by this scheduler integration.

An embedding application that constructs a Store-backed scheduler supplies its
own stable data-root anchor and validates its callback idempotency. Those
consumer adaptations do not control the framework Finding's completion.

## References

- GitHub Issues #84, #85, and #86.
- [Kubernetes CronJob limitations](https://kubernetes.io/docs/concepts/workloads/controllers/cron-jobs/#job-creation).
- [Temporal Activity idempotency and retry](https://docs.temporal.io/activity-definition#idempotency).
- [SQLite Isolation](https://www.sqlite.org/isolation.html).
- `echo-state/src/delivery.rs` and `echo-state/src/journal/file.rs`.
- `echo-orchestration/src/scheduler/cron_task.rs`.
- `echo-orchestration/src/scheduler/runner.rs`.
- `.echo-semantic/findings/finding.scheduler-cache-delivery.md`.
- `.echo-semantic/findings/finding.scheduler-control-fire-race.md`.
- `.echo-semantic/findings/finding.scheduler-task-id-uniqueness.md`.
