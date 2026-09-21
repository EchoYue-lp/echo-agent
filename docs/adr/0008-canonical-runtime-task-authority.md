# ADR 0008: Canonical Runtime Task Authority

- Status: Accepted
- Date: 2026-08-26
- Owners: `echo-orchestration/tasks`, `agent/subagent`

## Status

Accepted.

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
- <https://www.rfc-editor.org/rfc/rfc9110.html#name-if-match>
- <https://kubernetes.io/docs/reference/using-api/api-concepts/#resource-versions>

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
- Exact Subagent identity and live control are derived from that claim under
  [ADR 0058](0058-task-claim-subagent-attempt-control.md); live control remains
  a recoverable projection and never becomes a second durable task authority.
- Relation patches keep graph revision and runtime execution as separate
  preconditions. `TaskGraphCommit.expected_executions` captures the exact
  `TaskId -> TaskExecution` map observed before applying a patch. A store first
  validates the relation revision, then rejects the commit if any claim,
  retry, settlement, status, error, or task membership changed. Patch operations
  cannot exempt their target from this check. Create and legacy/direct commits
  may omit the stronger precondition; the canonical `TaskRevisionService`
  patch path never does.
- A controller adapter may atomically persist snapshots, dispatch Subagents,
  and apply product review/resource policy. It must not own another DAG loop,
  validator, ready frontier, retry state machine, or task store.
- `TaskSpawner` remains a process-local future tracker, not a durable relation
  authority. Team APIs compile into the canonical graph and dispatch through
  the same Subagent registry/executor.
- A plan remains an editable, versioned artifact. Todo and surface progress are
  read-only projections and cannot become execution inputs or separate stores.
- `AgentCheckpoint.current_plan` remains decodable and round-trippable in the
  public File/SQLite RuntimeStateStore contracts for historical checkpoints.
  ReactAgent neither restores it into a private plan state nor captures it at
  new safe points. A legacy value cannot become an alternate Task graph writer
  or override a later graph revision.

Dependency DAGs, revision safe points, claims, retry, cancellation, and generic
Subagent dispatch are reusable framework mechanisms. EKO worktrees, reviewer
policy, DomainProfile, file authority, approvals, and GUI/TUI/CLI projection
remain application concerns. The adapter boundary is intentionally thin.

## Consequences

Dynamic patches take effect at a reload safe point, and stale attempts or
stale relation patches cannot overwrite a claimed, retried, or settled task.
Runtime mutations do not increment the relation revision; exact execution
preconditions provide the equivalent of an `If-Match` validator without
merging the two lifecycle authorities. Retry, pause, skip, cancellation, and
dependency failure have one semantic source across Team and direct Task APIs.

Legacy `TaskManager`, `TaskStore`, `TaskExecutor`, `TaskScheduler`, manager-owned
ready loops, and Team-specific checkpoint nodes were removed. Applications
must adapt product data through `TaskSpec::extension` and the controller rather
than restoring compatibility fields or parallel task CRUD.
Retiring the ReAct plan projection does not delete the public checkpoint field
or rewrite existing File/SQLite records; consumers can still inspect historical
data, while only the revisioned Task service can publish current task plans.

## Verification

Tests cover create/update revision conflicts, complete graph validation,
dynamic revision reload, exact claim identity, stale and superseded settlement,
bounded/conflict-aware waves, retry exhaustion, explicit retry, resumable pause,
cancellation settlement, skip waivers, transitive dependency blocking, stall
detection, deterministic load/claim/commit and load/settle-or-retry/commit
interleavings, Team adaptation, extension round trips, and public facade
construction. Documentation and facade smoke tests assert that the canonical
services are the exported framework entry points.
