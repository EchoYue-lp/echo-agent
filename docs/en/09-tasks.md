# Revisioned Task Graphs

## Overview

echo-agent represents single tasks, todo-style lists, and dependency DAGs as
one revisioned task graph. There is no separate task-manager state machine.

- `TaskRevisionService` is the only CRUD, relation, validation, and revision
  authority.
- `RuntimeTaskService` is the only public dependency execution entry point.
- `TaskSpawner` tracks process-local background futures only; it does not own
  durable task relationships.
- A plan is an editable, versioned artifact over the graph. It is not an
  approval state machine.

## Task Model

Each committed node separates immutable specification from mutable execution
state:

```rust
pub struct Task {
    pub spec: TaskSpec,
    pub execution: TaskExecution,
}
```

`TaskSpec` contains only the task id, title, description, dependencies, retry
limit, and an opaque product extension. `TaskExecution` contains status, retry
count, failure fingerprint, and an optional attempt-scoped claim. Coding kinds,
Subagent selection, files, tools, checks, review, and UI projections belong in
an application-owned typed extension.

The shared lifecycle includes `Pending`, `Running`, `Blocked`, `Retrying`,
`Paused`, `Completed`, `Failed`, `TimedOut`, `Skipped`, and `Cancelled`.
Transitions are validated by `TaskStatus::transition_to`.

## Canonical CRUD Service

The default framework Agent registers three task tools:

| Tool | Contract |
|------|----------|
| `task_create` | Atomically creates one complete graph, or appends with `base_revision` |
| `task_update` | Applies one optimistic patch to specs, relations, order, skip, or status |
| `task_list` | Reads a bounded page of the current committed graph revision; accepts `limit` (1–100), an opaque `cursor`, and `detail_level` (`summary` or `full`) |

The first `task_create` call must carry every related task in one `tasks`
array. Later mutations include the current `base_revision`; stale writers fail
with a revision conflict instead of overwriting newer state.

`task_list` defaults to a 20-task summary page. The result metadata includes
`page.next_cursor`, `page.returned`, `page.total`, and `page.truncated` when more
tasks remain. Reuse the cursor with the same committed graph and limit; a query
or snapshot change invalidates it. `detail_level=full` adds dependencies, retry
counts, and non-empty lifecycle detail without creating a second store or
reducer.

Applications that need durable storage or product policy inject their own
`RevisionedTaskStore` and `TaskToolPolicy`:

```rust,ignore
use echo_agent::tasks::{
    DefaultTaskToolPolicy, InMemoryRevisionedTaskStore, TaskRevisionService,
};
use std::sync::Arc;

let service = Arc::new(TaskRevisionService::new(
    Arc::new(InMemoryRevisionedTaskStore::new()),
    Arc::new(DefaultTaskToolPolicy::new("run-42")),
));

let agent = ReactAgentBuilder::new()
    .model("qwen3-max")
    .task_revision_service(service)
    .build()?;
```

The policy adapter may resolve scope and attach a lossless product extension,
but generic patch semantics and DAG validation stay in the framework.

## Runtime Execution

`RuntimeTaskService<C>` repeatedly loads a committed `RuntimePlanSnapshot`
through its thin `RuntimeDagController` adapter:

1. Validate the complete snapshot and detect cycles.
2. Compute the ready frontier from committed dependencies and statuses.
3. Atomically claim tasks with revision, attempt, spec hash, and unique claim id.
4. Dispatch a bounded, conflict-free Subagent wave.
5. Resolve or abandon every claim with compare-and-set semantics.
6. Settle typed cancellation or resumable pause receipts at the wave boundary.
7. Reload at the next safe point so a newer revision can take effect.

The controller is a thin application adapter for persistence, Subagent
dispatch, review, and product-specific resource policy. It must not implement
a second ready-frontier loop or dependency state machine.

The service handles transitive failure blocking, skip and pause states, bounded
retries, cancellation settlement, superseded claims, and stall detection.
Dispatch resolution preserves `Failed` and `TimedOut` as distinct terminal
states. A requeue request declares which state applies if its retry budget is
exhausted, and the framework commits that state through the same exact-claim
compare-and-set path; persistence adapters must not reinterpret it afterward.
Dependency failure is a typed `DagDependencyState` projection; it is not
persisted as `TaskStatus::Blocked`, so retrying an ancestor removes the derived
block automatically. `Blocked` remains available for explicit product policy,
such as review or missing input. A paused claim clears ownership and resumes to
Pending without consuming retry budget. A Skipped dependency is carried to the
Subagent as a typed waiver rather than a fabricated output. Attempt-scoped
claim identity prevents an old dispatch from overwriting a reclaimed attempt.

## Projection and Progress

`TaskEvent` and `TaskProgress` are consumer-facing projections. Applications
may derive richer todo, evidence, and UI data from the committed task extension,
but the next runtime decision is always made from a committed revision loaded
through the canonical service/controller boundary.

See `tests/facade_smoke.rs` for public service construction and the private
`runtime_executor` tests for deterministic controller behavior.
