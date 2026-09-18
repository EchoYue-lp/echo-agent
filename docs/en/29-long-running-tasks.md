# Long-Running Work

## Separate Concerns

echo-agent exposes two complementary mechanisms:

| Mechanism | Authority | Use |
|-----------|-----------|-----|
| `TaskRevisionService` + `RuntimeTaskService` | Durable revisioned graph and dependency lifecycle | Multi-step Agent plans |
| `TaskSpawner` + `BackgroundTask<T>` | Process-local async handles | Polling, waiting for, or cancelling one future |

`TaskSpawner` is deliberately not a durable graph store. Restart recovery,
dependency relationships, claims, retries, and terminal settlement belong to a
`RevisionedTaskStore`/`RuntimeDagController` implementation.

## Background Futures

`BackgroundTask<T>` is a cloneable handle for one spawned future. It supports
non-blocking status reads, cancellation, and retryable waits with an optional
timeout. Clones share one lifecycle and cancellation scope; they do not clone
`T`. The first terminal waiter consumes the result. A later waiter returns
immediately with the persistent Completed, Failed, or Cancelled disposition
instead of waiting for another notification.

```rust,ignore
use echo_agent::tasks::{TaskSpawner, TaskSpawnerConfig};
use std::time::Duration;

let spawner = TaskSpawner::new(TaskSpawnerConfig::default());
let handle = spawner.spawn("fetch-data", async {
    Ok("result".to_string())
});

println!("{:?}", handle.status().await);
let result = handle.wait(Some(Duration::from_secs(30))).await?;
```

The process-local lifecycle is:

```text
Pending -> Running -> Completed
                   -> Failed
                   -> Cancelled
```

The spawner bounds concurrency with a semaphore and can list or cancel handles
that still exist in the current process. It does not serialize future closures
or claim they can resume after restart.

`default_timeout_secs` is one absolute budget from spawn acceptance through
capacity admission and execution. Cancellation and the deadline are observed
while queued as well as while running. A queued task that is cancelled never
starts; `max_concurrent = 0` produces a Failed handle rather than a permanently
Pending task. During execution, cancellation or timeout aborts and joins the
child task before terminal status is published. A child-task panic is converted
to Failed and is observable through `is_panicked()`.

Each individual `wait(timeout)` also uses one absolute timeout for that call;
notifications do not restart its budget, and a wait timeout never consumes the
eventual task result. Status, result delivery, list, and retention all read the
same process-local terminal state.

## Durable DAG Execution

For restart-safe Agent work, persist the task graph behind the canonical
`RevisionedTaskStore` and implement the narrow `RuntimeDagController` adapter.
The framework executor owns generic mechanics:

- complete-snapshot validation and cycle rejection;
- dependency ready-frontier calculation;
- bounded Subagent waves;
- attempt-scoped atomic claims and ABA protection;
- retry, skip, pause, transitive blocking, and cancellation settlement;
- revision reload at execution safe points;
- fail-closed handling of invalid snapshots and stalled graphs.

Applications own product-specific persistence, dispatch, review, and resource
selection in the controller. A controller returns committed snapshots and uses
compare-and-set operations for claims and results; it does not duplicate the
DAG loop.

### Exact Attempt Control

Every claimed task derives its Subagent execution identity from
`TaskClaim::execution_id(run_id, task_id)`. The runtime reserves that identity
before capacity admission and passes one task-child cancellation token through
dispatch, events, terminal settlement, and live control. Cancelling the run
propagates to every child; `request_attempt_interrupt` cancels only the exact
claim and rechecks the durable claim after projecting the request.

`TeamRuntimeHandle` retains one Team run ID, task store, `RuntimeTaskService`,
and Subagent control registry. Keep the handle when execution needs concurrent
snapshot inspection or exact interruption:

```rust,ignore
let handle = team.runtime_handle().await?;
let execution = team.execute("review the repository");

let snapshot = handle.snapshot().await?;
let task = snapshot.tasks.iter().find(|task| task.execution.claim.is_some())?;
let claim = task.execution.claim.as_ref()?;
handle
    .request_attempt_interrupt(&task.spec.id, claim)
    .await?;
```

Live pending/reserved/active/settled entries are process-local projections.
`RuntimeTaskService::reconcile_attempt_control` rebuilds their validity from
durable TaskClaims during recovery. Durable command replay belongs to the Host;
it must replay the exact claim identity rather than an execution name alone.
If a joined attempt cannot prove its durable terminal, waiters receive a typed
authority error instead of waiting indefinitely. Recovery publishes the
durable supersede to stale joined waiters and retires local supervisor state
before waiting for best-effort live-controller cleanup; active attempts remain
bound until their targeted abort and canonical join complete.
Custom Team integrations implement `TeamDispatchController`, which keeps
dispatch, reservation, interrupt, cleanup, and reconciliation on one live
control scope. Each retained runtime has a unique `handle_id`; one business run
may therefore expose multiple handles without overwriting control authority.
Active handles are retained regardless of the history limit; only settled
handles enter the bounded 64-entry retention set.
Caller-supplied runtimes that need concurrent control construct one
`TeamRuntimeServiceHandle<R>` and pass it to `execute_team_on_runtime_service`;
the binding prevents graph/output authority and execution/CAS authority from
coming from different runtime instances.

Non-Team `RuntimeDagController` adapters use
`SubagentExecutor::attempt_control_handle` to bind reservation, dispatch,
interrupt projection, cleanup, and reconciliation to one process scope. The
handle derives identity only from `TaskSubagentContext` or `TaskClaim`; it does
not load a task store or validate claim currency. Public commands must still
enter through the same `RuntimeTaskService`, which performs the durable
precondition before invoking the controller's live hook.

## Progress

`PhasePlan` and `ProgressReporter` provide structured progress within one task.
`ProgressBridge` can project Agent callbacks into `TaskEvent::Progress` on a
lossy `TaskEventBus` for user-interface updates. These events are projections,
not a task-state authority; durable state remains the committed graph.

## Scheduled Triggers

The scheduler module provides cron-backed triggers. A scheduled callback may
start a background future or request a revisioned run, but the schedule itself
does not create another task graph or execution state machine.
