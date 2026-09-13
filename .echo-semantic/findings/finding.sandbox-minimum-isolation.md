---
schema_version: 1
id: finding.sandbox-minimum-isolation
kind: finding
type: implementation_bug
status: open
severity: high
primary_focus: permission_external
focus: [result_side_effect, failure_concurrency, contract_evidence]
boundary_ref: boundary.tool-permission-sandbox
behavior_refs: [behavior.effect-permission-execution]
rule_refs: [rule.permission-effect-order]
evidence_refs: [evidence.effects-extensions]
audit_refs: [audit.tool-permission-sandbox.failure-concurrency]
decision_refs: []
repair_evidence_refs: []
verification_evidence_refs: []
rereview_audit_refs: []
discovered_at: source:8b3972e1d2bc92f4ad59f511b6674eaaf243c1760f21973caf9e96558f71db90
---

# Sandbox minimum isolation 可被 fallback 降级

## 问题

SandboxCommand 将 minimum_isolation 描述为调用方最低要求，SandboxManager 在 allow_fallback=true 时仍可选择更低隔离并执行。

## 触发条件与影响

Docker/K8s 不可用且 auto-detect fallback 开启时，声明最低隔离的 command 可能直接在本机执行。

## 证据

`echo-core/src/sandbox.rs` 与 `echo-execution/src/sandbox/manager.rs` 的 minimum 与 fallback 路径表达冲突。

## 处理记录

Discovery 记录；下一阶段区分 minimum 与 preferred，确保 minimum 不被静默降级。
