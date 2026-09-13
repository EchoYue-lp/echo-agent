---
schema_version: 1
id: finding.evolution-audit-atomicity
kind: finding
type: implementation_bug
status: open
severity: high
primary_focus: data_durability
focus: [result_side_effect, failure_concurrency, state_authority]
boundary_ref: boundary.eval-evolution
behavior_refs: [behavior.eval-evolution]
rule_refs: [rule.quality-observation-boundary]
evidence_refs: [evidence.provider-protocol-quality]
audit_refs: [audit.eval-evolution.data-durability]
decision_refs: []
repair_evidence_refs: []
verification_evidence_refs: []
rereview_audit_refs: []
discovered_at: f1e9027246760661144786e9e35615cd46d580c6
---

# Evolution mutation 与 change audit 非原子

## 问题

Memory write/promote/demote 先改变 Store，再写 change audit；audit 失败时 mutation 已可见，与“所有 mutation recorded”承诺冲突。

## 触发条件与影响

Audit 文件写入失败或进程在两步间中断时，memory/skill state 无对应变更记录，rollback 和审阅链失去事实。

## 证据

`src/evolution/layer.rs` 与 `src/evolution/audit.rs` 的 mutation/audit 顺序及模块承诺提供证据。

## 处理记录

Discovery 记录；下一阶段设计 prepare/commit/reconcile 或 durable outbox，不用 trace 替代业务 commit。
