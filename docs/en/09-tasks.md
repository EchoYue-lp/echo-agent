# Revisioned Task Graphs

## Overview

echo-agent represents single tasks, todo-style lists, and dependency DAGs as
one revisioned task graph. There is no separate task-manager state machine.

- `TaskRevisionService` is the only CRUD, relation, validation, and revision
  authority.
- `RuntimeDagExecutor` is the only dependency execution kernel.
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
| `task_list` | Reads the current committed graph revision |

The first `task_create` call must carry every related task in one `tasks`
array. Later mutations include the current `base_revision`; stale writers fail
with a revision conflict instead of overwriting newer state.

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

`RuntimeDagExecutor<C>` repeatedly loads a committed `RuntimePlanSnapshot`
from its `RuntimeDagController`:

1. Validate the complete snapshot and detect cycles.
2. Compute the ready frontier from committed dependencies and statuses.
3. Atomically claim tasks with revision, attempt, spec hash, and unique claim id.
4. Dispatch a bounded, conflict-free Subagent wave.
5. Resolve or abandon every claim with compare-and-set semantics.
6. Reload at the next safe point so a newer revision can take effect.

The controller is a thin application adapter for persistence, Subagent
dispatch, review, and product-specific resource policy. It must not implement
a second ready-frontier loop or dependency state machine.

The executor handles transitive failure blocking, skip and pause states,
bounded retries, cancellation settlement, superseded claims, and stall
detection. Attempt-scoped claim identity prevents an old dispatch from
overwriting a reclaimed attempt.

## Projection and Progress

`TaskEvent` and `TaskProgress` are consumer-facing projections. Applications
may derive richer todo, evidence, and UI data from the committed task extension,
but the next runtime decision is always made from a committed revision loaded
through the canonical service/controller boundary.

See `demo48_personal_assistant` for direct service use and the
`runtime_executor` tests for a deterministic controller implementation.
