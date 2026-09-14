---
schema_version: 1
id: finding.sandbox-manager-stream-failure-typing
kind: finding
type: implementation_bug
status: open
severity: medium
primary_focus: failure_concurrency
focus: [contract_evidence, result_side_effect]
boundary_ref: boundary.tool-permission-sandbox
behavior_refs: [behavior.effect-permission-execution]
rule_refs: [rule.permission-effect-order]
evidence_refs: [evidence.effects-extensions]
audit_refs: [audit.tool-permission-sandbox.failure-concurrency]
decision_refs: []
repair_evidence_refs: []
verification_evidence_refs: []
rereview_audit_refs: []
discovered_at: f1e9027246760661144786e9e35615cd46d580c6
---

# SandboxManager 建流失败丢失 typed Failed 终态

## 外部 Issue

GitHub Issue: https://github.com/EchoYue-lp/echo-agent/issues/82

## 问题

Backend 建流失败被 SandboxManager 包装成 `Complete(exit_code=-1)`，而核心 stream contract 已定义 `Failed` 终态。

## 触发条件与影响

消费者无法区分命令完成且返回负状态与隔离 backend 根本未启动，导致 retry、diagnostic 与 terminal 投影错误。

## 证据

`echo-execution/src/sandbox/manager.rs` 与 `echo-core/src/sandbox.rs` 展示包装和 typed event 合同。

## 处理记录

Failure-concurrency Audit 确认；后续 repair 保留 Failed 分类并补 backend-start failure stream test。
