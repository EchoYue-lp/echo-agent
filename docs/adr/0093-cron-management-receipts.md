# ADR 0093: Cron definition edits and original occurrence observations

- Date: 2026-10-24
- Status: Implementation candidate for the accepted Plan07 scope; focused qualification in progress.
- Owner: framework scheduler / original journal authority.

## Context and layering

`CronTaskStore` owns persisted definitions. `SchedulerRunner` owns the live
mutation permit, control/admission lock and callback settlement. Its existing
occurrence `DeliveryLedger` and file journal own event sequence, attempt and
terminal facts. EKO consumes this runner through its existing thin adapter.
Workspace IDs, TaskRun links, GUI grouping and application continuation policy
remain in EKO; the framework does not acquire them.

The original management API has no atomic definition edit. Removing and adding
changes the task identity and leaves a race. Returning only a callback string
also discards the occurrence/attempt and turns typed journal errors into text.
Finally, the old summary update treats a replaced definition as an execution
failure even after the original callback completed. These are different facts.

## References and options

The Plan07 investigation compared APScheduler's stable job ID plus reschedule
model and Kubernetes resource-version conflict rejection:

- https://apscheduler.readthedocs.io/en/3.x/userguide.html#modifying-jobs
- https://kubernetes.io/docs/reference/using-api/api-concepts/

We use the existing definition generation as the CAS token, keeping the original
store and runner. We do not import another scheduler, a distributed authority,
remove/add editing, or application retry policy. Tokio's owned tasks keep the
original operation alive when only its observer disappears:
https://docs.rs/tokio/latest/tokio/task/struct.JoinHandle.html

A second execution queue or history store would create competing facts. Parsing
a callback's output to discover an application run would invent a contract.
Instead, all three public observations extend the current owner paths.

## Decision

`CronTaskEdit` contains only name, cron expression and prompt.
`SchedulerRunner::edit_task(expected, edit)` enters the existing owned mutation
and control lock, then the store mutation lock. The store checks only the
expected ID and `same_definition`: nonempty definition ID, or both legacy IDs
empty and equal creation timestamps. Other expected fields never overwrite the
latest status, control revision or summary.

An actual edit preserves task ID, creation timestamp and current status, assigns
a new definition ID, checked-increments the control revision, and clears the new
definition's last-run summary. A no-op returns the exact current snapshot without
writing. Invalid cron, missing/conflicting definition, exhausted revision,
closed runner, unavailable authority, no-commit and unknown-commit are explicit
errors. Unknown writes retain the original store poison; no edit is retried by
this API. Standalone `CronTaskStore::edit` obeys the existing runner permit.

The old admission check rejects an old definition/control token that has not
started its effect. An already `EffectStarted` callback keeps its captured
snapshot and owner. Replaced/deleted definitions yield `Superseded` only for
summary projection: no write and no stale cache insertion. Real I/O or poison is
still a projection error. The original occurrence result is never reassigned.
The derived scheduled watermark drops an old definition cache key when its new
definition is first admitted, without deleting journal facts.

`run_once_with_receipt` is the original manual admit/drain/callback-completion
path. `run_once` only maps its result and error for existing callers. A successful
receipt includes the captured task/definition/occurrence/time/trigger, exact
attempt and unparsed callback string, plus the original terminal `ApplyReceipt`.
The original append receipt may record degraded append durability when a later
original durability barrier succeeded; success requires that barrier too.

Errors retain known occurrence/attempt facts, including the original claim when
its commit or durability observation fails, actual callback observations,
projection errors and the original typed journal cause. Journal failure metadata
keeps NotCommitted/Committed/Unknown, batch identity, the original single/batch
receipt when one exists, and the original retry/reopen flags. A reopen flag is
absent when the original error does not supply a typed requirement, rather than
guessing a recovery policy from its text. These flags describe
the retained journal operation, not permission to start another manual request.
Physical `ExecutionNotSettled`, callback failure, projection failure and terminal
commit failure remain distinct. No output string is parsed and no application
run ID is added. Dropping an observer does not cancel the admitted callback owner.

`read_occurrence_history(cursor, limit)` returns original typed delivery events,
projecting only the private captured payload into `SchedulerOccurrenceSnapshot`.
It preserves metadata, sequence and physical batch ID. The cursor binds journal
generation, retained floor, initial snapshot upper sequence and continuation.
The default is 128 events; explicit limits must be 1 through 512. These are
resource policy bounds, not measured performance thresholds. Every page reuses
indexed bounded `replay_after`; scheduler writes use single-record frames or the
existing two-record recovery admission batch. No full-tail read, claim,
checkpoint/recovery, callback or history write occurs. An exhausted old snapshot
does not absorb newly appended events. Missing/corrupt sequences, wrong
generation/floor, invalid cursors and unavailable journals fail explicitly.
The hot projection's terminal retention is not complete history.

## File journal poison and recovery boundary

Tracked framework and EKO callers and existing fault tests were inspected.
EKO chat-log replay uses the separate segmented journal; event-bus replay is a
different API. `CheckpointedReducer::recover` already propagates journal read
errors. Existing file-journal unknown-commit tests drop the poisoned authority
before verified reopen. No current contract promising a trusted page from a
live poisoned file handle was found. A confirmed full-write degraded receipt or
retryable `sync_data` failure does not by itself poison the journal and remains
readable.

For nonzero reads the original file `replay_after` now checks poison under its
existing state lock before returning any page, including an otherwise empty
page. This is an observable read precondition: unknown commit requires the
original close/reopen/recovery and retained-batch lookup before trusted history.
The original error propagates; it is never translated into an empty page, new
generation or reset cursor. There is no second reader/validator/store and no new
recovery or retry policy. Zero-limit no-read behavior of the generic journal is
unchanged; the scheduler rejects zero limits itself.

## Validation and delivery

Regression definitions cover stale/no-op/legacy/invalid/exhausted edits, runner
permit and ambiguous writes; old unstarted and already-started occurrences;
independent manual identities and legacy mapping; callback/unsettled/projection/
terminal boundaries; snapshot/cursor/limit behavior and absence of read effects.
An after-claim-commit observation fault checks ordinary/manual and legacy
recovery batch paths against their actual committed claim and receipt, without
starting a callback. This control does not simulate a physical sync failure.
The journal fault case injects an actual unknown full append, rejects the live
page, then reopens and reads the original committed facts. Known degraded reads
keep their existing controls.

Two controlled restorations of prior behavior produced actual failing
assertions: a completed callback lost success after definition replacement, and
a poisoned file journal returned a trusted history page. These controls changed
only the original behavior branches in the candidate; they were not a build of
the whole earlier revision. The corresponding candidate controls preserve
terminal completion, untouched replacement summary and no-replay checks.

The candidate scheduler suite passed 82 tests and the journal suite passed 117,
with zero failures or ignored tests; the example check passed without warnings.
Focused lint gates are also required before a local implementation checkpoint.
This is focused evidence only. Full-gate, integrated consumer, native GUI and
external-service acceptance remain pending.
The existing `demo70_scheduler` example remains in the learning crate's executable
all-targets chain and must compile with the qualified framework. Documentation
is updated in both long-running-task chapters; the facade already re-exports the
scheduler module. No manifest, lock, shared README or root library changes are
needed. Website mirrors belong to the integration owner after qualified source
delivery. This adds no accepted SDK wire/route/member automatically.
