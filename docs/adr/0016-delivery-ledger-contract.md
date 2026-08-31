# ADR 0016: Product-Neutral Delivery Ledger Contract

## Status

Superseded by [ADR 0019](0019-typed-delivery-ledger-api.md) for the public API
shape. This record remains as the historical contract baseline.

## Context

AgentRouter currently combines a product message envelope, an EKO address,
file-backed inbox layout, logical FIFO retention, and delivery lifecycle
settlement. The framework already owns the journal, checkpoint, replay, and
durability primitives, but replacing the EKO event schema directly could make
persisted messages unreadable.

## Decision

The framework exposes a product-neutral `DeliveryEnvelope`, `DeliveryEvent`,
`DeliveryLedgerProjection`, and journal-backed `DeliveryLedger`. Routes are
opaque strings, payloads are JSON values, and lifecycle facts carry stable
message, attempt, turn, correlation, and causation identities. The projection
enforces FIFO, stale-claim rejection, ordered effect/mailbox/drain/settlement
transitions, retry deferral, and configurable logical terminal bounds.

The first stage freezes this contract and proves EKO field-level conversion. It
does not replace the persisted EKO `AgentInboxEvent` schema yet. A later stage
must add a legacy replay adapter, switch one real enqueue/cold path, and only
then delete the displaced application reducer.

## Boundary

The framework does not know `WorkspaceId`, `ConversationStore`, `AgentMessage`,
groups, file paths, live/cold runtime selection, wake scheduling, authorship
policy, or surface rendering. EKO keeps those decisions and bridges framework
`AgentSteerReceipt`/`TurnInputReceipt` to the generic lifecycle.

## Consequences

Framework consumers can reuse durable delivery lifecycle semantics without
introducing a second mailbox or database. EKO retains compatibility control for
old inbox journals and can migrate schema deliberately after round-trip,
restart, stale-claim, retention, and owner-loss evidence is complete.
