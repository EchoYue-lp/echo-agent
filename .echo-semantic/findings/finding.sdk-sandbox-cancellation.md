---
schema_version: 1
id: finding.sdk-sandbox-cancellation
kind: finding
type: implementation_bug
status: resolved
severity: high
primary_focus: failure_concurrency
focus: [time_lifecycle, contract_evidence]
boundary_ref: boundary.sdk-facade-parity
behavior_refs: [behavior.sdk-facade-routing]
rule_refs: [rule.sdk-rust-authority]
evidence_refs: [evidence.sdk-contracts]
audit_refs: [audit.sdk-facade-plan08-final]
decision_refs: []
repair_evidence_refs: [evidence.sdk-contracts]
verification_evidence_refs: [evidence.sdk-contracts]
rereview_audit_refs: [audit.sdk-facade-plan08-final]
discovered_at: source:524b2f07633e8b5c757b49785e85280c60149f8ff4129632c3cd1e6bfa71d910
---

# Sandbox bridge 取消分类丢失

## 问题

第四轮审查发现cancel-aware Sandbox回调曾把extension取消映射为ReactError::Other。

## 触发条件与影响

Run取消发生在Sandbox回调执行期间时，下游可能把终态分类为普通失败，且无法证明cleanup已等待。

## 证据

Sandbox proxy现将取消、超时和Sandbox失败恢复为SandboxError；真实Run E2E挂起回调、取消Run，并验证cleanup回调先发生且终态为cancelled。

## 处理记录

代码修复与focused验证已完成；第十轮独立复审确认该finding闭合。
