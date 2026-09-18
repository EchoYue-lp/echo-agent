# ADR 0058: TaskClaim-Derived Subagent Attempt Control

- Status: Accepted
- Date: 2026-09-18
- Owners: `echo-orchestration/tasks`, `agent/subagent`

## Status

Accepted for the framework phase. The independent SDK adapter and inventory
remain pending and Issue #99 stays open until that second phase is delivered.

## Context

ADR 0008 made the revisioned task graph and `TaskClaim` the durable execution
authority. Team adapters still discarded that claim before member dispatch,
created unrelated execution identifiers, and gave every task the TaskRun root
cancellation token. An exact interrupt could therefore miss an attempt waiting
for admission, cancel siblings, or wait forever for a non-cooperative dispatch.

The control plane also has two different durability levels. A TaskClaim and its
compare-and-set settlement survive restart. Pending, reserved, active, and
settled Subagent control entries are process-local projections and cannot prove
that a command still targets the current durable attempt.

Cursor separates editable plan artifacts from execution roles. OpenAI Codex
binds active work to cancellation tokens and derives child cancellation for
the concrete task. Kubernetes `resourceVersion` and HTTP `If-Match` both reject
stale mutations instead of allowing a live cache to overwrite durable state.
These implementations support one durable identity with bounded live control,
not a second task lifecycle.

References:

- <https://cursor.com/docs/agent/plan-mode>
- <https://cursor.com/docs/subagents>
- <https://github.com/openai/codex/blob/3d3ae4965ab370217e871b3a7f0d15589557ee4b/codex-rs/core/src/state/turn.rs>
- <https://github.com/openai/codex/blob/3d3ae4965ab370217e871b3a7f0d15589557ee4b/codex-rs/core/src/tasks/regular.rs>
- <https://kubernetes.io/docs/reference/using-api/api-concepts/#resource-versions>
- <https://www.rfc-editor.org/rfc/rfc9110.html#name-if-match>

## Options Considered

1. Keep random Team member execution IDs and correlate them after dispatch.
   This cannot close the admission race or prove which durable attempt an event
   belongs to.
2. Cancel the TaskRun root token for exact task control. This is simple but
   incorrectly interrupts siblings and makes retry/recovery ambiguous.
3. Persist another Subagent state machine in the framework. This duplicates the
   TaskClaim authority and still requires cross-store reconciliation.
4. Derive one exact Subagent identity from TaskClaim, keep live control as a
   process projection, and guard every public control request with the durable
   claim before and after projection.

## Decision

Option 4 is adopted.

- `TaskClaim::execution_id(run_id, task_id)` is the only physical attempt
  identity. `TaskSubagentContext` carries the claim, task ID, run ID, revision,
  attempt, execution ID, and task child cancellation token without adapters
  rebuilding those fields.
- Event `run_id` remains the business run identity. A separate runtime-owned
  `control_scope_id` namespaces the process registry so an inner Team graph
  cannot reconcile its parent attempt or another graph that shares a run ID.
- `RuntimeDagExecutor` reserves the exact attempt before shared admission or a
  semaphore wait. Team member dispatch consumes the same reservation through
  `dispatch_attempt`; default Team and React Team use the same contract.
- Each claim gets a child cancellation token. Run cancellation propagates to
  all children; exact cancellation affects only its child.
- The live registry uses `Pending -> Reserved -> Active -> Settled`. Pending
  intents are bounded. Durable snapshot reconciliation removes stale pending
  and settled projections and cancels stale reserved or active attempts while
  retaining their binding until canonical dispatch settlement.
- `RuntimeTaskService::request_attempt_interrupt` checks the durable claim,
  projects the live interrupt, checks the claim again, and fails closed if the
  claim changed during the request. Its receipt and stale/capacity errors are
  typed; queued, reserved, first-active, repeated-active, and settled outcomes
  remain distinguishable.
- Each wave retains an exact `AbortHandle`. After the cancellation grace period
  only the target handle is aborted. The original `JoinSet` drains it and the
  cancelled claim settles through the existing task CAS path. Root cancellation
  retains the existing wave-wide grace and `abort_all` behavior.
- A queued command does not spend its grace before an `AbortHandle` exists.
  Handle registration observes an already-cancelled child and rearms a
  generation-fenced timer. Joined attempts retain cancellation/run/control
  projections until durable CAS is proven, closing the join-to-commit window.
- A successful durable settlement is the commit point. Live cleanup failures
  are diagnostic and cannot reverse terminal state. If the settlement response
  is lost, the executor reloads `claim_is_current`; a non-current claim proves
  that durable authority advanced and prevents a second abandonment CAS.
- The join-to-CAS observation is typed as settled or authority-unknown. A
  durable write plus claim lookup failure releases waiters with a retryable
  authority error instead of waiting forever. Recovery reads the durable
  snapshot first, publishes `superseded` to stale joined waiters, retires their
  local supervisor state, and only then awaits live-controller cleanup. Active
  stale attempts remain bound until targeted abort and canonical JoinSet drain.
- `TeamRuntimeHandle` binds a stable run ID, the Team task store, one
  `RuntimeTaskService`, and the same Subagent control registry. Execution,
  snapshot queries, and exact interrupts reuse this handle.
- Every handle has its own `handle_id`. A business run may retain multiple
  nested or concurrent Team handles; lookup by run returns all of them and
  exact lookup uses `handle_id`, so later execution cannot overwrite earlier
  control authority. Active handles are never evicted; the bounded 64-entry
  retention applies only after execution settles.
- Custom Team execution implements the complete `TeamDispatchController`
  contract. A bare callback cannot be attached to a stable control handle.
- External `RuntimeDagController` adapters obtain one
  `SubagentAttemptControlHandle` from `SubagentExecutor`. The handle fixes one
  control scope and keeps reservation, claim-derived dispatch, live interrupt
  projection, retirement, and reconciliation on the same registry. Raw
  registry mutation remains crate-private.
- Caller-supplied runtimes use `TeamRuntimeServiceHandle<R>`, which constructs
  the service from and stores the same `Arc<R>`; APIs never accept an unrelated
  runtime and service as separate arguments.

Durable command persistence remains a Host/SDK responsibility. Framework live
control is replayable from durable claims but is not itself a command ledger.

## Consequences

Events, callbacks, terminal results, and control requests now share the same
run/task/revision/attempt/execution lineage. Exact interrupts are sibling-safe,
cover admission waits, and terminate non-cooperative local work after a bounded
grace period. Already-issued remote effects remain governed by their original
effect contract; cancellation cannot retract an external side effect.

`RuntimeDagController` gains default control hooks, so existing adapters remain
source compatible while adapters that promise exact control can bind their live
registry. Team runtime APIs expose a stable handle. External handles are only
live capability objects: `RuntimeTaskService` still owns durable claim checks
and terminal settlement. SDK adapters must pin the framework revision and
preserve this identity rather than implementing another attempt mapping.

## Verification

Focused tests cover claim-derived Team event identity, pending and reserved
interrupts, cancellation classification on admission drop, sibling isolation,
pre-admission cancellation, exact targeted abort of a non-cooperative attempt,
stale-claim pre/post checks, durable reconciliation, lost settlement responses,
join-to-CAS authority failure and blocked-cleanup recovery, shared Team runtime
handles, and public facade construction.
