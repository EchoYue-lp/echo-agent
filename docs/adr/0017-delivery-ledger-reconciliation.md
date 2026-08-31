# ADR 0017: Delivery Ledger Prepared Reconciliation

## Context

EKO's AgentRouter must change its durable authority without weakening the
framework journal contract. A caller can lose the result of an append after the
write has started, so a plain event API is not enough for an adapter that must
reopen and distinguish an absent frame from an already committed one.

## Decision

`DeliveryLedger` exposes `apply_prepared` and `lookup_batch`. The prepared batch
keeps its stable batch identity and payload digest across retries. `NotCommitted`
may return the same batch for an immediate retry; `OutcomeUnknown` and identity
conflicts require a verified reopen and read-only lookup first. An
`AlreadyCommitted` lookup is reconciled from the retained journal frame and
never appended again.

The lifecycle also permits an adapter to defer an `EffectStarted` admission
when a typed rejection proves that mailbox acceptance and drain did not occur;
that transition clears the provisional turn identity. After mailbox acceptance,
the attempt must settle with its existing identity.

The existing single-event `apply` API delegates to this path. Hosts that need
custom physical durability or reopen handling may use
`DeliveryLedger::apply_prepared_with`; its callback returns the journal receipt
for the exact prepared batch. The framework verifies that identity and payload
digest before projection preflight, checkpoint folding, logical retention, and
committed-invariant checks. The callback cannot mutate lifecycle state or bypass
the reducer.

The same boundary is available for high-level lifecycle operations: the
`prepare_*` methods generate and preflight typed events, and a host commits
those events through the hook. This keeps claim and settlement construction in
the framework even when the physical journal is application-owned.

## Consequences

Framework consumers can migrate durable authorities without inventing a second
retry loop or mailbox. Reconciliation remains product-neutral, while the
application still decides how to reopen files, report unknown outcomes, and
protect product-specific data.
