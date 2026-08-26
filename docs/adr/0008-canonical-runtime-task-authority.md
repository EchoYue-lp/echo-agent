# ADR 0008: Canonical Runtime Task Authority

- Status: Accepted
- Date: 2026-08-26
- Owners: `echo-orchestration/tasks`, `agent/subagent`

## Context

The framework historically had overlapping task models and execution loops:
revisioned task relations, manager plans, team-specific nodes, and legacy task
executors could each decide status, readiness, retry, or settlement. Parallel
authorities make dynamic graph edits and restart recovery ambiguous. A stale
executor can overwrite a newer revision, and UI/todo projections can drift into
runtime inputs.

Industry agent systems keep plans as inspectable artifacts and keep execution
lifecycle separate. Cursor Plan Mode produces an editable plan before Agent
execution. OpenAI Codex exposes thread, turn, and item lifecycle events rather
than encoding plan approval as additional task-run states. Both patterns favor
one executable graph/lifecycle authority with projections around it, not a
second state machine per surface or collaboration role.

References:

- <https://cursor.com/docs/agent/plan-mode>
- <https://cursor.com/docs/subagents>
- <https://github.com/openai/codex/blob/main/codex-rs/app-server/README.md>

## Options Considered

1. Preserve each legacy executor and synchronize them through adapters. This
   keeps multiple ready frontiers and cannot make settlement atomic.
2. Move EKO worktree, reviewer, approval, and UI fields into a large framework
   task runtime. This would bind the reusable framework to one product.
3. Keep one revisioned framework graph and one runtime service, while making
   application adapters provide persistence, dispatch, and product policy.

## Decision

The framework has one revisioned TaskRun graph and two non-overlapping service
boundaries over that same authority.

- `TaskRevisionService` is the only CRUD, relation, validation, ordering, and
  revision commit authority. `task_create`, `task_update`, and `task_list` use
  it directly.
- `RuntimeTaskService` is the only public dependency execution entry point. Its
  internal `RuntimeDagExecutor` owns validation, ready-frontier computation,
  bounded waves, retry bookkeeping, cancellation/pause safe points, stall
  detection, and terminal settlement.
- `TaskSpec` contains portable specification fields and an opaque `extension`.
  `TaskExecution` contains framework lifecycle, retry, failure, and exact claim
  state. Application fields must round-trip losslessly through the extension.
- Claims bind revision, attempt, stable spec hash, and unique identity.
  Compare-and-set settlement rejects stale or superseded Subagent results.
- A controller adapter may atomically persist snapshots, dispatch Subagents,
  and apply product review/resource policy. It must not own another DAG loop,
  validator, ready frontier, retry state machine, or task store.
- `TaskSpawner` remains a process-local future tracker, not a durable relation
  authority. Team APIs compile into the canonical graph and dispatch through
  the same Subagent registry/executor.
- A plan remains an editable, versioned artifact. Todo and surface progress are
  read-only projections and cannot become execution inputs or separate stores.

Dependency DAGs, revision safe points, claims, retry, cancellation, and generic
Subagent dispatch are reusable framework mechanisms. EKO worktrees, reviewer
policy, DomainProfile, file authority, approvals, and GUI/TUI/CLI projection
remain application concerns. The adapter boundary is intentionally thin.

## Consequences

Dynamic patches take effect at a reload safe point, and stale attempts cannot
overwrite a reclaimed task. Retry, pause, skip, cancellation, and dependency
failure have one semantic source across Team and direct Task APIs.

Legacy `TaskManager`, `TaskStore`, `TaskExecutor`, `TaskScheduler`, manager-owned
ready loops, and Team-specific checkpoint nodes were removed. Applications
must adapt product data through `TaskSpec::extension` and the controller rather
than restoring compatibility fields or parallel task CRUD.

## Verification

Tests cover create/update revision conflicts, complete graph validation,
dynamic revision reload, exact claim identity, stale and superseded settlement,
bounded/conflict-aware waves, retry exhaustion, explicit retry, resumable pause,
cancellation settlement, skip waivers, transitive dependency blocking, stall
detection, Team adaptation, extension round trips, and public facade
construction. Documentation and facade smoke tests assert that the canonical
services are the exported framework entry points.
