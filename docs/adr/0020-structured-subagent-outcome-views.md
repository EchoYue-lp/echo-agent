# ADR 0020: Structured Subagent Outcome Views

## Status

Accepted

## Context

`SubagentOutcome` owns the structured result, artifacts, remaining work, and
evidence produced by a framework subagent. A product runtime should be able to
persist and render this value directly; duplicating it into a product result
type makes the framework contract look incomplete and forces needless field
conversion.

## Decision

The framework owns the complete generic outcome and the derived views that can
be defined entirely from its evidence vocabulary:

- `SubagentVerification` and `SubagentVerificationStatus` are returned by
  `SubagentOutcome::verification`.
- `SubagentTouchedFiles` is returned by `SubagentOutcome::touched_files`.
- The runtime refreshes these views after parsing reported output and merging
  observed tool evidence.

EKO persists `SubagentOutcome` directly inside its task-run summary. Task
identity, plan revision, review policy, and other EKO fields remain alongside
the outcome in EKO-owned records, but there is no second result DTO and no
framework-shaped conversion API. GUI/TUI event paths and task persistence use
the same `SubagentOutcome` value.

## Consequences

- Every framework consumer can inspect verification and file-access facts
  without reimplementing evidence parsing.
- EKO no longer duplicates the generic result or derivation algorithms.
- The framework result JSON gains two derived fields; development data can be
  recreated and no legacy result schema is supported.
