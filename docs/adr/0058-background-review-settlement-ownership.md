# ADR 0058: Background Review Settlement Ownership

- Date: 2026-09-18
- Owners: `src::evolution` maintainers

## Status

Accepted

## Context

`BackgroundReviewer::review` previously spawned a Tokio task and returned its
`JoinHandle`. Dropping that handle detached an auto-persisting review, so model
failure, panic, partial memory mutation, and shutdown were no longer observable
by the caller. The framework also exposed `persisted: bool`, which could not
represent a write that started but failed after a partial effect.

The framework is reusable and does not own EKO workspace leases or application
shutdown. EKO already has a review registry that owns those product concerns.

## Options

1. Keep returning a detached `JoinHandle` and document that callers must retain
   it. Rejected because Rust deliberately detaches a Tokio task when the handle
   is dropped.
2. Add a second framework task supervisor and durable debt store. Rejected
   because it would duplicate application lifecycle and persistence ownership.
3. Return a lazy caller-owned future, classify the write result honestly, and
   let embedding applications admit that future into their existing lifecycle
   owner. Chosen.

## Decision

`review`, `review_and_wait`, and `review_by_run_id` return a lazy
`BackgroundReviewHandle`. Polling it directly drives the caller-owned
operation; the framework never calls `tokio::spawn`, creates a receipt registry,
or owns cancellation. `ReviewIdentity` is available before the first poll and
binds the source `run_id` to the deterministic memory `persistence_key`. The
existing `ReviewOutcome.run_id` and persistence action continue to identify
settled or unknown writes without adding required fields to public result
structs; cancellation before settlement is reconciled from the handle identity.

`ReviewCandidate.persisted` is tri-state: `Some(true)` means the write settled,
`Some(false)` means no write was attempted, and `None` means an attempted write
has an unknown result and must be reconciled using the identity-bound memory
journal before retry. Applications that run reviews in the background must admit
the handle into their own generation/lease owner, retain its identity, and
perform shutdown cancellation and evidence settlement there. Application-
specific workspace leases, evidence inbox persistence, and UI projection remain
outside the framework.

## Consequences

The old synchronous `review(...) -> JoinHandle<_>` source shape is removed.
Callers that need foreground behavior simply await the handle. Background
callers own the task and its receipt/lease. An unpolled handle has no receipt to
wait on and no effect to settle. Process abort and panic-abort remain outside an
in-process receipt; partial store mutations are reported as unknown rather than
being described as rolled back, but the stable handle identity remains available
for reconciliation before settlement.

[ADR 0065](0065-evolution-memory-audit-reconciliation.md) gives the layered
memory mutation behind an accepted review a durable operation identity. Its
startup reconciliation closes the framework audit gap but does not replace the
application's review admission, shutdown, or result ownership.

## Verification

Framework tests cover unpolled futures, cancellation, panic, partial writes,
zero budget, and the three public entry points. EKO tests cover rejection before
poll, observer drop, admission closure, shutdown drain, and evidence settlement.
