---
schema_version: 1
id: finding.command-cell-retention-lease-prune-race
kind: finding
type: implementation_bug
status: resolved
severity: high
primary_focus: failure_concurrency
focus: [time_lifecycle, state_authority, contract_evidence]
boundary_ref: boundary.task-subagent-workflow
behavior_refs: [behavior.task-subagent-execution, behavior.effect-permission-execution]
rule_refs: []
evidence_refs: [evidence.task-subagent-workflow, evidence.effects-extensions, evidence.command-cell-cancel-artifact-settlement-repair, evidence.command-cell-cancel-artifact-settlement-verification]
audit_refs: [audit.task-subagent-workflow.time-lifecycle, audit.command-cell-settlement-rereview]
decision_refs: []
repair_evidence_refs: [evidence.command-cell-cancel-artifact-settlement-repair]
verification_evidence_refs: [evidence.command-cell-cancel-artifact-settlement-verification]
rereview_audit_refs: [audit.command-cell-settlement-rereview]
discovered_at: f1e9027246760661144786e9e35615cd46d580c6
---

# CommandCell retention prune 可删除并发新 lease

## 外部 Issue

GitHub Issue: https://github.com/EchoYue-lp/echo-agent/issues/45

## 问题

Prune 先扫描 lease 计数并收集 key，随后排序并无条件 remove；删除前不复核 lease，watcher/waiter 可在 scan 与 remove 之间成功取得 lease。

## 触发条件与影响

观察者已获 retained observation lease 后，cell 仍可能被 prune，下一次 wait 返回 NotFound，违反 ADR 0025 多观察者合同。

## 证据

`echo-orchestration/src/tasks/command_cell.rs`、`echo-core/src/tools/cell.rs` 与 ADR 0025 展示 lease/prune 顺序。

## 处理记录

Time-lifecycle Audit 确认。retention 删除现使用 DashMap `remove_if` 在 shard 锁内原子复核 terminal、waiter lease 和 observation lease；新增 lease 竞态测试与 lease drop 后收敛测试通过，repair、verification 与独立复审已闭合本 Finding。
