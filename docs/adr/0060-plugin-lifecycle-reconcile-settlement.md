# ADR 0060: Fail-Closed Plugin Callback Reconciliation

## Status

Accepted

## Context

`PluginLifecycleManager::reconcile` deactivated callbacks absent from the desired set and then
activated the desired callbacks even when an old `deactivate` failed. The old registration remained
active with cleanup debt, so two callback owners could run effects at once. The split
`deactivate_not_in` / `activate_enabled` API and direct `activate` exposed the same bypass. An
`activate` callback can also partially start effects before reporting failure.

ADR 0012 owns immutable preparation, and ADR 0045 DU-71 forbids overlapping Plugin generations.
This decision concerns callback ownership inside the existing manager. Registry persistence,
component wiring, and full host coordination remain separate authorities tracked by #72 and #73.

Kubernetes controllers reconcile actual state toward desired state and retry unresolved work;
desired state alone is not proof that an old resource was removed. Tokio's graceful shutdown
guidance separates requesting cancellation from waiting for tasks to finish. Here callbacks are
synchronous: a successful cleanup callback is the settlement signal, while a failed callback
retains debt and blocks activation.

## Decision

- `PluginLifecycleManager` retains its existing `cleanup_required` state for any callback that
  may have partially acquired resources. Any such debt blocks all new callback activation,
  including `reconcile`, `activate_enabled`, and direct `activate`.
- A successful retry of an active callback's `deactivate` clears its cleanup debt. For failed
  initialization or activation, `unregister` must complete cleanup before the registration can
  be replaced. A failed `shutdown` is tracked separately: later `deactivate` success does not
  settle it. Failed `init` also requires shutdown because it may have partially acquired
  resources. `unregister` retries only the cleanup stages still outstanding. The existing
  registration remains the retry owner on cleanup failure.
- The manager still attempts the remaining deactivations and reports their errors. It does not
  publish a replacement callback while any old cleanup is unresolved. Already active callbacks
  without their own debt retain idempotent `activate` behavior; repeated reconciliation after
  settlement does not start them again.

## Alternatives

- Proceed with unaffected desired callbacks after a failed withdrawal: rejected because callback
  effects can overlap and the manager cannot prove they are independent.
- Clear debt after a failed callback or on `Drop`: rejected because neither proves external
  cleanup finished.
- Introduce another generation store or host coordinator here: rejected because the current
  manager already owns callback state, and #72/#73 own broader publication coordination.

## Consequences

Failure in one callback temporarily blocks activation of others until the failed owner settles;
callers receive errors and can retry withdrawal or unregister. This closes callback overlap but
does not claim that Registry state, wired components, and callbacks form one atomic transaction.

## References

- [Kubernetes controller pattern](https://kubernetes.io/docs/concepts/architecture/controller/)
- [Tokio graceful shutdown](https://tokio.rs/tokio/topics/shutdown)
- [ADR 0012](0012-immutable-plugin-preparation.md)
- [ADR 0045](0045-confirmed-semantic-governance-decisions.md)
