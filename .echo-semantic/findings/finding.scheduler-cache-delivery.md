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
evidence_refs: [evidence.task-subagent-workflow]
audit_refs: [audit.task-subagent-workflow.data-durability]
decision_refs: []
repair_evidence_refs: []
verification_evidence_refs: []
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

Data-durability Audit 确认 cache/migration 缺口；跨 crash missed occurrence、retry 与 delivery guarantee 仍需 semantic-decide。
