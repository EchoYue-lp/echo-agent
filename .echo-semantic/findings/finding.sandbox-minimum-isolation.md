---
schema_version: 1
id: finding.sandbox-minimum-isolation
kind: finding
type: implementation_bug
status: resolved
severity: high
primary_focus: permission_external
focus: [result_side_effect, failure_concurrency, contract_evidence]
boundary_ref: boundary.tool-permission-sandbox
behavior_refs: [behavior.effect-permission-execution]
rule_refs: [rule.permission-effect-order]
evidence_refs: [evidence.effects-extensions, evidence.sandbox-minimum-isolation-repair, evidence.sandbox-minimum-isolation-verification]
audit_refs: [audit.tool-permission-sandbox.failure-concurrency, audit.sandbox-minimum-isolation-rereview]
decision_refs: []
repair_evidence_refs: [evidence.sandbox-minimum-isolation-repair]
verification_evidence_refs: [evidence.sandbox-minimum-isolation-verification]
rereview_audit_refs: [audit.sandbox-minimum-isolation-rereview]
discovered_at: f1e9027246760661144786e9e35615cd46d580c6
---

# Sandbox minimum isolation 可被 fallback 降级

## 外部 Issue

GitHub Issue: https://github.com/EchoYue-lp/echo-agent/issues/83

## 问题

SandboxCommand 将 minimum_isolation 描述为调用方最低要求，SandboxManager 在 allow_fallback=true 时仍可选择更低隔离并执行。

## 触发条件与影响

Docker/K8s 不可用且 auto-detect fallback 开启时，声明最低隔离的 command 可能直接在本机执行。

## 证据

`echo-core/src/sandbox.rs` 与 `echo-execution/src/sandbox/manager.rs` 的 minimum 与 fallback 路径表达冲突。

## 处理记录

`3735f7e0` 使显式 `minimum_isolation` 不再被 fallback 降级；buffered、limited、
stream 路径和不可用 Docker 反例均由当前主线的 manager 测试覆盖（12/12）。
独立 reviewer 核对源码、原反例和验证收据后确认无剩余 blocker，本分支 Finding
标记 resolved。`e8371e58` 的隔离 target 完整本地门禁已通过；PR/CI、远端 main 与
Issue #83 关闭仍待单独交付。
