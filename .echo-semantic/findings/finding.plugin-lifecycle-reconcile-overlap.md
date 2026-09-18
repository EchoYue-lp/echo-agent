---
schema_version: 1
id: finding.plugin-lifecycle-reconcile-overlap
kind: finding
type: implementation_bug
status: resolved
severity: medium
primary_focus: time_lifecycle
focus: [failure_concurrency, result_side_effect, state_authority]
boundary_ref: boundary.extension-lifecycle
behavior_refs: [behavior.extension-publication]
rule_refs: [rule.extension-generation-authority]
evidence_refs: [evidence.effects-extensions]
audit_refs: [audit.extension-lifecycle.time-lifecycle, audit.plugin-lifecycle-reconcile-rereview]
decision_refs: []
repair_evidence_refs: [evidence.plugin-lifecycle-reconcile-repair]
verification_evidence_refs: [evidence.plugin-lifecycle-reconcile-verification]
rereview_audit_refs: [audit.plugin-lifecycle-reconcile-rereview]
discovered_at: f1e9027246760661144786e9e35615cd46d580c6
---

# Plugin lifecycle reconcile 可形成两代资源重叠

## 外部 Issue

GitHub Issue: https://github.com/EchoYue-lp/echo-agent/issues/74

## 问题

旧 Plugin deactivate 失败后，LifecycleManager 仍继续 activate 新集合；失败旧项保持 active/cleanup_required。

## 触发条件与影响

热更新或启停时，旧外部资源未撤销而新代资源又激活，造成重复 hook/process/network effect。

## 证据

`echo-core/src/plugin/lifecycle.rs` 的 reconcile/deactivate failure 顺序提供源码反例。

## 处理记录

Time-lifecycle Audit 确认；后续 repair 应阻断冲突 activate 或显式表示 overlap/debt，并补故障测试。

Issue #74 现在分别跟踪 deactivation 与 shutdown debt，在旧代效果全部结算前阻断每个
activation入口，并覆盖失败重试与init失败清理。独立复审、严格语义验证与完整本地合并
门禁均已通过；外部Issue只在同一快照进入远端main后关闭。
