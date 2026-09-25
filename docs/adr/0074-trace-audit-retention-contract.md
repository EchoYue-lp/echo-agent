# ADR 0074: Trace and Audit Retention at the Producer Boundary

## Status

Accepted

- Date: 2026-09-25
- Owners: `src::trace`, `echo_core::audit`, `echo_state::audit`

## Context

Trace and Audit are optional diagnostic observations, but custom `RunStore` and
`AuditLogger` implementations are valid framework extension points. The
built-in in-memory and JSONL backends already sanitize content, while a custom
backend can otherwise receive raw input, output, tool arguments, or callback
errors. A partial write also cannot be represented as a successful diagnostic
delivery without losing the distinction between accepted state and durable
failure.

The records contain two different classes of data. Content fields may contain
user or model text and secrets. Typed identity and effect fields (run/session/
turn/execution IDs, call IDs, tool names, paths, statuses, counters and
timestamps) are required to address and correlate a diagnostic record. Treating
every string as redactable would break lookup and replay; treating every string
as safe would make the retention promise false.

## Decision

1. Framework producers sanitize a producer-owned copy with the default
   `ContentRetentionPolicy` before calling custom trace `save`/`append_event`
   and Audit mutation methods. The default trace finalizer sanitizes its
   standalone output/error strings. `Run::apply_retention`,
   `RunEvent::apply_retention`, and `AuditEvent::apply_retention` define the
   reusable record boundary; custom finalizers use
   `ContentRetentionPolicy::sanitize_text` for their standalone strings.
2. A custom backend that overrides a mutation method must apply the same or a
   stricter policy before durable acceptance. Built-in backends re-apply their
   configured policy at their ingestion/read boundaries.
3. Content fields are redacted and bounded; typed addressing/effect fields are
   preserved for diagnostics. Callers must not put credentials in identities
   that are intended to remain queryable.
4. A backend returns `Err` when a write is partial or durability is unknown.
   The producer keeps the accepted state observable and emits a separate
   diagnostic-delivery failure; backend failure never becomes a second Agent
   execution terminal.

## Alternatives considered

- Redact every string, including IDs and paths: rejected because it destroys
  diagnostic addressing and typed replay facts.
- Trust every custom backend to sanitize without a producer boundary: rejected
  because a normal React integration would silently leak content to an
  extension sink.
- Treat a partial write as success: rejected because callers could not tell
  whether the retained state is durable and recovery could not be reasoned
  about.
- Add a second framework-wide audit store or delivery ledger: rejected; the
  existing `RunStore`/`AuditLogger` remain the persistence authorities and
  failure visibility is a separate observation.

## Consequences

Custom backend authors can reuse the public retention helpers and must preserve
the typed identity/error contract. Applications receive sanitized content even
when they provide only a custom sink through the normal Agent producer path.
Direct calls to a custom backend remain that backend's responsibility. The
policy does not claim to recognize every possible secret format, and it does
not impose file-size or time-based retention.

## Verification

Focused regressions cover InMemory and JSONL trace retention, `AuditCallback`
custom sinks, typed identities that remain addressable under zero limits, and
custom trace/audit sinks that retain a sanitized partial record while returning
an observable persistence error. Final workspace gates and independent rereview
remain delivery responsibilities.

## References

- [ADR 0053](0053-trace-audit-persistence-visibility.md)
- [`ContentRetentionPolicy`](../../echo-core/src/utils/retention.rs)
- `.echo-semantic/findings/finding.trace-audit-secret-boundary.md`
