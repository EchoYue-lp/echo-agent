# ADR 0018: AgentRouter Legacy Wire Boundary

## Status

Superseded by [ADR 0019](0019-typed-delivery-ledger-api.md) during development.
The legacy bridge described here is no longer an active path.

## Context

Phase 2 moved AgentRouter lifecycle, FIFO, retry, checkpoint and retention
authority into the product-neutral `DeliveryLedger`. The application still
needs to read existing EKO journals and checkpoints whose serialized event
shape predates the framework contract.

## Decision

The framework remains the only lifecycle and projection authority. EKO keeps a
thin `LegacyDeliveryJournal` adapter that maps framework events to the existing
`AgentInboxEvent` wire while preserving the physical journal path, sequence,
batch identity and lookup/reopen behavior. A minimal EKO checkpoint codec is
used only to bootstrap a framework checkpoint when a legacy journal has already
pruned its prefix.

The application no longer owns an `EventReducer<AgentInboxEvent>`, frontier
state, terminal retention algorithm, or old checkpoint writer. The legacy wire
and checkpoint codec cannot mutate lifecycle state independently and are not a
second mailbox or reducer.

## Consequences

Existing EKO data remains recoverable without a file-format rewrite. Framework
consumers get one lifecycle contract, while EKO retains only the product-specific
serialization bridge needed for local historical data. The wire codec can be
removed after a separately audited data migration; release readiness is not
inferred from its presence or absence.
