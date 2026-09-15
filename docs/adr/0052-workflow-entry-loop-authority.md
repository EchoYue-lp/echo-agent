# ADR 0052: Canonical Workflow Entry Loop Authority

## Status

Accepted

- Date: 2026-09-15
- Owners: `echo-orchestration::workflow`

## Context

`Graph::run`, `run_until_interrupt`, checkpoint resume, and `run_stream` each
implemented their own node-routing loop. The loops separately decided node
boundaries, finish handling, fan-out, interruption, errors, and completion.
They had already diverged: finish nodes bypassed `interrupt_after`, resumed
execution used different interrupt checks, parallel failures omitted
`NodeError`, and the public `WorkflowEvent::Token` had no built-in producer.

This is a generic framework concern. It does not depend on EKO product policy,
UI projection, Task DAG scheduling, or SDK wire types. The existing `Graph`,
`SharedState`, `WorkflowContinuation`, and `CheckpointStore` remain the public
and persistence contracts.

## Industry Evidence

LangGraph's compiled Pregel runtime implements `invoke` by collecting
`stream`, and `ainvoke` by collecting `astream`. Interrupt options and output
projection are parameters to the same execution path rather than separate
graph loops. Temporal similarly records each workflow state transition in one
Event History and restores execution by replaying that history; recovery does
not define a second workflow program.

These systems differ in persistence and distribution, but converge on the
relevant rule: invocation, observation, interruption, and recovery are views
over one transition authority.

References:

- [LangGraph Pregel `invoke`/`ainvoke` source](https://github.com/langchain-ai/langgraph/blob/230927fb3a9ac9b2893a30322b4dfea7cdea9a8f/libs/langgraph/langgraph/pregel/main.py#L3783-L4095)
- [Temporal Workflow Execution and replay](https://docs.temporal.io/workflow-execution)
- [Temporal documentation source](https://github.com/temporalio/documentation/blob/cf33981b02ad2e5cac696c68db4217f6e34b53d1/docs/encyclopedia/workflow/workflow-execution/workflow-execution.mdx)
- GitHub Issue #112.

## Options

1. Keep four loops and add parity tests. This detects future drift but leaves
   four owners for routing and terminal semantics.
2. Extract a single-step reducer while every public entry retains a loop. This
   shares local transitions but still lets entry adapters reorder interrupts,
   events, and terminal settlement.
3. Use one internal execution loop and make public entries thin adapters. The
   loop emits events to an optional channel; non-streaming entries ignore the
   channel, while `run_stream` projects it as a pull stream.

## Decision

Choose option 3:

- `Graph::execute_loop` exclusively owns cancellation checks, step limits,
  node lookup and execution, routing, fan-out, finish handling, interrupt
  checkpoints, path/step accounting, `NodeError`, and `Completed`.
- `run` and `run_until_interrupt` only construct a fresh cursor and select
  whether interrupts are honored.
- Resume keeps the existing claim/heartbeat/ack/requeue settlement wrapper,
  restores the checkpoint into the same execution cursor, and calls the same
  loop. Approval skips only the exact `BeforeNode` interrupt represented by
  that checkpoint; a distinct interrupt on the next node is still honored.
- `run_stream` only transports committed loop events. It drains queued events
  before returning the terminal error, so `NodeError` is observable before the
  matching failed stream item.
- Agent nodes always use the existing cancellable
  `Agent::execute_stream_with_cancel`/`chat_stream_with_cancel` contract, so
  Graph cancellation, timeout, stream drop, and sibling failure reach the
  producer. Token deltas become `WorkflowEvent::Token` when a consumer exists
  and are otherwise drained; the terminal `FinalAnswer` is committed to the
  configured state key and message history before `NodeEnd`.
- A finish node is an ordinary executable node followed by an `End`
  continuation. `interrupt_before` and `interrupt_after` therefore apply to it
  with the same semantics as every other node.

The public API, serialized `WorkflowEvent` variants, checkpoint shape, and
`CheckpointStore` contract do not change.

## Consequences

All production Graph entry points now share one routing and terminal authority.
Adding a new entry mode must delegate to `execute_loop`; it must not copy the
node loop. Checkpoint settlement remains outside the loop because it owns the
lease around one resume attempt, not workflow transitions.

Graph Agent nodes now require the documented terminal event contract: a
successful stream ends with `AgentEvent::FinalAnswer`. An `AgentEvent::Error`,
`Cancelled`, or stream error becomes the node failure; no `NodeEnd` or
`Completed` follows it.

Parallel success state and path projection remain in registration order.
Progress events may expose completion order, while their `step_index` remains
the stable registration-order index.

## Verification

Focused `echo_orchestration` Graph tests cover linear, conditional, loop,
parallel, cancellation, max-step, checkpoint claim/requeue/heartbeat, and all
four public entry families. Dedicated regressions cover Agent tokens,
finish-node interrupt/resume parity, sequential `NodeError` ordering, and
parallel `NodeError` ordering. Full workspace and all-feature gates remain the
merge workflow's responsibility.
