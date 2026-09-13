---
schema_version: 1
id: finding.in-memory-audit-successful-drop
kind: finding
type: implementation_bug
status: open
severity: medium
primary_focus: contract_evidence
focus: [data_durability, failure_concurrency]
boundary_ref: boundary.observation-persistence-delivery
behavior_refs: [behavior.observation-persistence]
rule_refs: [rule.fact-projection-separation]
evidence_refs: [evidence.persistence-observation]
audit_refs: [audit.observation-persistence-delivery.contract-evidence]
decision_refs: []
repair_evidence_refs: []
verification_evidence_refs: []
rereview_audit_refs: []
discovered_at: f1e9027246760661144786e9e35615cd46d580c6
---

# InMemoryAuditLogger 丢写仍返回成功

## 问题

InMemoryAuditLogger 在写锁 poisoned 时不保存事件却返回 `Ok(())`，调用方无法区分接纳成功和静默丢弃。

## 触发条件与影响

任一先前 panic poison 锁后，后续 audit 记录持续丢失但上层仍观察成功，违反 logger 的可诊断接纳语义。

## 证据

`echo-state/src/audit/memory.rs` 展示 lock error 分支；现有 tests 没有 poisoned-lock 故障注入。

## 处理记录

Contract-evidence Audit 确认；后续 repair 返回 typed error 或明确声明/暴露 best-effort drop。
