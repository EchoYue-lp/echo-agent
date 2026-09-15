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

The scheduler is a framework capability.  It must not acquire a second Task
DAG, delivery ledger, or application-specific retry policy.  The open Findings
#84, #85, and #86 require a local authority and lifecycle repair, but the
source does not establish whether missed occurrences must be replayed or
whether callback delivery is at-most-once or at-least-once across a process
crash.

## Options

1. Treat every task copied by `tick` as irrevocably fired.  This preserves the
   old race and makes a successful disable/remove unable to prevent a callback
   that has not started.
2. Add a pre-callback admission gate and a stable definition identity.  A
   control operation that commits before admission suppresses the occurrence;
   once admission wins, the callback is allowed to settle.  This closes the
   local race without claiming a cross-crash delivery guarantee.
3. Add a durable occurrence/claim ledger and choose an explicit retry policy.
   This would be a new persistence protocol and requires a product decision on
   at-most-once versus at-least-once delivery, which is outside these Findings.

## Decision

Choose option 2 for this repair slice:

- The store remains the single durable authority for task definitions and
  `last_run` projection.  The runner list is a derived cache and is refreshed
  from successful store mutations; it is never updated ahead of the store.
- `CronTask.id` is unique within a store.  Add and load/migration paths reject
  duplicate IDs instead of merging, partially updating, or deleting an
  ambiguous set of definitions.  A store-backed migration never overwrites an
  existing destination value.
- A tick reserves an occurrence using task definition identity (`id` plus
  `created_at`) and its scheduled timestamp.  Immediately before invoking the
  callback, the runner takes the task write lock and verifies that the same
  definition still exists and is enabled.  Disable/remove that commits before
  this admission gate suppresses the callback.  The callback is admitted while
  the same lock is held; a later control operation does not retract an admitted
  invocation.
- Callback settlement updates the store only for the captured definition
  identity, then applies the returned snapshot to the cache if that definition
  is still present.  A removed and recreated definition cannot receive a stale
  callback result.
- This ADR does not define crash recovery, durable occurrence claims, replay,
  or exactly-once delivery.  Those remain an explicit semantic decision before
  adding a scheduler delivery ledger.

## Consequences

Task listing and scheduler polling observe the same committed definition and
last-run state in the normal runner path.  Control operations have a clear
linearization point relative to callback admission, and duplicate IDs fail
closed at startup, import, and programmatic add.  A callback that was already
admitted may still complete after disable/remove, by design.

The scheduler still has no durable claim before an external callback runs.  A
process crash can therefore leave an occurrence absent from `last_run` and the
next process may fire it again or miss it, depending on the polling window.
Selecting and implementing that policy requires a separate decision and
Finding; this repair must not imply stronger delivery semantics.

## References

- GitHub Issues #84, #85, and #86.
- `echo-orchestration/src/scheduler/cron_task.rs`.
- `echo-orchestration/src/scheduler/runner.rs`.
- `.echo-semantic/findings/finding.scheduler-cache-delivery.md`.
- `.echo-semantic/findings/finding.scheduler-control-fire-race.md`.
- `.echo-semantic/findings/finding.scheduler-task-id-uniqueness.md`.
