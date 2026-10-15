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
