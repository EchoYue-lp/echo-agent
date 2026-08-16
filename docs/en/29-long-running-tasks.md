# Long-Running Work

## Separate Concerns

echo-agent exposes two complementary mechanisms:

| Mechanism | Authority | Use |
|-----------|-----------|-----|
| `TaskRevisionService` + `RuntimeDagExecutor` | Durable revisioned graph and dependency lifecycle | Multi-step Agent plans |
| `TaskSpawner` + `BackgroundTask<T>` | Process-local async handles | Polling, waiting for, or cancelling one future |

`TaskSpawner` is deliberately not a durable graph store. Restart recovery,
dependency relationships, claims, retries, and terminal settlement belong to a
`RevisionedTaskStore`/`RuntimeDagController` implementation.

## Background Futures

`BackgroundTask<T>` is a cloneable handle for one spawned future. It supports
non-blocking status reads, cancellation, and retryable waits with an optional
timeout.

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

## Progress

`PhasePlan` and `ProgressReporter` provide structured progress within one task.
`ProgressBridge` can project Agent callbacks into `TaskEvent::Progress` on a
lossy `TaskEventBus` for user-interface updates. These events are projections,
not a task-state authority; durable state remains the committed graph.

## Scheduled Triggers

The scheduler module provides cron-backed triggers. A scheduled callback may
start a background future or request a revisioned run, but the schedule itself
does not create another task graph or execution state machine.
