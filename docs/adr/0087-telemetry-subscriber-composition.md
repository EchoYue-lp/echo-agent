# ADR 0087: Compose process telemetry with an existing subscriber

## Status

accepted for the SDK governance repair on 2026-10-14 after independent design review (0 findings).

## Context

A Host has a stderr tracing subscriber before an accepted telemetry initialization request. Replacing the global subscriber fails; dropping the already-built metrics provider synchronously during rollback blocks the ACP dispatcher. The existing framework owns METRICS, providers, sampling and one-shot lifecycle.

## Decision

Expose a Rust/Host-only stable composition layer and read-only console filter. Formatting has one writer with the existing diagnostic filter OR the console filter; it is not duplicated. Standalone exporters/providers must be ready before installing a subscriber, and failed installation completes rollback without publishing globals. Delegate Tracer and PreSampledTracer operations to the existing runtime using standard BoxedTracer/BoxedSpan, and use standard per-layer dynamic filters. The OTel layer is never wrapped in reload, preserving context downcast. Host-provided console writers choose stderr; standalone initialization preserves its original console behavior. Initialization and shutdown retain their existing API and one-shot semantics. No public language contract or additional telemetry state authority is introduced.

## Alternatives and tradeoffs

Ignoring subscriber failure, exporting only metrics, removing diagnostics or writing console output into ACP stdout breaks existing behavior. Reloading the entire OTel layer loses required downcast support. A stable composed layer uses the existing tracing/OpenTelemetry implementations without copying ID generation, sampling or export scheduling. The approved design is copied to docs/telemetry-composition-design.md for independent repository review.

## Consequences

Existing init/shutdown signatures and one-shot lifecycle remain unchanged. The new composition helper is Rust/Host-only. The SDK must explicitly pin the published framework change and prove real collector receipt and blocking-job ownership.

## Validation

Verify real collector receipt, shutdown flush, valid parent/sampling context, initialization boundaries, standalone/external-subscriber errors and rollback. SDK blocking jobs use the original Host settlement owner; cancellation does not prove completion. All source and delivery gates remain required.
