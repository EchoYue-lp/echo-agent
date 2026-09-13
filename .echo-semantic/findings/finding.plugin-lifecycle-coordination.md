---
schema_version: 1
id: finding.plugin-lifecycle-coordination
kind: finding
type: authority_conflict
status: open
severity: high
primary_focus: time_lifecycle
focus: [state_authority, failure_concurrency, data_durability]
boundary_ref: boundary.extension-lifecycle
behavior_refs: [behavior.extension-publication]
rule_refs: [rule.extension-generation-authority]
evidence_refs: [evidence.effects-extensions]
audit_refs: [audit.extension-lifecycle.state-authority, audit.extension-lifecycle.time-lifecycle]
decision_refs: []
repair_evidence_refs: []
verification_evidence_refs: []
rereview_audit_refs: []
discovered_at: f1e9027246760661144786e9e35615cd46d580c6
---

# Plugin Registry、wiring 与 callback lifecycle 未统一编排

## 外部 Issue

GitHub Issue: https://github.com/EchoYue-lp/echo-agent/issues/73

## 问题

Registry disable/uninstall、Integrator unwire 与 LifecycleManager deactivate/shutdown 是三段状态，未发现一个生产入口确保顺序和 cleanup debt 闭合；PluginLoaded/Disabled 也未发现 emitter。

## 触发条件与影响

Plugin reload/disable/uninstall 或 callback failure 时，持久 enabled 状态、live components、callbacks 和 Hook events 可能分离。

## 证据

`echo-core/src/plugin/registry.rs`、`plugin/lifecycle.rs`、`src/plugin/prepared.rs` 与 Hook event definitions 提供证据。

## 处理记录

Discovery 记录；下一阶段沿真实 host consumer 审计，不把 EKO policy 整体下沉 framework。
