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

### Safe points before unstarted tool work (2026-10-25)

The same core loop also checks accepted input after a model response and before
admitting each existing execution wave. The alternatives were to keep waiting
until the whole tool batch completes, or to interrupt already-running tool
futures. We choose an earlier check for unstarted work while preserving the
original physical owner for admitted work:

- Before the assistant tool proposal is added to the transcript, the original
  drain can discard that unpublished proposal and return to the next model
  iteration with the correction. No tool result is fabricated for an
  unpublished call. Ordinary assistant draft text may remain as context.
- After the assistant tool message has been published, a correction at the next
  wave admission skips the still-unstarted calls. Every published ID receives
  its own explicit unsuccessful, not-executed result through the existing
  interruption trace/audit/callback projector. Their execution-start authority
  proves `side_effect = None`; an interrupted call that actually entered
  `ExecuteStage` keeps the original conservative `Possible` fact.
  The original current-wave slice proves which attempts were never admitted:
  a completed earlier model batch may legally reuse a call ID. The accumulated
  turn-wide started-ID history cannot override this later not-started fact,
  and is retained for the original physical owners rather than cleared.
  For an admitted attempt that fails before execution, the original pipeline
  context independently records entry into `ExecuteStage` before its first
  await. This flag covers direct/custom `ExecuteStage` calls without callbacks;
  callback visibility or an empty result is not used to guess physical start.
- Close the outstanding call/result pairs before inserting the new user input.
  Only the original context insertion publishes `Drained`; observing pending
  input does not consume it or acknowledge it. Both pending observation and
  drain require the same turn ID and private incarnation.
- A wave admitted at the mailbox check keeps all of its original futures,
  results, stream readers and resources until actual settlement. Input accepted
  after that check is considered at the next unstarted wave or loop boundary;
  it does not preempt an active model, tool, parallel peer, HITL or side effect.
  An explicit Stop continues through the original cancellation path.
- A skipped remainder invalidates earlier final-answer candidates from that
  batch. The original loop asks the model to reconsider; it does not replay
  uncertain effects, mutate a task/plan graph, or create a second scheduler.

The comparison source is the official
[Pi agent loop](https://github.com/earendil-works/pi/blob/main/packages/agent/src/agent-loop.ts),
reviewed as file blob `cfc9401e5581847f6d5a9bb1815ab0f0e663f2e5`
(a file identity, not a repository commit). That implementation polls steering
after the full assistant tool batch, including sequential execution. The
pre-tool/wave checks here are an adopted responsiveness enhancement, not a
claim that Pi already skips remaining calls when steering arrives.

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

Original-loop model-return and physical-tool barriers additionally distinguish
unpublished proposals, admitted sequential/parallel waves and later unstarted
waves. They check actual owned sentinel files, complete call/result pairing,
context-before-drain ordering, no-steering controls and Stop interleavings.
Interruption controls distinguish started and unstarted calls for both timeout
and cancellation. These tests require execution on the changed source; their
definitions alone are not runtime qualification or application/GUI acceptance.
Cross-iteration ID-reuse controls inspect each original proposal's call/result
pairs separately and distinguish the earlier completed effect from the later
unadmitted effect and its actual skipped trace fact.
