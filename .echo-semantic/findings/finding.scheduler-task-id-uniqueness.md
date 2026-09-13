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
repair_evidence_refs: []
verification_evidence_refs: []
rereview_audit_refs: []
discovered_at: source:8b3972e1d2bc92f4ad59f511b6674eaaf243c1760f21973caf9e96558f71db90
---

# Scheduler 不保证 CronTask ID 唯一

## 问题

CronTask ID 可公开赋值且 add 不校验唯一；重复 ID 被 last_fired 合并，update/set-status 只更新首项，remove 删除全部同 ID 定义。

## 触发条件与影响

导入、迁移或 programmatic add 产生重复 ID 后，trigger、状态更新和删除会作用于不同定义集合，破坏任务寻址权威。

## 证据

`echo-orchestration/src/scheduler/cron_task.rs` 与 `scheduler/runner.rs` 展示 ID 构造、存储与按 ID 操作。

## 处理记录

Data-durability Audit 确认；无需等待 delivery guarantee 裁决即可增加唯一性约束和迁移检查。
