# ADR 0053: Separate Diagnostic Persistence Delivery from Execution

- Date: 2026-09-15
- Owners: `src::trace`, `echo_state::audit`, `src::agent::react`

## Status

Accepted

## Context

Trace and Audit are optional observations of an Agent execution. Their direct
backend APIs already return `Result`, but the React integration discarded
several backend errors. A failed initial trace save still published a run ID,
the default `RunStore::append_event` treated a missing run as success, and
Audit callbacks could drop events without a machine-readable failure fact.
`FileAuditLogger` also returned success after a buffered flush without a
durability barrier and could not recover a crash-torn final JSONL record.

These failures must be visible without allowing a diagnostic backend to
replace the Agent producer's Completed, Failed, or Cancelled terminal.

## Industry References

- The OpenTelemetry
  [error-handling specification](https://github.com/open-telemetry/opentelemetry-specification/blob/main/specification/error-handling.md)
  says runtime telemetry failures must not significantly change instrumented
  application behavior. Suppressed failures should be logged through
  language-specific diagnostics, and SDKs may expose a separate configurable
  error handler.
- The OpenTelemetry Logs SDK
  [export contract](https://github.com/open-telemetry/opentelemetry-specification/blob/main/specification/logs/sdk.md#logrecordexporter-operations)
  returns an explicit Success or Failure to the processor, while ForceFlush
  and Shutdown report error or timeout independently from the application
  operation that produced the log.
- Tokio tracing-appender exposes an
  [ErrorCounter](https://docs.rs/tracing-appender/latest/tracing_appender/non_blocking/struct.ErrorCounter.html)
  for lossy non-blocking output. This makes intentional telemetry loss
  observable without turning a formatting writer into application authority.

The common pattern is to preserve the producer result, represent diagnostic
delivery failure separately, and expose loss through a dedicated result,
handler, or counter.

## Decision

1. `RunStore` and `AuditLogger` remain the only persistence authorities for
   their records. Their direct methods are required operations and continue to
   return `Result`.
2. Agent-integrated Trace and Audit writes are diagnostic delivery. A backend
   failure creates `DiagnosticDeliveryFailure` with a record family,
   operation, occurrence time, optional identity, and backend error. The
   failure never contains the diagnostic payload.
3. `DiagnosticDeliveryObserver` is the notification boundary. The producer
   performs only a bounded non-blocking `try_send`; a process-local diagnostic
   dispatcher emits the stable tracing target
   `echo_agent::diagnostic_delivery` and then invokes an optional application
   observer. Queue saturation, disconnect, dispatcher initialization failure,
   reentrant reporting, and observer unwind increment the saturating
   `diagnostic_delivery_dropped_count`. A blocking observer can delay later
   diagnostic notifications but cannot delay the Agent producer.
4. A failed initial trace save does not publish a trace run ID. Append and
   finalize failures are reported with `append`, `load`, or `finalize`
   operations. Missing runs are failures, including the default
   `RunStore::append_event` implementation.
5. `AuditCallback` and direct React audit sites report rejected records through
   the same observer. They remain observational callbacks. An application that
   requires an audit record as part of a business commit must call
   `AuditLogger::log` directly and bind its `Result` at that commit boundary.
6. A successful `FileAuditLogger::log` means serialization, write, flush, and
   `sync_data` completed. Reopen validates every complete JSONL record. Only an
   invalid final record without a newline is treated as crash-torn and
   truncated; a valid non-newline final record is terminated, while complete
   corruption fails closed without mutation. A lifetime file lease prevents a
   second live logger from racing recovery or append; callers share one logger
   through `Arc`.
7. Metrics and skill-usage telemetry remain a separate best-effort class. They
   may expose counters or warnings but do not create Trace/Audit delivery facts
   and do not change Agent execution.

## Alternatives Rejected

- Failing the Agent turn when Trace or callback Audit persistence fails would
  create a second terminal authority and contradict the producer result.
- Returning only `Option<run_id>` after an initial save failure conflates
  disabled tracing with rejected persistence and can publish phantom IDs.
- Logging unrelated warning strings at each call site is not a stable
  structured contract for filtering, counting, or alerting.
- Adding a retry queue or delivery ledger would require retry, retention,
  shutdown, and ownership policy not needed to close this failure-visibility
  gap. A future durable diagnostic outbox must be a separate decision.

## Consequences

Framework consumers can distinguish execution outcome from diagnostic
completeness. Existing Agent calls keep their producer result when optional
diagnostic persistence fails, while a default tracing subscriber or custom
observer normally receives one structured failure fact for each rejected
operation. The bounded drop counter is the explicit signal that this
best-effort notification path itself lost evidence.

`FileAuditLogger` performs a data durability barrier per record, which costs
more than buffered flush. Consumers that need lower-cost lossy telemetry should
use a telemetry backend rather than weakening the Audit success contract.

A `sync_data` error has an unknown physical outcome: the record may already be
present even though durability was not established. The callback integration
reports the failure and does not retry blindly because AuditEvent has no
idempotency identity. Retry/outbox semantics require a separate contract.

This ADR does not close `finding.in-memory-audit-successful-drop` (#61), which
tracks an in-memory backend that currently reports success after a poisoned
lock, or `finding.trace-audit-secret-boundary` (#103). The observer never adds
record payloads, but custom backend error text remains subject to that separate
retention/redaction audit.

Application-provided observers are trusted local extension code. Running them
off the producer path contains blocking, reentrancy, and unwind panics; Rust
`panic=abort`, process termination, and a permanently blocked diagnostic
dispatcher cannot be recovered in-process. These conditions may lose later
notifications and are reflected only by the counter when the process remains
alive.

This change adds a public observer API but no protocol field, state machine,
store, terminal event, or SDK wire authority.

## Verification

Focused tests cover missing-run append, failed initial trace save and stale ID
clearing, append/load failure observation, Audit callback failure observation,
successful producer completion despite Trace and Audit failures, blocking,
reentrant and unwinding observers, `SyncData`-backed file writes, path
replacement, crash-torn final record recovery, valid non-newline tails, and
complete corruption rejection. Full workspace and SDK contract gates remain
required before closing Finding #46 on main.

Final-save failure coverage uses the canonical `AgentRunSnapshot::finalize_run`:
the initial save and subsequent load succeed, while the terminal save fails.
`FinalizeSaveFailingRunStore` is a private test injector with a bounded save
counter, not a second production persistence or execution authority. The test
observes a Finalize delivery failure without changing the producer terminal.

The public inventory generation and review now belong to the independent
`echo-agent-sdk` repository. Framework verification does not complete that
obligation or close Issue #46; process-local observer/control APIs remain
Host/Rust-only or deferred until the SDK owner refreshes and classifies them.
