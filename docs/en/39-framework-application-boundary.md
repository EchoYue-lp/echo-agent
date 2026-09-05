# Framework and Application Boundary

`echo-agent` is a general Agent development framework and toolbox. EKO is one
application built on the framework, not the definition of what the framework
may provide. A product-neutral capability does not need a second outside user
before it can be added to the framework.

## Placement rule

A capability belongs in `echo-agent` when its semantics are complete without
EKO `AppState`, workspace identity, Tauri, UI DTOs, product file layout, or EKO
policy. Its public types must describe a reusable Agent concept, its dependency
direction must remain framework-to-application, and it must have framework-owned
tests, examples, and documentation.

Current adoption count is evidence for API maturity and packaging, not an
admission gate. Waiting for adoption before building the toolbox would make the
framework depend on the users it is meant to attract.

## Boundary rule

Applications may decode user or transport input, select product policy, attach
product metadata, and project framework values to a surface. Generic framework
values themselves must remain the authority: an application must not mirror a
framework struct in a second DTO or expose source-named conversion helpers such
as `to_framework_*` and `from_framework_*`. When a generic capability is
missing, add it to the framework and switch the real application path to that
API before removing the duplicate model.

Public facade paths describe capabilities, not source crates. For example,
`echo_agent::llm` exposes clients and configuration directly, while
`echo_agent::llm::types` is the documented low-level wire surface. Split-crate
migration paths such as `llm::core` or `llm::integration` are not retained.

## Current ownership

| Framework | EKO application |
| --- | --- |
| Agent turn execution, tracked receipts, Task DAG, retry and cancellation | Workspace identity, file-backed task facts, review/worktree policy |
| Tool protocols, ToolManager, artifacts and permission primitives | Direct-user visibility, retention and UI/tool projections |
| Subagent lifecycle and attempt-scoped control | EKO pool policy, workspace generation and surface commands |
| Journal/checkpoint primitives and immutable plugin preparation | Chat payload, retention, target publication and EKO preferences |

`KeyedExecutionAdmission` is the reusable framework primitive for opaque-key
leases, per-key process permits, retirement fences, close, and shutdown waits.
EKO's `AgentPool` wraps it while retaining Agent creation, capacity classes,
workspace transitions, and plugin/model/tool publication policy.

`AgentPool`, `AgentRouter`, `ChatEventLog`, `PluginRuntimeService`, and
`ExtensionControlService` are therefore candidates for kernel extraction only at
their product-neutral seams. They are not approved for whole-module migration.
EKO `AppState`, workspace registry, DomainProfile, research/analysis/browser
policy, review/worktree behavior, and surface projections remain application
concerns.

## One authority

`ExecutionAdmission` is the composition entry point and carries no EKO quota.
`max_concurrent_subagents` remains standalone per-runtime width, while
`max_concurrent_forks` remains the executor fallback when no shared admission
is injected.

The framework and application must share one meaning for a generic lifecycle or
receipt. If a proposed adapter needs its own DAG traversal, status reducer,
retry policy, or durable input lifecycle, the boundary is wrong. Reuse or extend
the existing framework authority, or keep the capability in EKO until a smaller
kernel can be defined without product coupling.

See [ADR 0014](../adr/0014-framework-capability-placement.md) for the decision
record, [ADR 0015](../adr/0015-keyed-execution-admission.md) for keyed admission,
and the cross-repository candidate audit.
