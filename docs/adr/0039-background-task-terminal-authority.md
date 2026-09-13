# ADR 0039: BackgroundTask uses one process-local terminal authority

- Date: 2026-09-14
- Owners: `echo-orchestration/tasks/background_task`

## Status

Accepted.

## Context

`TaskSpawner` manages process-local asynchronous futures. It is deliberately
separate from the revisioned, durable Task graph. Its public handle promises
status inspection, cancellation, retry-safe waiting, and cloneability.

The existing implementation splits one task across a Tokio status lock, a
result cell, `Notify`, a terminal boolean, and an unobserved outer JoinHandle.
That creates several contradictory observations:

- `wait` checks the result before registering for `notify_waiters`, so a
  concurrent completion can be missed.
- one waiter consumes the result and another waiter can sleep forever instead
  of receiving the documented status-based response;
- the terminal boolean cannot distinguish Completed, Failed, and Cancelled, so
  type-erased fallback can report the wrong terminal;
- cancellation and timeout begin only after semaphore admission;
- zero concurrency can leave a task Pending forever;
- a panic in the user future terminates the outer task before status and result
  are published.

The type also does not implement Clone even though source and bilingual public
documentation say that it does.

Tokio documents that concurrent receivers must call `Notified::enable` before
checking shared state to prevent lost wakeups. Tokio also defines JoinHandle as
owned join permission: dropping detaches the task, awaiting `&mut JoinHandle`
is cancel-safe, panic is returned as JoinError, and observed completion follows
task destruction. OpenAI Codex routes background request results back through
one app event reducer, reinforcing the distinction between one result commit
and repeatable state observation.

- <https://github.com/tokio-rs/tokio/blob/master/tokio/src/sync/notify.rs>
- <https://docs.rs/tokio/latest/tokio/task/struct.JoinHandle.html>
- <https://github.com/openai/codex/blob/main/codex-rs/tui/src/app/background_requests.rs>

## Options Considered

1. Only replace `notify_waiters` with `notify_one`. Concurrent receivers still
   compete for one permit and status/result publication remains split.
2. Require `T: Clone` so every waiter receives a value. This breaks existing
   generic callers and changes the public contract.
3. Change `wait` to return `Arc<T>`. This also breaks the public signature and
   language contracts.
4. Keep multiple status/result flags and add ordering patches. The fallback
   state remains a second authority and future exits can still miss a path.
5. Store status and the single-consumer result in one shared state, make the
   handle cloneable, register waiters before inspection, and supervise the user
   future through an awaited child-task JoinHandle.

## Decision

Choose option 5.

- `BackgroundTaskHandleState<T>` atomically stores the current
  `BackgroundTaskStatus` and optional `Result<T>` behind one short synchronous
  mutex. No guard crosses an await or user callback.
- A private type-erased status source reads that same state for list, status,
  and retention. The parallel terminal boolean and lock-busy synthetic status
  are removed.
- `BackgroundTask<T>` implements Clone without requiring `T: Clone`. Clones
  share identity, state, notification, and cancellation.
- The result remains single-consumer. The first successful waiter takes `T`;
  later waiters immediately receive a status-derived terminal error. Failed
  and Cancelled observations preserve their terminal meaning.
- Every wait call creates and enables `Notified` before inspecting state. Its
  optional timeout is one absolute deadline for that call and is not reset by
  notifications. Timing out never consumes the result.
- Spawn computes one optional absolute deadline at acceptance. Semaphore
  admission and execution both select with deterministic precedence:
  cancellation, deadline, then normal readiness.
- `max_concurrent=0` retains the compatible `TaskSpawner::new -> Self` shape,
  but each spawned handle settles immediately as Failed configuration rather
  than staying Pending or silently increasing capacity.
- After admission, the user future runs in a child execution task. The
  supervisor awaits its JoinHandle. Cancellation or timeout aborts and awaits
  the child before publishing terminal; JoinError panic maps to Failed.
- A single settlement helper writes result and status once under the state
  lock, then wakes waiters after releasing it.
- `is_panicked` reads typed panic provenance committed atomically with the
  persistent terminal status: nonterminal is unknown, caught execution-task
  panic is true, and other terminals are false.

## Result And Observer Contract

`wait -> Result<T>` remains a consuming operation because arbitrary `T` is not
cloneable. Cloneability applies to the handle and repeatable status observation,
not duplication of the returned value.

| State | First waiter | Later waiter |
| --- | --- | --- |
| Completed | original `T` | immediate completed/result-consumed error |
| Failed | original `ReactError` | immediate failure reconstructed from status |
| Cancelled | cancellation error | immediate cancellation error |

Timeout is not a task terminal and leaves this table unchanged.

## Deadline And Cancellation Contract

The spawner deadline starts when `spawn` accepts the future. Capacity waiting
therefore consumes the same budget as execution. Cancellation is persistent
and wins when cancellation, deadline, and normal readiness are observed in the
same poll. Deadline wins over normal completion in the same poll.

Queued cancellation or deadline never starts the user future. Execution
cancellation and timeout do not publish terminal until the child task has been
aborted and joined.

## Consequences

- No waiter can miss terminal publication or wait again after terminal state.
- Status/list/prune cannot reinterpret Failed or Cancelled as Completed.
- Public handle cloneability matches documentation.
- Timeout and cancellation cover both admission and execution.
- User-future panic becomes observable Failed state rather than a permanently
  Running/Pending handle.
- A second waiter does not receive `T`; callers that need shared values must
  choose an explicitly shareable result type such as `Arc<U>` as `T`.
- Public method and state shapes remain unchanged.

## SDK Impact

The only new Rust public identity is `Clone for BackgroundTask<T>`. The SDK
inventory classifies Rust trait implementations as `language_intrinsic`; no
TypeScript, Python, or Java facade method is added. Canonical inventory,
parity manifest, source contract, and shared digests must be regenerated and
reviewed for that exact change.

## Compatibility And Rollback

Existing constructors, configuration fields, status variants, method
signatures, result type, and process-local scope remain source compatible.
Queue time now counts toward the existing default timeout, and zero concurrency
returns a terminal configuration error instead of hanging.

Rollback must restore state, wait registration, supervision, and SDK inventory
together. Restoring the terminal boolean, detaching the child task, or claiming
cloneability only in documentation is not an acceptable partial rollback.

## Verification

Deterministic tests cover concurrent clone waiters, timeout retry, exact
type-erased terminal states, queued cancellation, queued deadline, zero
concurrency, normal success/failure, execution cancellation/timeout, and child
settlement. Panic mapping is reviewed against Tokio JoinHandle and exercised
without adding a panic-producing API to project tests when a safe existing
fixture is available.

The final gates include BackgroundTask/TaskSpawner tests, affected workspace
tests, formatter, strict Clippy including panic policy, feature checks,
bilingual documentation, SDK regeneration/checks, semantic strict/change
evidence, Issue reconciliation, and independent review.
