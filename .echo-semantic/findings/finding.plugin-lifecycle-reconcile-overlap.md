---
schema_version: 1
id: finding.plugin-lifecycle-reconcile-overlap
kind: finding
type: implementation_bug
status: open
severity: medium
primary_focus: time_lifecycle
focus: [failure_concurrency, result_side_effect, state_authority]
boundary_ref: boundary.extension-lifecycle
behavior_refs: [behavior.extension-publication]
rule_refs: [rule.extension-generation-authority]
evidence_refs: [evidence.effects-extensions]
audit_refs: [audit.extension-lifecycle.time-lifecycle]
decision_refs: []
repair_evidence_refs: []
verification_evidence_refs: []
rereview_audit_refs: []
discovered_at: source:8b3972e1d2bc92f4ad59f511b6674eaaf243c1760f21973caf9e96558f71db90
---

# Plugin lifecycle reconcile 可形成两代资源重叠

## 问题

旧 Plugin deactivate 失败后，LifecycleManager 仍继续 activate 新集合；失败旧项保持 active/cleanup_required。

## 触发条件与影响

热更新或启停时，旧外部资源未撤销而新代资源又激活，造成重复 hook/process/network effect。

## 证据

`echo-core/src/plugin/lifecycle.rs` 的 reconcile/deactivate failure 顺序提供源码反例。

## 处理记录

Time-lifecycle Audit 确认；后续 repair 应阻断冲突 activate 或显式表示 overlap/debt，并补故障测试。
