# ADR 0060: Task execution CAS, recoverable results and scoped interruption

- Date: 2026-10-12

## Status

accepted

## Context and ownership

`TaskRevisionService` accepted arbitrary `RevisionedTaskStore` backends, but
execution mutation was only exposed on `InMemoryRevisionedTaskStore`. A consumer
could register a callback Store for CRUD while continuing execution against a
different in-memory graph. Team success publication also used a process-local
result map after committing `Completed`, so restoring the graph did not restore
its dependency outputs. A task-local Cancelled or Paused state caused the DAG
executor to settle the whole run, interrupting unrelated branches.

General mechanisms belong in the framework: relation/execution CAS, exact
claim identity, results, dependency blockers, retries, cancellation and safe
points. SDK adapters serialize typed outputs and callback Store transactions;
they do not own another graph, ready frontier or recovery state machine. EKO
review, worktree and UI policy remain application concerns.

Whole-repository discovery found the existing runtime pure mutation helpers,
`TaskRevisionService`, `RuntimeTaskService`, exact-claim attempt control and
Team result accessors. This decision extends those paths rather than creating
a second executor or result Store.

## References and alternatives

[Temporal CancellationScope](https://typescript.temporal.io/api/classes/workflow.CancellationScope)
separates scoped cancellation from parent scope cancellation. That distinction
supports preserving independent DAG branches after cancelling one task.
[LangGraph persistence](https://docs.langchain.com/oss/python/langgraph/checkpointers)
records graph state and successful node writes so later execution can reuse
successful work. Here the existing Task graph is the persistence authority;
we do not add another checkpoint or success cache.
[Tokio select cancellation safety](https://docs.rs/tokio/latest/tokio/macro.select.html)
requires reasoning about futures dropped at cancellation points. Committing a
result and terminal status in one transaction removes the post-CAS publication
await window rather than depending on a local cache being eventually filled.

Alternatives rejected:

1. Add runtime RPC methods that mutate an adapter-local graph: leaves two
   authorities and repeats framework lifecycle logic in each language.
2. Advance relation revision on every execution write: conflates editable
   plan relations with runtime facts and invalidates unrelated safe-point work.
3. Publish results to a Session cache after terminal CAS: process restart loses
   already successful outputs; cancellation can expose an incomplete success.
4. Infer a run interruption from any task status: a local stop becomes a parent
   stop and prevents independent ready roots or subsequent waves from running.

## Decision

`RevisionedTaskStore::compare_and_commit_execution` receives
`TaskExecutionCommit { expected_revision, expected_executions, next }`. The
backend atomically compares the relation revision and complete execution map.
An execution commit preserves relation revision, context, specifications and
order. A concurrent relation patch or execution mutation rejects a stale CAS.
The existing relation `compare_and_commit` remains a revision-plus-one commit.
A CRUD-only backend explicitly returns `Unsupported` for execution; it never
silently substitutes an in-memory Store. `supports_runtime_execution()` is a
side-effect-free backend declaration, defaulting to false. InMemory opts in;
custom execution backends opt in explicitly. `TaskRevisionService` exposes the
same backend capability and rejects runtime mutation before loading or writing
when it is absent. Adapters preflight before publishing an execution handle;
they do not probe this capability by making a mutation.

`TaskRevisionService` exposes the runtime mutation entries using its registered
Store. A framework-owned load/mutate/CAS loop calls the existing pure helpers,
recomputes after conflicts and returns typed outcomes. Execution CAS retries
are capped at 32; persistent conflict returns the last typed conflict so a
backend bug cannot keep lifecycle commands spinning forever. Concrete InMemory
convenience methods use that same path.

`TaskExecution.result: Option<Value>` is the canonical current-attempt output.
`CompletedWithResult` commits the value and `Completed` in the same transaction.
`Some(Value::Null)` is an explicit result and differs from missing `None`.
The result field deserializes present JSON values directly; an absent field uses
its default. Ordinary serde `Option<Value>` deserialization would collapse an
explicit null into absence and is not valid for this fact.
The no-output `Completed` request remains available to generic consumers whose
tasks have no result contract. Team and SDK result-bearing tasks must use
`CompletedWithResult`; missing or malformed result data is an explicit error.
New claims/retries clear current-attempt result data; already committed terminal
success survives a later interruption request. Historical attempt retention,
if required, is the backend journal's responsibility, not a second live output
map.

`TaskExecution.interruption_requested` records an exact-claim interruption.
`request_runtime_task_interruption` returns false for a superseded or settled
claim. The existing live interrupt API only projects cancellation to the
physical attempt. `TeamRuntimeHandle` also persists Cancelled through this same
request API before signaling its member; a cancellation-unaware member's late
success cannot bypass the durable request. Resolution uses the durable request and returns
`Interrupted { disposition }`, so a late success/cancellation cannot replace a
previously committed pause request. Completion and interruption are ordered by
the same backend CAS. A restored driver does not infer that an in-flight claim
owner is dead from the absence of process-local controls. After its owner is
proven stopped, the caller can settle the recorded request using the existing
exact-claim resolution API. Multi-process drive ownership/fencing belongs to
the backend or caller policy; an execution CAS does not implement an owner
lease. Running claims are never blindly redispatched on reconstruction.

Exact abandonment settlement returns `Settled { status }` from the same CAS.
Attempt watchers project that committed status, not the originally requested
abandonment status and not a later snapshot lookup. Relation patches that clear
claim ownership also clear the attached interruption request. The canonical
revision service and direct InMemory graph commit validate claim/request
invariants before publication.

Each joined attempt resolves through the existing exact-claim settlement path
immediately, even while an unrelated sibling remains active in the same wave.
Resolution policy, CAS and cleanup futures have independent, wave-bounded
supervised owners. The existing join supervisor concurrently observes dispatch
joins, settlement receipts and the root token. [ADR0088](0088-subagent-settlement-observation.md)
changes cancellation grace to a scoped observer deadline; it never aborts the
original dispatch or an in-flight CAS whose commit is unknown. Slow review or
Store work does not change that observation deadline or prove physical stop. If the calling execution future is dropped, the settlement
owner retains the same controller and shared control projections until it
settles or reports unknown authority. A private exit guard retains the same
watch sender and shared projection map independently of the parent's observer.
Unexpected owner exit publishes `AuthorityUnknown` only if no receipt exists,
and marks the shared control phase unknown; a genuine committed receipt is
never overwritten. The common unknown-publication helper follows the same
rule, so a supervisor observing a post-CAS cleanup panic cannot downgrade the
receipt that the owner already published. All observation writes, including
normal settlement, public reconciliation and owner exit, use one private
`publish_attempt_fact` boundary. None may become Unknown or a verified Settled
fact; Unknown may become Settled only after actual settlement/reconciliation;
Settled is immutable, including its status. The shared phase derives from the
published observation. Projection retirement never changes a retained receiver.
Public reconcile and second execute during held cleanup therefore preserve
Completed and do not redispatch the successful task. Cleanup failure remains an independent
bounded diagnostic/error. Tests retain watchers with a live and an aborted
parent across such a cleanup panic. Parent-abort tests cover success, external backend panic,
and actual commit with response/lookup loss. The next safe point still waits for every
settlement receipt; no timeout is disguised as durable success.
This makes exact Pause/Cancel receipts observable without requiring the sibling
to finish. The framework still joins and settles every claim before honoring a
run-wide stop or loading the next relation revision and dispatching the next
wave. Immediate terminal CAS is not another scheduler or an early next-wave
safe point; it uses the same registered Store and result transaction.


**Caller-drop ownership before dispatch join.**

`RuntimeTaskService::execute` is an observer of one private framework-owned
execution future. The owned future runs the single existing DAG implementation;
it does not duplicate frontier, cancellation or settlement logic. Admission
requires an active Tokio runtime and returns an error before backend work when
one is missing. The caller's cancellation scope is a child of its supplied
token. Dropping the observer requests that scope's cancellation, without
cancelling its parent token or aborting a backend claim/CAS future whose commit
is not yet known. The owner retains claim response, control reservation,
shared admission, dispatch joins and settlement until the normal framework
interruption policy has committed an outcome. This includes caller exit while
a claim response is held, before reservation completes, or while a physical
dispatch ignores cancellation. The original producer must physically close
before an exact claim is settled; grace expiry and observer disappearance are
never terminal receipts. Lost owners without closure remain AuthorityUnknown.

A wave retains known exact claims before any awaited reservation. An unexpected
external reservation/controller unwind transfers that wave's actual JoinSet to
its cleanup owner. It joins every dispatch stop, then uses the same backend
abandonment CAS for each exact claim independently. A failed or unwound
settlement publishes the existing `AuthorityUnknown` observation; it does not
invent Cancelled/Completed or prevent another known claim from settling.
Already returned post-join settlement owners remain independent, and the
single `publish_attempt_fact` boundary still makes real Settled facts immutable.

A backend that commits a claim and unwinds before returning its identity creates
an unknown ownership result: execute returns an explicit owner JoinError,
retains the canonical Running claim and does not dispatch, retry, guess an
identity, or manufacture a watch/receipt. Reconciliation reads those durable
facts without pretending that missing local control proves the owner stopped.
Backend owner leases/fencing or an authoritative receipt are required to resolve
that case, as already required for multi-process recovery. No new public
Task status, schema, retry counter, scheduler or application policy is added.

A task-local cancellation derives transitive non-success dependency blockers.
A paused dependency remains waiting and resumes without spending retry budget.
Independent ready roots and subsequent waves continue. Only the explicit run
cancellation token invokes run-wide settlement; aggregate Cancelled/Paused is
reported after independent work is exhausted, without changing blocked pending
siblings. Root cancellation still settles every unfinished task. Priority is explicit:
Cancelled is stronger than Paused, for both task requests and run recovery.
A run-level Paused settlement must settle an already accepted exact Cancelled
request to Cancelled; it cannot erase that fact and make the task resumable.
Two pause requests preserve the earlier accepted exact-claim reason. Recovery
checkpoint/reconstruction and explicit resume retain cancelled dependency
blockers while independent paused branches can resume and continue.

## Extension invocation cancellation boundary

Real SDK Host task cancellation exposed a shared-token defect outside the DAG:
`ExtensionInvocationAuthority::lease` retained the passed run/connection token
itself. Cancelling a reverse stream lease therefore cancelled that parent,
affecting unrelated Task attempts. The framework lease now owns
`framework_cancellation.child_token()`. Parent cancellation propagates downward;
exact lease cancellation, timeout and stream release cannot cancel a parent or
sibling. The SDK bridge must use this primitive without another cancellation
state machine or a redundant adapter-owned token hierarchy.

`close_admission` preserves its existing contract: it refuses new calls and
cancels the authority's connection fallback token. `cancel_all` settles and
cancels every tracked child, including those attached to custom run tokens,
without cancelling those custom parents. Bounded drain continues to wait for
lease release. These invocation facts do not change Agent/Task terminal state
or provide an additional task-cancellation authority.

## Consequences and verification

Backend authors implement an execution transaction in addition to optional CRUD
transactions. Public TaskExecution constructors must initialize both optional
fields, and serializers must preserve them losslessly. Result decoding remains
a typed consumer boundary; the framework does not depend on SDK wire shapes or
`SubagentResult`.

Focused regressions cover callback-shaped stores, explicit unsupported
execution, relation and execution CAS conflicts, exact interruption versus late
results, canonical output restoration, independent branches after local
Cancel/Pause, pause/resume without sibling replay and existing root cancellation.
Formal Task docs are updated in English and Chinese. Framework examples live
under `echo-agent-learning/examples`; no changed example directly constructs
TaskExecution or implements the new optional backend method, so their source is
unchanged and compilation remains part of workspace all-target verification.
Website and SDK contract/client updates are consumer deliverables tracked by
the coordinating SDK governance task rather than framework-owned files.
