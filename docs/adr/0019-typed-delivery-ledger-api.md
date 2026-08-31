# ADR 0019: Typed Delivery Ledger API

## Status

Accepted

## Context

The first delivery-ledger migration exposed a product-neutral
`DeliveryEnvelope` as `route: String` plus `payload: serde_json::Value`. EKO
then encoded `AgentAddress` and `AgentMessage` into those fields and rebuilt
them on every projection read. The ledger was durable and correct, but the
public shape encouraged application adapters and duplicated the message model.

`echo-agent` is an SDK. Its public delivery API must be generic enough for a
consumer to retain its own route and payload types without moving product
concepts into the framework. This follows the established typed-boundary
pattern used by Serde's generic `Serialize`/`DeserializeOwned` contracts and
message systems such as NATS, where routing metadata and payload are carried as
typed caller-owned values rather than framework-specific domain objects.

## Decision

Make delivery primitives generic over `Route` and `Payload`:

- `DeliveryEnvelope<Route, Payload>` owns the caller's route and payload.
- `DeliveryEvent<Route, Payload>`, `DeliveryRecord<Route, Payload>`,
  `DeliveryLedgerProjection<Route, Payload>`, and `DeliveryLedger<J, Route,
  Payload>` preserve those types through journal, checkpoint, claim, and
  projection APIs.
- Defaults remain `String` and `serde_json::Value` for simple callers.
- `DeliveryRoute` is a small framework contract for route validation; the
  framework supplies the `String` implementation and applications may
  implement it for their own route type.
- `DeliveryRoute` and `DeliveryPayload` require `PartialEq`, not `Eq`, so the
  typed API accepts ordinary serde payloads such as structs containing `f64`.
- `DeliveryEnvelope::new` is the ordinary caller construction path. No
  application-side source-named projection helper is required.
- `DeliveryTransition` is the single lifecycle command. `DeliveryLedger::transition`
  applies it directly, while `prepare_transition` supports a host-owned
  physical journal without introducing an application-side mirror enum. The
  named methods (`begin_effect`, `accept_mailbox`, `mark_drained`, `defer`, and
  `settle`) remain convenience entry points over that same command path.

EKO uses `DeliveryLedger<Journal, AgentAddress, AgentMessage>` directly. Its
product DTOs remain in EKO where they are needed for GUI/CLI wire contracts,
but durable delivery state no longer has a second message, phase, or record
model. No source-named conversion helper or legacy decoder is part of the
development implementation; existing local inbox data is intentionally reset
when the new schema is used.

## Alternatives considered

1. Keep `String`/`Value` and add nicer conversion helpers. Rejected: this
   preserves the duplicated model and only hides the adapter.
2. Move `AgentAddress` and `AgentMessage` into `echo-agent`. Rejected: these
   contain EKO workspace/conversation authorship policy and would couple the
   SDK to one product.
3. Keep a non-generic ledger and add an EKO-specific framework feature.
   Rejected: a feature flag would still pollute the framework and would not
   help other consumers.

## Consequences

- Framework callers get compile-time typed route/payload round trips and a
  concise public API.
- EKO owns only product presentation and workspace policy; it no longer owns a
  second durable delivery reducer or projection conversion.
- The delivery journal schema changes during development. Existing local
  inbox data is intentionally not migrated; a fresh data root is required.
- Public rustdoc examples and the EKO integration contract must cover a typed
  route/payload round trip.
