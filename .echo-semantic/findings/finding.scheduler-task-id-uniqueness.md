---
schema_version: 1
id: finding.scheduler-task-id-uniqueness
kind: finding
type: authority_conflict
status: open
severity: high
primary_focus: state_authority
focus: [data_durability, failure_concurrency]
boundary_ref: boundary.task-subagent-workflow
behavior_refs: [behavior.task-subagent-execution]
rule_refs: []
evidence_refs: [evidence.task-subagent-workflow]
audit_refs: [audit.task-subagent-workflow.data-durability]
decision_refs: []
repair_evidence_refs: [evidence.scheduler-occurrence-authority-repair]
verification_evidence_refs: []
rereview_audit_refs: []
discovered_at: f1e9027246760661144786e9e35615cd46d580c6
---

# Scheduler 不保证 CronTask ID 唯一

## 外部 Issue

GitHub Issue: https://github.com/EchoYue-lp/echo-agent/issues/86

## 问题

CronTask ID 可公开赋值且 add 不校验唯一；重复 ID 被 last_fired 合并，update/set-status 只更新首项，remove 删除全部同 ID 定义。

## 触发条件与影响

导入、迁移或 programmatic add 产生重复 ID 后，trigger、状态更新和删除会作用于不同定义集合，破坏任务寻址权威。

## 证据

`echo-orchestration/src/scheduler/cron_task.rs` 与 `scheduler/runner.rs` 展示 ID 构造、存储与按 ID 操作。

## 处理记录

Data-durability Audit 确认；无需等待 delivery guarantee 裁决即可增加唯一性约束和迁移检查。
