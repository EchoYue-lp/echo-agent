---
schema_version: 1
id: finding.k8s-sandbox-cleanup-settlement
kind: finding
type: implementation_bug
status: open
severity: high
primary_focus: result_side_effect
focus: [time_lifecycle, failure_concurrency, state_authority]
boundary_ref: boundary.tool-permission-sandbox
behavior_refs: [behavior.effect-permission-execution]
rule_refs: [rule.permission-effect-order]
evidence_refs: [evidence.effects-extensions]
audit_refs: [audit.tool-permission-sandbox.result-side-effect]
decision_refs: []
repair_evidence_refs: []
verification_evidence_refs: []
rereview_audit_refs: []
discovered_at: f1e9027246760661144786e9e35615cd46d580c6
---

# K8s Sandbox Pod 清理没有可靠 owner settlement

## 问题

Caller drop 只借助 kill_on_drop 终止本地 kubectl，不删除已被 API server 接纳的 Pod；delete_pod 还丢弃 spawn/exit/确认错误，无 detached owner、RAII receipt 或 cleanup debt。

## 触发条件与影响

Stream receiver 关闭、kubectl future drop 或 delete 失败时，Pod 可遗留且调用方仍观察结束/成功，资源泄漏不可诊断。

## 证据

`echo-execution/src/sandbox/k8s.rs` 与 `sandbox/manager.rs` 展示 Pod 创建、future drop 可达性和删除错误处理。

## 处理记录

Result-side-effect Audit 确认 owner gap；后续 repair 需 detached cleanup owner/receipt/debt 与 deterministic kubectl fault tests。
