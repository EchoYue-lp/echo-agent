# ADR 0069: Host-level plugin lifecycle coordinator

## Status

Accepted

## Context

Plugin lifecycle state is split across three deliberate authorities:

- `PluginRegistry` durably owns installed packages and enabled intent.
- `PluginPublicationTarget` owns one Agent's active immutable generation,
  publication receipt, and wiring cleanup debt.
- `PluginLifecycleManager` owns callback effects and callback cleanup debt.

Each authority is correct within its boundary, but hosts previously had to
compose registry mutation, callback withdrawal, generation rollback,
publication, callback activation, and lifecycle Hook events themselves. A
partial reload could therefore leave durable desired state ahead of actual
runtime state without one receipt describing what must be retried.

This is a reusable framework concern: every host that exposes plugin lifecycle
operations needs the same ordering. Product policy, UI projections, filesystem
watching, and application-specific approval remain outside the framework.

The design follows the common controller model used by Kubernetes: durable
desired state is committed independently, and a serialized reconciler moves
actual state toward it while retaining retryable status. Temporal's Saga and
idempotency guidance similarly favors explicit compensations and stable
operation identities over pretending that external effects share one database
transaction. VS Code's extension lifecycle separates activation and
deactivation callbacks from package discovery. Echo Agent keeps those
authorities separate and adds only their missing coordinator.

## Decision

`PluginCoordinator` is the sole public framework entry point for host-level
`startup`, `reconcile`, `enable`, `reload`, `disable`, `uninstall`, retry, and
`shutdown` transitions. It owns only:

- exclusive transition admission;
- a monotonic operation identity and phase receipt;
- the deterministic retry order between existing authorities; and
- the last converged plugin set and publication receipt.

It does not copy registry entries, publication generation state, callback
state, or MCP identities. Registry mutations commit desired state first.
Failures after that point return `ActualPending` and retain the canonical
operation receipt. Retry resumes the failed phase in this order:

1. resolve dependencies and build one applicable immutable generation;
2. settle old callback effects;
3. withdraw the exact old publication receipt;
4. publish the prevalidated generation;
5. activate callbacks for the desired set; and
6. attempt lifecycle notifications after the runtime commit.

Dependency or generation-wide preparation failure occurs before any withdrawal,
so the old actual generation remains live. The pending operation keeps its
identity and `Preparation` phase but discards the invalid Arc and invalidates the
Integrator cache. Retry therefore observes repaired package files and prepares
a new candidate; persistent invalid input cannot advance actual state.
Registry refresh is commit-on-success and reuses the last successful scope
selection. A Host that established a Project-only view cannot import User or
Local packages during retry; all-scope startup records all scopes and can
observe a dependency installed later in any of them.

The registry's dependency resolution is the ordering authority. Callback init,
activation, publication, and `PluginLoaded` attempts use dependency-first
order. Callback withdrawal, uninstall/shutdown cleanup, and `PluginDisabled`
attempts use reverse dependency order.

Unresolved callback or wiring cleanup blocks all later generations. A
cancelled publication is recovered through
`PluginPublicationTarget::pending_cleanup_receipt`; no second generation or MCP
owner identity is introduced. Plugin MCP connections continue to use the
owner-qualified `McpServerId` from ADR 0067.

If callback init or activation fails, retry first asks
`PluginLifecycleManager::reset_for_retry` to settle deactivate/shutdown debt
while retaining the same registration. Only that existing authority decides
when cleanup is complete; the coordinator does not mirror callback flags.

`shutdown` withdraws actual state without changing durable enabled intent, so
the next process can reconcile the same registry. `uninstall` delegates final
plugin callback shutdown to `PluginLifecycleManager`; disable retains callback
registration for a later enable.

Reconcile and already-satisfied enable/disable requests are idempotent when the
registry revision, dependency-ordered active plugin set, and the exact
Agent-bound `PluginPublicationTarget` match the last committed receipt. A
different Agent is rejected by the existing #72 target fence rather than
reusing `Converged` or mutating durable intent. The target check precedes every
registry, callback, or publication side effect. Registering new lifecycle callbacks invalidates the
converged revision so the next reconcile initializes and activates them.
Explicit reload always prepares and publishes a newer generation.

`PluginLoaded` and `PluginDisabled` are notification attempts, not another
transaction authority. They run only after component publication and callback
activation have committed. Disabled notifications use the post-withdrawal Hook
snapshot, so the removed plugin cannot run its own cleanup event. The operation
receipt marks an event attempted before awaiting it; retry therefore preserves
order and does not issue the same event twice. Cancellation or process failure
can still lose a notification after that marker. Cross-process durable Hook
delivery and acknowledgement remain the open Hook producer contract in #58.
Every unfinished receipt projects `ActualPending`; because the phase advances
before each await, dropping a transition future preserves the precise retry
phase rather than leaving a stale running status.

## Alternatives

- Put registry, wiring, and callbacks into one new state machine: rejected
  because it would duplicate three existing authorities and make their receipts
  disagree.
- Let each embedding application coordinate the phases: rejected because the
  ordering and cleanup invariants are product-independent framework behavior.
- Roll registry state back when runtime publication fails: rejected because
  durable desired state must remain explicit; hiding the gap prevents reliable
  retry and restart reconciliation.
- Claim durable exactly-once lifecycle Hooks: rejected because the current Hook
  executor has no durable acknowledgement protocol. #58 owns that broader
  contract.

## Consequences

Hosts gain one retryable lifecycle facade and can distinguish `Converged` from
`ActualPending`. A failed actual transition can leave the registry's desired
state committed by design, but no later generation can pass its debt. Restart
creates a fresh coordinator and reconciles durable intent into a fresh Agent.
Lifecycle notifications are ordered and de-duplicated within one in-process
operation, with the documented cancellation/crash delivery gap.

## References

- [Kubernetes controllers](https://kubernetes.io/docs/concepts/architecture/controller/)
- [Temporal compensating actions](https://docs.temporal.io/encyclopedia/workflow-message-passing)
- [VS Code extension lifecycle](https://code.visualstudio.com/api/references/activation-events)
- [ADR 0012](0012-immutable-plugin-preparation.md)
- [ADR 0060](0060-plugin-lifecycle-reconcile-settlement.md)
- [ADR 0067](0067-mcp-owner-qualified-identity.md)
