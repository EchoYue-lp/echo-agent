---
schema_version: 1
id: finding.scheduler-control-fire-race
kind: finding
type: implementation_bug
status: resolved
severity: high
primary_focus: failure_concurrency
focus: [time_lifecycle, result_side_effect, state_authority]
boundary_ref: boundary.task-subagent-workflow
behavior_refs: [behavior.task-subagent-execution]
rule_refs: []
evidence_refs: [evidence.task-subagent-workflow, evidence.scheduler-occurrence-authority-repair, evidence.scheduler-occurrence-authority-verification]
audit_refs: [audit.task-subagent-workflow.data-durability, audit.scheduler-occurrence-authority-rereview]
decision_refs: []
repair_evidence_refs: [evidence.scheduler-occurrence-authority-repair]
verification_evidence_refs: [evidence.scheduler-occurrence-authority-verification]
rereview_audit_refs: [audit.scheduler-occurrence-authority-rereview]
discovered_at: f1e9027246760661144786e9e35615cd46d580c6
---

# Scheduler disable/remove 与已捕获 callback 不线性一致

## 外部 Issue

GitHub Issue: https://github.com/EchoYue-lp/echo-agent/issues/85

## 问题

Tick 在锁内克隆待执行任务后释放锁；随后成功 disable/remove 不会撤销已经捕获但尚未调用的 callback。

## 触发条件与影响

用户观察控制操作成功后，已被旧 tick 捕获的任务仍可能产生外部副作用，控制状态与执行事实分叉。

## 证据

`echo-orchestration/src/scheduler/runner.rs` 的 to_fire clone、锁释放与 callback 顺序构成源码反例。

## 处理记录

Scheduler以control lock和epoch在线性化点重新接纳occurrence；成功control只允许此前已经接纳
的callback继续，旧definition不能回写重建任务。定向竞态测试与独立复审通过。
