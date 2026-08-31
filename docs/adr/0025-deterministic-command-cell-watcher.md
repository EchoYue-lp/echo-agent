# ADR 0025: Deterministic Command-Cell Watcher

## Status

Accepted

## Context

The command-cell registry already owns output cursors, retention leases,
terminal causes, artifact state, and retry-safe reads. EKO nevertheless used a
low-reasoning Subagent that repeatedly called the `wait` tool, then read the
same typed registry state again before publishing a result. That added a model
call, Subagent admission, prompt policy, provider failure, and a second status
that could not be authoritative for the command.

The ability to retain and drain one asynchronous command is useful to any
framework consumer and contains no EKO workspace or conversation policy.

## Decision

1. `CommandCellWatcher` is the framework's only retained terminal-observation
   driver. It acquires `CommandCellObservationLease` before an embedding
   application reads its own projection.
2. The watcher repeatedly calls `CommandCellRegistry::wait` with the returned
   byte cursor, retains a UTF-8-safe output tail, and returns only after the
   typed phase is terminal and all currently observable bytes are drained.
3. `CommandCellWatchCancellation` explicitly chooses whether cancellation
   returns immediately or is recorded while the watcher drains to the real
   terminal. Cancellation never changes command-cell truth or stops the
   command implicitly.
4. The framework outcome contains the final `CommandCellSnapshot`, output
   excerpt, cursor/elision facts, and whether cancellation was observed. It has
   no model summary, provider status, Subagent identity, or product delivery
   fields.
5. Embedding applications own address validation, durable receipts, result
   delivery, acknowledgement, recovery, surface projection, and any explicit
   command-stop operation.

## Consequences

- Watching a command consumes no model tokens or Subagent capacity.
- Provider failure and prompt drift can no longer affect command observation.
- Multiple observers remain safe because every watcher uses the existing
  registry lease and cursor contract.
- Applications must not recreate a model-driven polling role or infer terminal
  state from prose, active-process maps, or cancellation alone.
