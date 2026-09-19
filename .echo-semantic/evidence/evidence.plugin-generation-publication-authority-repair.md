---
schema_version: 1
id: evidence.plugin-generation-publication-authority-repair
kind: evidence
observed_at: source:3a8aba9cee4bdf94c17e2039bb9bc22ebcf6fb112bfe26ce5686002569409849
source_refs:
  - src/plugin/prepared.rs
  - src/agent/react/mod.rs
  - src/plugin.rs
  - docs/adr/0012-immutable-plugin-preparation.md
  - docs/adr/0060-plugin-lifecycle-reconcile-settlement.md
supports: [behavior.extension-publication, rule.extension-generation-authority]
limitations:
  - Plugin Registry persistence and callback lifecycle are not part of one host transaction (#73)
  - MCP server owner-qualified identity remains a separate Finding (#75)
  - Independent rereview, integration gates, and mainline delivery remain outstanding
---

# Plugin target publication authority repair

## 支持的结论

An immutable prepared generation remains cached by `PluginIntegrator`; a checked process-wide
allocator orders snapshots from independent Integrators. Active
publication state and the canonical receipt now live on each `ReactAgent`. Independent Agents
using the same cloned integrator have separate authority. All public apply and rollback calls
resolve that Agent-bound authority; the arbitrary map-based unwire entry is no longer public.

The target checks generation order and active or pending cleanup before side effects. Successful
apply issues a private token and binds the receipt to generation, content identity and its complete
component inventory. Withdrawal checks the token's identity and unchanged receipt against the
canonical record. A successful withdrawal is repeatable until the next generation publishes;
old prepared snapshots and receipts then fail with typed stale errors. Failed apply does not
advance the published generation. Failed or cancelled cleanup leaves its receipt in the target
for explicit retry, and no new generation can publish while the debt remains.
MCP servers are visited deterministically and each successful connection enters the canonical
receipt before the next awaits. A previously absent target reserves its cleanup name before a
fallible connection await; if connection fails, that name is settled or remains cleanup debt.

## 来源与范围

ADR 0012 is the single immutable-preparation and per-target publication decision, with ADR 0060
continuing to own callback cleanup. It uses Kubernetes resource-version-style optimistic fencing
on the actual target; no EKO product policy or integrator-global active generation state was added.
Rollback of this candidate is the task branch before merge; after delivery, revert the task's
squash commit as one unit. Removing only the target fence while retaining its receipt API would
reopen this Finding.

## 已知缺口

Registry、callback 与 MCP owner 的边界分别由 #73、#75 及相应 Finding 验收。
本候选不声明 Finding resolved，独立复审与主线交付仍需完成。
