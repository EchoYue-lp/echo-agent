---
schema_version: 1
id: finding.scheduler-cache-delivery
kind: finding
type: authority_conflict
status: open
severity: high
primary_focus: data_durability
focus: [state_authority, failure_concurrency, time_lifecycle, result_side_effect]
boundary_ref: boundary.task-subagent-workflow
behavior_refs: [behavior.task-subagent-execution]
rule_refs: [rule.task-subagent-authority]
evidence_refs: [evidence.task-subagent-workflow, evidence.scheduler-occurrence-authority-repair, evidence.scheduler-occurrence-authority-verification]
audit_refs: [audit.task-subagent-workflow.data-durability]
decision_refs: []
repair_evidence_refs: [evidence.scheduler-occurrence-authority-repair]
verification_evidence_refs: [evidence.scheduler-occurrence-authority-verification]
rereview_audit_refs: []
discovered_at: f1e9027246760661144786e9e35615cd46d580c6
---

# Scheduler store、cache 与 callback delivery 未闭合

## 外部 Issue

GitHub Issue: https://github.com/EchoYue-lp/echo-agent/issues/84

## 问题

Fire 后只更新 CronTaskStore 而不刷新 runner cache，list 可返回旧 last-run；callback effect 与 store update 没有持久 claim，legacy migration 还可能覆盖已有目标 backend。

## 触发条件与影响

定时任务执行、进程崩溃、run-once/tick 并发或 migration target 已存在时，观察状态、重复/丢失和持久定义可能不一致。

## 证据

`echo-orchestration/src/scheduler/runner.rs` 与 `scheduler/cron_task.rs` 的更新、cache 和 migration 路径提供证据。

## 处理记录

Data-durability Audit 确认 cache/migration 缺口。当前候选复用 framework DeliveryLedger，
将已持久 occurrence 定义为 callback at-least-once：owner loss 写 OutcomeUnknown 并以同一
occurrence ID、新 attempt 重投，known callback result 形成 terminal；external effect
exactly-once 由 callback 按 occurrence ID 幂等实现。Store-owned definition incarnation与
durable control revision阻断remove/re-add及disable/enable ABA。Cron offline misfire不在本合同内。

候选commit `573ee8b2`已在main基线`b71f03ba`的新worktree中集成，尚未提交。
新增cancelled replay反例证明取消后的runner不构造callback；修复后scheduler全部30项通过。
完整affected-crate重跑388项通过，但首次出现一次非scheduler workflow lease时序失败，
详见verification Evidence，不将重跑绿当成该问题已修复。最终独立rereview、共享semantic
snapshot、change-evidence、workspace及17-feature集成门禁仍未闭合。
SDK已提取到独立仓库，新增public API契约由SDK owner更新；CLI仍需stable data-root
occurrence anchor及测试。因此Finding保持open，不宣称main-ready，外部Issue不关闭。
