---
schema_version: 1
id: evidence.plugin-lifecycle-coordinator-repair
kind: evidence
observed_at: source:757f499d4d9a40a4c27791933cb3d5e9d3b2dda76a1a28e4561e317d4719be94
source_refs:
  - src/plugin/coordinator.rs
  - src/plugin.rs
  - src/plugin/prepared.rs
  - echo-core/src/plugin/registry.rs
  - echo-core/src/plugin/lifecycle.rs
  - docs/adr/0069-plugin-host-lifecycle-coordinator.md
supports: [behavior.extension-publication, rule.extension-generation-authority]
limitations:
  - Plugin Hook attempts are ordered and de-duplicated only within one in-process operation; durable delivery remains #58
  - Product-specific filesystem watching and UI projection remain embedding-application policy
  - Independent rereview, remote CI, and mainline delivery remain outstanding
---

# Plugin lifecycle coordinator repair

## 支持的结论

`PluginCoordinator` is the single public framework facade for startup, reconcile, enable, reload,
disable, uninstall, retry and shutdown. It commits `PluginRegistry` desired state first and retains
an explicit `ActualPending` operation receipt when callback or publication effects have not
converged. The coordinator owns only serialized admission, phase order, operation identity and the
last converged receipt.

Existing authorities remain intact: `PluginRegistry` owns durable intent,
`PluginPublicationTarget` owns the Agent-bound generation and cleanup receipt, and
`PluginLifecycleManager` owns callback effects and cleanup debt. The retry path settles callback
cleanup only after dependency resolution and one applicable immutable generation are ready,
then withdraws the exact old wiring receipt and publishes that prevalidated generation,
activates desired callbacks, and only then attempts lifecycle Hook notifications. A failed init or
activation uses `PluginLifecycleManager::reset_for_retry`; the coordinator does not mirror callback
flags. MCP resources continue to use the owner-qualified `McpServerId` from #75.

The registry's dependency topology orders callbacks and events: forward for init/activate/loaded,
reverse for deactivate/unregister/disabled. No-op convergence also checks the exact Agent-bound
publication target. A different Agent is rejected by the #72 target fence before durable intent
changes. Late callback registration invalidates the converged
revision so the next reconcile cannot skip activation.

Invalid dependency resolution or generation-wide preparation stays in `Preparation` before any
callback or wiring withdrawal. The invalid Arc is discarded and the cache invalidated, so repair
followed by retry uses the same operation identity while building a fresh candidate. Persistent
invalid input leaves the old actual generation untouched.
Registry refresh uses the last successful scope selection and swaps the in-memory authority only
after a complete scan. This lets all-scope startup observe later dependency installation without
allowing a Project-only consumer to import Local or User packages during retry.

Repeated reconcile and already-satisfied enable/disable operations are no-ops when registry
revision, active owner set and publication receipt already agree. Explicit reload always advances
the immutable generation. Shutdown removes process-local effects without changing durable enabled
intent, allowing a new process to reconcile the same registry.

## 事件边界

`PluginDisabled` uses the post-withdrawal Hook snapshot, so the removed plugin cannot run its own
cleanup event. Event attempts are marked before await and retried operations never issue the same
attempt twice. Cancellation or process failure after that marker can lose a notification; this
candidate deliberately does not claim cross-process exactly-once delivery and leaves #58 open.
Pending receipts always expose `ActualPending` with the phase set before the next await, including
when a caller drops the transition future.

## 来源与范围

Rollback is the task branch before this candidate or a revert of its eventual squash commit. The
repair adds no second generation counter, registry, callback state machine, or MCP identity. Product
watchers, GUI state, and EKO policy are outside this framework slice.

## 已知缺口

Finding #73 remains open until independent rereview, full gates, remote CI, and mainline delivery
are recorded. Finding #58 remains open for durable Hook producer acknowledgement.
