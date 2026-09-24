# ADR 0073: Hook Event Producer Contract

## Status

Accepted

- Date: 2026-09-25
- Owners: `echo-core/hooks`, `echo-execution/skills/hooks`, framework runtime adapters

## Context

`HookEvent::ALL` lists 31 serializable names, while the English and Chinese
guides previously read as though every name had a framework-owned automatic
producer. In particular, `PermissionDenied` has a context factory but no
dedicated dispatch, and `Notification`/`ConfigChange` only have a generic
manual lifecycle helper. Four Evolution events are only enum/context branches.
The other four Evolution events require an opt-in `HookEvolutionObserver`;
Task events require the embedding application's bridge, while the default
`ReactAgent` installs a unified executor that emits Subagent lifecycle events.
Registration and dispatch are separate facts. A valid Hook rule for a catalog
event cannot by itself create a runtime producer.

## Options Considered

1. Auto-emit every catalog event from `ReactAgent`. This would invent lifecycle
   facts where the framework does not own the state transition, and would
   duplicate the application task, subagent, config and notification owners.
2. Delete every event without an automatic producer. This would remove useful
   public extension names and break consumers that explicitly dispatch them.
3. Keep the catalog and classify each event by actual producer ownership.
   Only add a new producer after the owning state transition and delivery
   contract are established.

## Decision

Every catalog event has exactly one documented producer class:

- `framework-auto`: a framework-owned runtime path emits it when that path is
  used. This is conditional on that path, not a guarantee on every turn.
- `host-owned`: the embedding application invokes the existing event-specific
  bridge or observer at its authoritative boundary. The framework does not
  infer the event from unrelated state.
- `no-producer`: the enum and context can be configured or manually
  constructed, but no current framework path emits it. The event must not be
  advertised as an automatic callback.

The bilingual `23-hooks.md` guides hold the complete 31-row matrix; the
`hook_event_producer_contract` integration test checks that both matrices
cover `HookEvent::ALL` exactly once with the same classification.

`PermissionDenied` remains `no-producer` until Issue #37 binds an approval
receipt to the final effective tool call and chooses a single denied-effect
boundary. This ADR does not add a second permission authority or synthesize
the event through `fire_lifecycle_hook`, which currently rejects tool events.

`Notification` and `ConfigChange` also remain `no-producer`; the generic
`fire_lifecycle_hook` helper is not a dedicated producer.

`PostMemoryWrite`, `MemoryLayerChange`, `SkillCandidateDetected`, and
`SkillHealthCheck` have observer callbacks, but a host must wire
`HookEvolutionObserver` into the relevant manager/detector/monitor. The
remaining four Evolution variants have no dedicated callback. Task bridges
are host-owned because the embedding application owns the corresponding runtime
state. Subagent events are framework-produced by the unified executor
installed by `ReactAgent`. `SubagentHookBridge` remains an explicit adapter
for a host-owned external runtime, but a host must choose either that adapter
or the default executor for each dispatch attempt; wiring both duplicates
Start/Stop. Plugin events are framework-produced only when
`PluginCoordinator` performs its publication/withdrawal event phase; use of
raw plugin pieces is not an implicit event guarantee.

## Consequences

- Hook registration remains backward compatible. No event name, wire format,
  producer API, or runtime authority changes in this documentation slice.
- A consumer can now distinguish a configured rule from a guaranteed producer.
  Adding or changing a producer requires a source-path test and synchronized
  updates to both guides and the contract test.
- Issue #58 remains open until independent review, integrated validation, and
  mainline evidence confirm the matrix. Issue #37 separately owns the
  `PermissionDenied` producer decision.

## References

- `echo-core/src/hooks/types.rs`: event catalog and context factories.
- `src/agent/react/run/pipeline.rs`, `src/agent/react/run/phases/`,
  `src/agent/react/mod.rs`, `src/agent/subagent/executor.rs`,
  `src/plugin/coordinator.rs`: framework-owned producers.
- `src/hooks_bridge.rs`, `src/evolution/runtime_integration.rs`: opt-in adapters.
- `docs/en/23-hooks.md`, `docs/zh/23-hooks.md`: consumer-facing matrix.
