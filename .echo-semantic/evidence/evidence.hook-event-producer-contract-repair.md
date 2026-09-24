---
schema_version: 1
id: evidence.hook-event-producer-contract-repair
kind: evidence
observed_at: f4ba41cf3e0c52ceb45b4cf9f92530aec64fc84c
source_refs:
  - echo-core/src/hooks/types.rs
  - src/agent/react/run/pipeline.rs
  - src/agent/react/run/context.rs
  - src/agent/react/run/phases/
  - src/agent/react/capabilities.rs
  - src/agent/react/mod.rs
  - src/agent/subagent/executor.rs
  - src/plugin/coordinator.rs
  - src/hooks_bridge.rs
  - src/evolution/runtime_integration.rs
  - docs/adr/0073-hook-event-producer-contract.md
  - docs/en/23-hooks.md
  - docs/zh/23-hooks.md
  - docs/en/07-skills.md
  - docs/zh/07-skills.md
  - tests/hook_event_producer_contract.rs
supports: [behavior.extension-publication, behavior.observation-persistence, rule.extension-generation-authority]
limitations:
  - This candidate changes the documentation and contract evidence only; it does not add a PermissionDenied producer or any new Hook dispatch path.
  - Host-owned bridges and the opt-in evolution observer still require the embedding application to wire them at its authoritative lifecycle boundary.
  - Focused contract validation is complete only after the new integration test passes; full workspace gates, independent rereview, and mainline delivery remain outstanding.
---

# HookEvent producer contract repair evidence

## Supported conclusions

The 31 `HookEvent` names remain a stable catalog, but the consumer contract no
longer treats catalog membership as automatic producer coverage. The bilingual
Hooks and Skills guides and ADR 0073 classify every event exactly once as
`framework-auto`, `host-owned`, or `no-producer`.

The source audit found framework-owned producers for the tool pipeline,
session/run finalization and preparation, compression, skill discovery,
parallel-tool settlement, terminal failure, and PluginCoordinator event phase.
Task events remain application-owned through `TaskHookBridge`; the default
`ReactAgent` installs `SubagentExecutor::unified_hook_executor`, so Subagent
events are framework-auto per dispatch attempt. Four Evolution events are
emitted only through an opt-in `HookEvolutionObserver`; four Evolution
variants have no observer callback and remain catalog-only. `PermissionDenied`,
`Notification`, and `ConfigChange` remain intentionally unproduced for their
future dedicated boundaries. `SubagentHookBridge` is an explicit alternative
adapter for a host-owned external runtime, not an additional producer to wire
alongside the default executor.

## Authority and scope

ADR 0073 is the decision authority for the classification. The Markdown matrix
is the consumer-facing projection, and
`tests/hook_event_producer_contract.rs` is the executable drift check: it
requires both bilingual matrices to cover `HookEvent::ALL` exactly once and to
agree with the explicit source-level classification.

No second Hook producer, permission authority, or lifecycle state machine was
introduced. Generic `ReactAgent::fire_lifecycle_hook` remains a manual escape
hatch and does not prove that an event is automatically emitted.

## Known gaps

This evidence does not claim Issue #37's `PermissionDenied` producer, the
application-side task lifecycle wiring, or a framework producer for
the four catalog-only Evolution variants. Independent review, full gates,
semantic snapshot refresh, remote CI, and mainline delivery remain pending.
