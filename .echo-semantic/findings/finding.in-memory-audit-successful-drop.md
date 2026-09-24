---
schema_version: 1
id: finding.in-memory-audit-successful-drop
kind: finding
type: implementation_bug
status: resolved
severity: medium
primary_focus: contract_evidence
focus: [data_durability, failure_concurrency]
boundary_ref: boundary.observation-persistence-delivery
behavior_refs: [behavior.observation-persistence]
rule_refs: [rule.fact-projection-separation]
evidence_refs: [evidence.persistence-observation, evidence.audit-poison-current-repair, evidence.audit-poison-current-verification]
audit_refs: [audit.observation-persistence-delivery.contract-evidence, audit.audit-poison-lock-rereview]
decision_refs: []
repair_evidence_refs: [evidence.audit-poison-current-repair]
verification_evidence_refs: [evidence.audit-poison-current-verification]
rereview_audit_refs: [audit.audit-poison-lock-rereview]
discovered_at: f1e9027246760661144786e9e35615cd46d580c6
---

# InMemoryAuditLogger 丢写仍返回成功

## 外部 Issue

GitHub Issue: https://github.com/EchoYue-lp/echo-agent/issues/61

## 问题

InMemoryAuditLogger 在写锁 poisoned 时不保存事件却返回 `Ok(())`，调用方无法区分接纳成功和静默丢弃。

## 触发条件与影响

任一先前 panic poison 锁后，后续 audit 记录持续丢失但上层仍观察成功，违反 logger 的可诊断接纳语义。

## 证据

`echo-state/src/audit/memory.rs` 展示 lock error 分支；现有 tests 没有 poisoned-lock 故障注入。

## 处理记录

`3735f7e0` 使 poisoned write lock 恢复内部 guard 后再 push；当前主线 `f7c1fef7`
的故障注入回归 1/1 通过。独立 reviewer 核对源码、原反例和验证收据后确认无剩余
blocker，本分支 Finding 标记 resolved。`e8371e58` 的隔离 target 完整本地门禁已通过；
PR/CI、远端 main 与 Issue #61 关闭仍待单独交付。
