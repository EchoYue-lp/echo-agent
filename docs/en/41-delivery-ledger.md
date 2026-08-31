# Delivery Ledger

`echo_agent::delivery` provides product-neutral durable delivery
primitives. It keeps the caller's route and payload types intact while the
framework owns ordering, attempts, lifecycle facts, retention, and recovery.

## Public API

The stable SDK facade is available from `echo_agent::delivery`:

```rust
use echo_agent::delivery::{
    DeliveryEnvelope, DeliveryLedger, DeliveryLedgerConfig, DeliveryPayload,
    DeliveryRoute, DeliverySettlement, DeliveryTransition,
};
```

`DeliveryEnvelope<Route, Payload>` is generic. `String` and
`serde_json::Value` are the defaults for simple callers; applications with a
domain address and message type use those types directly:

The complete compile-ready typed lifecycle example is included in the
`DeliveryLedger` rustdoc; it also demonstrates payloads with `f64` fields.

```rust
type Ledger<J> = DeliveryLedger<J, MyAddress, MyMessage>;

fn drive<J>(ledger: &Ledger<J>, address: MyAddress, message: MyMessage) -> echo_agent::error::Result<()> {
    ledger.enqueue(DeliveryEnvelope::new("message-1", address, message))?;
    if let Some(claim) = ledger.claim_next()? {
        ledger.transition(&claim, DeliveryTransition::effect_started("turn-1"))?;
        ledger.transition(
            &claim,
            DeliveryTransition::settled(DeliverySettlement::terminal(
                Some("turn-1".to_string()),
                echo_agent::delivery::DeliveryOutcome::Completed,
                Some(true),
                None,
                None,
            )),
        )?;
    }
    Ok(())
}
```

`DeliveryRoute` validates a caller-owned route. `DeliveryPayload` guarantees a
serde-capable, owned, partially comparable payload that can be retained and
replayed; it intentionally does not require `Eq`, so payloads containing values
such as `f64` remain valid. The projection returns a typed
`DeliveryRecord<Route, Payload>` directly, so callers do not need a source-named
conversion layer.

## Lifecycle

```text
Persisted -> Claimed -> EffectStarted -> MailboxAccepted -> Drained -> TurnSettled
                         \-> Deferred -> Claimed (new attempt)
```

Every claim carries a strictly increasing attempt and opaque attempt ID.
`DeliveryClaim.payload` is the original caller-owned payload selected from the
frontier; route and lifecycle identity remain typed alongside it.
`EffectStarted` carries the actual turn identity. `MailboxAccepted` and
`Drained` must match the same attempt and turn. A retryable settlement requires
the next-attempt time; a terminal settlement leaves the FIFO frontier.

Owner loss is represented by `OutcomeUnknown`; an application may use `Dropped`
when it explicitly retires a delivery. The framework never infers success from
output text or transport EOF, and an effect cannot be replayed without a new
explicit claim.

## Retention and Recovery

Journal sequence, durability, checkpoint recovery, and physical segment
pruning remain owned by `echo_agent::state::journal`. Delivery logical terminal
retention is bounded by both record count and byte size in
`DeliveryLedgerConfig`. Projection state is disposable; recovery replays the
durable typed journal and reapplies the same bounds. Checkpoint recovery also
validates order/frontier membership, route identity, attempt identity, and
phase-specific terminal fields before the state is exposed to a caller.

`DeliveryLedger` composes the existing `EventJournal` and
`CheckpointedReducer` authorities. `PreparedJournalBatch` keeps append
identity stable for retry and reconciliation; an unknown outcome must be
reopened and looked up before any retry.

`JournalDurabilityStatus` is serializable as the canonical tagged durability
value (`unconfirmed`, `confirmed`, or `degraded` with its error). Applications
can expose it directly instead of defining a parallel receipt enum.

Hosts that need custom physical durability or reopen handling can use
`DeliveryLedger::apply_prepared_with`. Its callback supplies only the journal
receipt for the exact prepared batch; the framework verifies the batch identity
and payload digest before folding. Lifecycle preflight, checkpointing,
retention, and post-commit validation remain framework-owned.

`DeliveryTransition` is the single lifecycle command type. Ordinary callers
can pass it to `transition`; hosts that must commit through a custom physical
journal can pass the same value to `prepare_transition`. The older named
methods (`begin_effect`, `accept_mailbox`, `mark_drained`, `defer`, and
`settle`) remain convenience methods over that same command path.

`prepare_claim_next` returns a `DeliveryClaimDraft`; `prepare_transition`
returns a validated `DeliveryEvent` without introducing an application-side
conversion type.

EKO uses `DeliveryLedger<Journal, AgentAddress, AgentMessage>` directly. Its
workspace policy and GUI/TUI/CLI presentation remain application-owned, while
durable delivery state has one framework record and one reducer. This typed API
supersedes the temporary legacy wire bridge; development data roots should be
recreated after the schema change. See [ADR 0019](../adr/0019-typed-delivery-ledger-api.md).
