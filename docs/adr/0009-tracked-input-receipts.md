# ADR 0009: Tracked Active and Initial Input Receipts

- Status: Accepted
- Date: 2026-08-27
- Owners: `echo-core::agent`, `agent/subagent`, `echo-orchestration::runtime`

## Context

Active Subagent messages were routed through the legacy `steer_input` API. That
API returns only a turn identity, so a caller could mistake mailbox admission
for context consumption or turn completion. Initial inputs driven by
`AgentTurnDriver` had the inverse problem: the driver returned a terminal
`TurnReceipt`, but exposed no lossless way to observe acceptance or context
drain before that terminal.

The framework already has one authoritative tracked lifecycle for active-turn
steering: `AgentSteerReceipt` reports `Accepted`, `Drained`, and
`TurnSettled(completed|failed|cancelled|dropped)`. The framework must reuse that
authority instead of adding another mailbox, reducer, or supervisor.

## Decision

1. `SubagentControlRegistry::send_message_tracked` calls
   `Agent::steer_input_tracked` directly. `SubagentMessageReceipt` contains only
   the exact attempt envelope (`execution_id`, `attempt`) and the nested
   `AgentSteerReceipt`; its turn identity is read from that nested receipt.
   The legacy `send_message` route and independent `Delivered` projection are
   removed from the framework.
2. `TurnRequest::with_input_receipt` creates one `TurnInputReceipt` and places
   only its publisher in the request. The caller keeps the receiver before
   invoking the same `AgentTurnDriver`.
3. `AgentTurnDriver` validates the request identity, publishes `Accepted`
   immediately before calling an Agent stream API, and settles the same receipt
   with the real `TurnOutcome`. Publisher drop is an RAII `Dropped` terminal.
4. A concrete Agent publishes `Drained` through the optional
   `AgentInvocationContext::input_lifecycle` publisher only after the initial
   input has been successfully inserted into its model context and before
   provider execution. `ReactAgent` owns that producer point. Generic Agent
   implementations without the publisher retain `drained = false`, even when
   they emit output.
5. `TurnInputReceipt` and its publisher share one watch controller and one
   terminal guard. Receipt reads never create a second cached terminal
   authority; a closed publisher is interpreted as `Dropped` only as a
   defensive fallback.

## Route-specific phases

| Route | Accepted producer | Drained producer | Terminal producer |
| --- | --- | --- | --- |
| Active Agent/Subagent message | `TurnSteerMailbox::steer_tracked` | ReAct loop `SteerDrainBatch::mark_drained` | active-turn lease settlement |
| Cold initial input | `AgentTurnDriver` after identity validation, before Agent stream call | `ReactAgent` after `ContextManager` input preparation, before provider call | the same `AgentTurnDriver` |
| Generic Agent without lifecycle publisher | `AgentTurnDriver` | none; remains `false` | the same `AgentTurnDriver` |

Terminal-before-drain, cancellation, sink failure, missing terminal, shutdown,
ABA/stale attempt, and publisher/future drop are typed outcomes. No output EOF,
rendered event, or terminal event is used to infer `Drained`.

## Alternatives rejected

- Keeping `turn_id` as a `Delivered`-looking Subagent receipt: conflates
  identity with consumption and creates a second authority in application
  adapters.
- Marking cold input drained on the first output envelope or at stream EOF:
  neither proves that `ContextManager` accepted the input.
- Adding an application mailbox, store, supervisor, or a second lifecycle
  reducer: violates the framework/application boundary and duplicates the
  existing tracked mailbox.

## Consequences

Framework consumers can wait for active-message drain and terminal settlement,
and can observe initial-input admission without changing the driver or owning a
second tracker. Existing legacy `steer_input` callers remain source-compatible
as a thin turn-ID adapter, but framework production control paths use the
tracked APIs. Product-specific durable persistence and admission policy remain
outside this crate.

## Verification

Focused tests cover active accepted-before-drain, all four terminal outcomes,
terminal-before-drain, exact-attempt stale/settled rejection, cold accepted /
drained / failed / cancelled / dropped outcomes, generic no-publisher
no-drain, immediate Agent construction drain, and monotonic single settlement.
