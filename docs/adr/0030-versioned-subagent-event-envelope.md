# ADR 0030: Versioned Subagent Event Envelope

- Status: Accepted
- Date: 2026-09-04
- Scope: `echo-core::agent::EventEnvelope`, `echo-agent::agent::subagent`

## Context

The Subagent executor already wrapped inner `AgentEvent` streams in `EventEnvelope`, but it
discarded the envelope and re-emitted raw `SubagentEvent` values. Dispatch start, isolation, and
terminal events were outside that inner stream. Consumers therefore received no common sequence,
stable event id, timestamp, or parent link. A bounded broadcast receiver could lag without a
Subagent-specific replay contract, and a GUI adapter had to reconstruct identity after receipt.

## Decision

1. `EventEnvelope` accepts a serializable payload while retaining `AgentEvent` as its default type.
   All event families reuse the same identity validation, deterministic event id, canonical content
   hash, timestamp, and payload-neutral trajectory validation.
2. One outer Subagent dispatch attempt owns one `SubagentEventPublisher`. Its complete lifecycle,
   including internal hook retries, uses one stream and one monotonic sequence.
3. `SubagentEventPayload` hashes the Subagent invocation identity together with the raw event.
   Task, attempt, plan revision, agent path, and parent execution come from existing lineage values;
   applications must not reconstruct them from execution-id formatting.
4. The existing raw event subscription remains a compatibility projection derived from envelopes.
   Direct raw emission accepts registry events only; execution and uplink events require the active
   attempt publisher. New consumers use `subscribe_envelopes` as the ordering and identity authority.
5. The bus retains bounded general history, lifecycle/tool boundaries, and terminal snapshots.
   `replay_after` returns a typed gap when its suffix is not contiguous, while
   `replay_for_execution` discovers a retained stream even when its start was missed. The latest
   retained terminal is returned separately for final-output reconciliation.

## Alternatives

- Add sequence numbers in each application adapter: rejected because loss before the adapter would
  appear continuous.
- Add common fields to every raw enum variant: rejected because start and terminal production still
  would not share one sequencing authority and identity logic would be duplicated.
- Make replay unbounded: rejected because model deltas can grow without limit in a long-running
  local process.

## Consequences

Framework consumers can detect lag, replay retained boundaries, and reconcile terminal output
without adopting EKO concepts. Raw listeners remain source compatible but do not provide ordering
or recovery guarantees. Replay is an in-process bounded window, so embedding applications still
own durable storage and product addressing.
