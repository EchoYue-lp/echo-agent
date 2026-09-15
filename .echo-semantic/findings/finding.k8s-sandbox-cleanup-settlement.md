---
schema_version: 1
id: finding.k8s-sandbox-cleanup-settlement
kind: finding
type: implementation_bug
status: resolved
severity: high
primary_focus: result_side_effect
focus: [time_lifecycle, failure_concurrency, state_authority]
boundary_ref: boundary.tool-permission-sandbox
behavior_refs: [behavior.effect-permission-execution]
rule_refs: [rule.permission-effect-order]
evidence_refs: [evidence.effects-extensions, evidence.k8s-sandbox-cleanup-settlement-repair, evidence.k8s-sandbox-cleanup-settlement-verification]
audit_refs: [audit.tool-permission-sandbox.result-side-effect, audit.k8s-sandbox-cleanup-settlement-rereview]
decision_refs: []
repair_evidence_refs: [evidence.k8s-sandbox-cleanup-settlement-repair]
verification_evidence_refs: [evidence.k8s-sandbox-cleanup-settlement-verification]
rereview_audit_refs: [audit.k8s-sandbox-cleanup-settlement-rereview]
discovered_at: f1e9027246760661144786e9e35615cd46d580c6
---

# K8s Sandbox Pod 清理没有可靠 owner settlement

## 外部 Issue

GitHub Issue: https://github.com/EchoYue-lp/echo-agent/issues/62

## 问题

Caller drop 只借助 kill_on_drop 终止本地 kubectl，不删除已被 API server 接纳的 Pod；delete_pod 还丢弃 spawn/exit/确认错误，无 detached owner、RAII receipt 或 cleanup debt。

## 触发条件与影响

Stream receiver 关闭、kubectl future drop 或 delete 失败时，Pod 可遗留且调用方仍观察结束/成功，资源泄漏不可诊断。

## 证据

`echo-execution/src/sandbox/k8s.rs` 与 `sandbox/manager.rs` 展示 Pod 创建、future drop 可达性和删除错误处理。

## 处理记录

Result-side-effect Audit 确认 owner gap；当前 repair 已建立 detached cleanup owner/receipt/debt，
并闭合首次NotFound后create延迟提交的竞态。Deterministic fake-kubectl 18项focused tests与两轮
独立集成复审已通过；SDK合同、两档workspace Clippy、完整workspace/all-target/all-feature测试、
no-default-features检查、17-feature矩阵与语义strict/change-evidence门禁全部通过，本Finding已闭合。
GitHub Issue #62继续保持open，直到本提交经MR进入远端main后关闭。
