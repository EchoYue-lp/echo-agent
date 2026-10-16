# Cancellation observation and physical settlement

The framework retains one original task/Subagent execution owner until its
producer closes and the existing exact-claim CAS settles. Cancellation asks
that owner to stop. The configured cancellation grace bounds only observation;
it does not authorize forced abort, early claim release or a replacement attempt.
A normal execution has no grace timer. Repeated requests do not extend the
first admitted scope's deadline; exact A does not cancel sibling B or start the
root observer clock.

`ReactError::ExecutionNotSettled { run_id, execution_id }` reports the observer's
knowledge. It is not Failed, Cancelled, Paused, a new TaskStatus, or a safe reason
to replay execute. Read the same original handle/status. Without physical
closure knowledge, including foreign Agent setup, disconnect and restart,
retain claim/resources and explicit unknown admission rather than assuming stop.
Non-cooperative producers can keep existing capacity occupied indefinitely.

## Rust source migration

- `AgentFailure::from(&error)` becomes `AgentFailure::try_from(&error)?` at a
  fallible terminal boundary. Ordinary fields remain unchanged; an observation
  is returned as the same typed error instead of a fabricated terminal failure.
- `AgentEvent::from_error(source, &error)` returns `Result<AgentEvent>`.
  Channels carrying `Result<AgentEvent>` can send that result directly. Normal
  error event wire fields and accepted `standard:session/update` routing remain.
- `AgentTurnDriver::drive(...)` returns `Result<TurnReceipt>`. `Ok` is the
  original receipt. An observation produces no Failed receipt; live original
  streams are drained to actual EOF. Existing `core:run` / `_echo_agent/run/start`
  routing does not change.
- ACP `prepare_run` records a driver observation on the original `RunEntry`
  and completes its connection task successfully, because an official ACP
  spawned-task error closes the whole server. Use `RunEntry::wait_result()`
  to observe either the original receipt or its structured Run error. An error
  retains the original lease, remains nonterminal and does not permit retry.
  Standard Prompt returns it as a request error; unrelated requests continue.
  `wait_receipt()` and bounded SDK Run waits remain receipt-only, including
  after cancellation or EOF. A real committed receipt takes precedence over
  earlier observation; transport failures still propagate.
- `subagent_status_from_error` becomes fallible for the same nonterminal case.
- `TurnInputReceipt::wait_for_accepted`, `wait_for_drained` and
  `wait_for_turn_settled` return `Result<TurnInputState>`. The original watch
  wakes on observation refusal without fabricating Dropped, Drained or a
  terminal input state. Sender loss is not physical closure proof.
- Controller implementations must add
  `request_task_interruption(run_id, task, claim, disposition) -> Result<bool>`.
  Delegate to `TaskRevisionService::request_runtime_task_interruption`; true is
  exact current-claim durable admission, false is supersession, and IO failure
  retains uncertainty. Do not implement a second lifecycle algorithm.
- Internal Team composition uses `RuntimeTaskService::execute_to_settlement`,
  sharing the original private spawn/Join owner with `execute`, but waiting for
  physical settlement without an observer deadline. Do not call execute again
  to retrieve a timed-out owner's result.


- `HeadlessResult` keeps its five public values and formatting/exit behavior.
  Replace external struct literals with `HeadlessResult::new(output, success,
  model, format, exit_on_error)`. A returned observation's `retained_owner()`
  borrows the same original `HeadlessRunHandle`; retain the result or clone that
  handle while its Agent close authority remains unknown. Internal stored
  results are owner-free, so this projection has no state/result self-cycle.
  Cancelling a waiter signals its child scope and does not abandon the original
  running task. If waiting may be cancelled and subsequent unknown ownership
  must remain observable, use `start_headless` and retain that handle; a cancelled
  convenience call has not yet returned an owner. Explicitly dropping every
  parent owner is not physical proof;
  an observation does not authorize automatic retry or invent late closure.

These are intentional Rust source compatibility changes. SDK Host/CLI consumers
must adapt their existing owner and error boundaries and validate compilation;
the new Rust variant/helper is not automatically an accepted language method or
wire identity. [ADR0088](adr/0088-subagent-settlement-observation.md) records the
accepted decision and [the design](supreme/specs/2026-10-14-subagent-physical-settlement/design.md)
defines the complete lifecycle and validation obligations.

## Started Tool ownership

The canonical started/draining Tool path retains the same execution and permit
through configured timeout, cancellation grace, batch timeout, and output receiver
closure. Cancellation signals the Tool's child scope; parent and sibling scopes
are unchanged. A first Complete keeps its complete business result while the
original stream drains to EOF. Output delivery failure stops delivery and returns
its first cause only after that physical closure. Unknown closure remains typed
and retains the original resource projection. Ordinary non-draining ToolManager
methods keep their existing bounded policy.

## A later result from the same live source

An intermediate `ExecutionNotSettled` does not make the original stream unusable.
Continue that same source: a later explicit terminal for its bound run/execution,
followed by actual EOF, can let the original owner settle once. Preserve the first
business cause and value fields. Source identity and execution Option must match;
sink observations are separate. EOF alone, synthetic terminal projections, wrong
generation or a newer observation cannot release the original claim/resources.
The default Core cancellation wrapper retains physical drain and defers its one
cancel terminal until that source closes; if an observation occurred, a later
original terminal is required. No-source setup uncertainty stays unknown. This
adds no new wire event, replay operation, registry or deadline.

## Scheduler callback ownership

New scheduled/manual admissions persist a protected physical-settlement payload in the original occurrence journal. Dropping a run_once/drain waiter detaches only observation; the same admitted callback owner continues its original work and result projection. Cancellation does not requeue or drop it. Typed observation leaves its original occurrence/attempt nonterminal and does not update last_run or produce a Failed delivery. A protected started occurrence cannot be automatically replayed after owner loss or reopen; known ordinary result/error still settles through the existing ledger. join/shutdown distinguishes loop exit from protected unresolved work.

Old flat records retain their explicit legacy at-least-once recovery. New versioned protected payloads reject older readers instead of letting them ignore the protection field; downgrade therefore fails closed. No new Task status, public scheduler method, SDK operation, recovery registry or late-receipt guarantee is added. Rust EffectStarted literal/match consumers additionally handle its optional replacement_payload field; existing helpers emit None and preserve the original bytes. A callback that already returned unknown cannot be re-executed to obtain its result; authoritative late closure must actually exist, not be inferred from an empty active set.

Keep a mutable SchedulerHandle when calling `join`/`shutdown`: both now borrow the same parent instead of consuming it. The handle retains the original Runner/FireFn/store/control root after an unknown result, and a cancelled pending wait does not take its original join. Repeating a wait observes the same operation, never starts another callback. A real callback still in progress has its own retained original execution task; one that already returned unknown has no invented late-join source. Explicitly dropping the final Runner/Handle (including temporary-handle chaining) destroys that parent by caller choice; it is not evidence of physical stop or permission to replay.

A legacy journal is not permission to replay a new unsettled callback. Its historical initial recovery may proceed, but the new callback's protected payload is committed atomically with the original exact-claim EffectStarted event before the effect, including the existing recovery batch. The optional payload field is absent for ordinary ledger users; protected events/checkpoints deliberately reject old readers rather than allowing a downgrade to discard protection. No journal authority is bypassed with an in-memory or checkpoint-only rewrite.

## Workflow protection (accepted A)

CheckpointStore implementations opt into supports_protected_claims and implement atomic protect_claim(checkpoint_id, attempt_id) on the same claim authority. The default is unsupported. Graph exposes its bound Store capability; executable resume variants and branch fail before claim/fork/effects without it. Deferred, Rejected and fresh execution keep their distinct behavior. Native Memory is instance-local; File retains protected records across restart using its original lock and versioned private format. A protected claim cannot expire, be overwritten/deleted or be removed by a partially successful clear. Only actual same-owner closure permits existing ack/requeue; no receipt-query/force-unlock API is added. Downgrading an old binary onto protected data is unsupported because old delete/clear bypass decoding.

Cancellation, configured Node timeout, heartbeat failure, sibling failure and delivery closure preserve the original started Node/source until physical close. Context-aware Agent calls bind the original run/node identity; a same-source later terminal+EOF can supersede its own observation, while foreign/setup/no-terminal observations remain unknown. Preserve the original Graph/Store/Agent parent when observing unknown; do not execute again or infer closure from heartbeat/TTL/EOF alone. The accepted SDK reverse operation/capability are maintained in the separate SDK repository, bound to original proposal23c5a59a; Rust additions are not automatically language API.

Each original Graph operation owns its one existing loop/Node future through a private retained-definition lease. Dropping a borrowed run/resume waiter or a polled run_stream signals that same operation and leaves it draining; it does not execute again. A never-polled stream starts no producer. Keep the original Graph/Agent/Store parent after a returned unknown; destruction of all parents or runtime shutdown is not an automatic physical receipt. No public Graph Clone/handle is added.

Workflow Store observations from claim renewal or requeue preserve their original run/execution identity. The original Node still drains after renewal failure, but its local terminal cannot clear a Store observation or authorize ack/requeue. Ordinary Store errors retain their known-closure requeue policy. An auxiliary ordinary requeue failure after a first business cause is logged without replacing that original typed cause.
