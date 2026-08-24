# ADR 0003: Tracked Steering Lifecycle

- Status: Accepted
- Date: 2026-08-25
- Owners: `echo-core/agent`, `echo-agent/agent/react`

## Context

`Agent::steer_input` historically returned the active turn ID immediately after
placing a message in the ReAct mailbox. Callers could prove acceptance, but not
whether the single ReAct loop had moved that message into model context or
whether the owning root turn later completed, failed, was cancelled, or was
aborted. A durable application therefore had to choose between retaining an
already-consumed input forever and acknowledging it before consumption.

The framework already has one turn mailbox, one ReAct core loop, and one
`ActiveTurnLease`. Adding an application-side queue or inferring consumption
from product rendering would create a second lifecycle authority. The design is
based on these verified repository boundaries and the existing lifecycle ADRs;
external documentation was unavailable during this review, so no external
runtime behavior is assumed.

## Options Considered

1. Keep returning only a turn ID and let each application infer consumption
   from its transcript or foreground state. This cannot distinguish mailbox
   acceptance from model-context drain and fails across cancellation or crash.
2. Replace `steer_input` with a new mandatory return type. This is precise but
   unnecessarily breaks existing `Agent` implementations and callers.
3. Preserve `steer_input` and add an optional tracked method whose receipt is
   driven by the existing mailbox, core-loop drain, and turn lease. This keeps
   compatibility while giving durable consumers authoritative boundaries.

## Decision

The framework adds `Agent::steer_input_tracked` and the public
`AgentSteerReceipt`, `AgentSteerState`, `AgentSteerPhase`, and
`AgentSteerTurnOutcome` types.

- `Accepted` is published only after the exact active turn mailbox owns the
  input.
- The core loop acquires its context lock, takes a batch from the mailbox,
  inserts every message into `ContextManager`, and only then publishes
  `Drained`. Waiting for the context lock cannot produce a false drain receipt.
- The active turn lease publishes `TurnSettled` from the real root-turn
  terminal. Completed, cancelled, and failed paths are explicit. Dropping or
  aborting the owner publishes `Dropped`.
- Terminal state records whether drain occurred. If cancellation or drop wins
  before context insertion, the receipt settles with `drained: false`.
- Each active lease carries a private, non-reusable incarnation. External turn
  IDs remain stable correlation keys, but a stale lease cannot change
  steerability or settle/drop a newer incarnation that reused the same ID. The
  exact token is cloned into `AgentRunSnapshot`; mailbox drain requires both the
  external ID and pointer-identical incarnation, so an old core cannot consume
  a newer same-ID generation.
- A `UserPromptSubmit` hook block is a failed root turn, never a completed one.
  Steering accepted while that hook runs settles as failed and undrained.
- If an implementation drops the watch sender without publishing a terminal,
  every receipt clone shares one synthesized `Dropped` terminal that preserves
  whether the last observed state was drained. Waiters never return a stranded
  `Accepted` or `Drained` state after channel closure.
- State transitions are monotonic. A late drain cannot overwrite an already
  settled turn, and replacing an unexpected active lease settles the previous
  one as dropped.
- Mailbox and receipt locks are synchronous and are never held across an await.
  The async context lock is acquired before the short mailbox transfer.
- The legacy `steer_input` remains available and uses the same mailbox. Third
  party `Agent` implementations remain source compatible; tracked steering
  returns `Unsupported` until they provide real lifecycle signals.

No application journal, retry policy, UI state, or delivery-specific concept is
added to the framework. Applications decide which receipt boundary is durable
enough for their product.

## Consequences

Durable consumers can retain input through `Accepted`, acknowledge it after
`Drained`, and separately wait for root-turn settlement. `Drained` does not mean
the turn succeeded, and `TurnSettled { drained: false, .. }` explicitly tells a
caller that replay remains necessary.

One small watch channel is retained per accepted steering input until the turn
settles. Legacy callers may discard the receiver; the same bounded turn owner
still releases the sender at terminal.

Tracked steering is an additive public API. Implementations that only override
the legacy method continue compiling but do not receive inferred or fabricated
tracking.

## Verification

Mailbox tests cover accepted-before-drain, terminal-before-drain, owner drop,
same-ID stale-lease ABA, unique concurrent receipts, and FIFO batch drain. A
same-ID stale-snapshot test proves that only the current incarnation can drain.
A context barrier test proves
that a receipt remains accepted while the context lock is held, then changes to
drained only after insertion. A competing terminal test proves that settlement
before context access yields `drained: false`. ReAct integration tests cover
completed, provider-failed, and cancelled terminals after a real drain, plus a
non-streaming hook block accepted before terminal. Closed-sender tests cover
accepted/drained inputs and cloned waiters. A façade test keeps all public
receipt types reachable through `echo_agent`.
