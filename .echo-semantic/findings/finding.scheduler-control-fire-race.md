---
schema_version: 1
id: finding.scheduler-control-fire-race
kind: finding
type: implementation_bug
status: open
severity: high
primary_focus: failure_concurrency
focus: [time_lifecycle, result_side_effect, state_authority]
boundary_ref: boundary.task-subagent-workflow
behavior_refs: [behavior.task-subagent-execution]
rule_refs: []
evidence_refs: [evidence.task-subagent-workflow]
audit_refs: [audit.task-subagent-workflow.data-durability]
decision_refs: []
repair_evidence_refs: []
verification_evidence_refs: []
rereview_audit_refs: []
discovered_at: f1e9027246760661144786e9e35615cd46d580c6
---

# Scheduler disable/remove 与已捕获 callback 不线性一致

## 问题

Tick 在锁内克隆待执行任务后释放锁；随后成功 disable/remove 不会撤销已经捕获但尚未调用的 callback。

## 触发条件与影响

用户观察控制操作成功后，已被旧 tick 捕获的任务仍可能产生外部副作用，控制状态与执行事实分叉。

## 证据

`echo-orchestration/src/scheduler/runner.rs` 的 to_fire clone、锁释放与 callback 顺序构成源码反例。

## 处理记录

Data-durability Audit 确认；后续 repair/decision 需明确 disable/remove 对 admitted occurrence 的语义并提供 invocation identity。
