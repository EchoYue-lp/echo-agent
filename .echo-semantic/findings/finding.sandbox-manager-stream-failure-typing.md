---
schema_version: 1
id: finding.sandbox-manager-stream-failure-typing
kind: finding
type: implementation_bug
status: resolved
severity: medium
primary_focus: failure_concurrency
focus: [contract_evidence, result_side_effect]
boundary_ref: boundary.tool-permission-sandbox
behavior_refs: [behavior.effect-permission-execution]
rule_refs: [rule.permission-effect-order]
evidence_refs: [evidence.effects-extensions, evidence.sandbox-manager-stream-failure-typing-repair, evidence.sandbox-manager-stream-failure-typing-verification]
audit_refs: [audit.tool-permission-sandbox.failure-concurrency, audit.sandbox-manager-stream-failure-typing-rereview]
decision_refs: []
repair_evidence_refs: [evidence.sandbox-manager-stream-failure-typing-repair]
verification_evidence_refs: [evidence.sandbox-manager-stream-failure-typing-verification]
rereview_audit_refs: [audit.sandbox-manager-stream-failure-typing-rereview]
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

Failure-concurrency Audit 确认；`3f63bf78` 将 SandboxManager backend 建流失败改为
typed `SandboxStreamEvent::Failed`，并集中复用 SandboxError 到 stream failure 的映射。
真实 manager caller 入口的 backend-start failure regression 已通过；独立复审 PASS，
本 Finding 在当前候选分支标记 resolved。完整门禁、PR/CI、远端 main 与 Issue #82
关闭仍待交付验收。
