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
evidence_refs: [evidence.provider-protocol-quality, evidence.evolution-memory-audit-repair, evidence.evolution-memory-audit-verification]
audit_refs: [audit.eval-evolution.data-durability]
decision_refs: []
repair_evidence_refs: [evidence.evolution-memory-audit-repair]
verification_evidence_refs: [evidence.evolution-memory-audit-verification, evidence.foundation-36-72-51-integration-verification]
rereview_audit_refs: []
discovered_at: f1e9027246760661144786e9e35615cd46d580c6
---

# Evolution mutation 与 change audit 非原子

## 外部 Issue

GitHub Issue: https://github.com/EchoYue-lp/echo-agent/issues/51

## 问题

Memory write/promote/demote 先改变 Store，再写 change audit；audit 失败时 mutation 已可见，与“所有 mutation recorded”承诺冲突。

## 触发条件与影响

Audit 文件写入失败或进程在两步间中断时，memory/skill state 无对应变更记录，rollback 和审阅链失去事实。

## 证据

`src/evolution/layer.rs` 与 `src/evolution/audit.rs` 的 mutation/audit 顺序及模块承诺提供证据。

## 处理记录

候选实现已把分层记忆 write/delete/status/layer/budget/approved merge 接到
durable prepare、业务 audit 幂等提交与重启 reconcile；ADR 0065 明确原始 Store
读者的中间态和事后 rollback 边界。当前状态保持 open，等待完整工程门禁、独立
rereview 与远端主线交付，不能用本分支 focused 结果提前关闭 Issue。
