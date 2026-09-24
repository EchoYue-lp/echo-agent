---
schema_version: 1
id: finding.scheduler-cache-delivery
kind: finding
type: authority_conflict
status: resolved
severity: high
primary_focus: data_durability
focus: [state_authority, failure_concurrency, time_lifecycle, result_side_effect]
boundary_ref: boundary.task-subagent-workflow
behavior_refs: [behavior.task-subagent-execution]
rule_refs: [rule.task-subagent-authority]
evidence_refs: [evidence.task-subagent-workflow, evidence.scheduler-occurrence-authority-repair, evidence.scheduler-occurrence-authority-verification]
audit_refs: [audit.task-subagent-workflow.data-durability, audit.scheduler-occurrence-authority-rereview]
decision_refs: [decision-adr-0042-scheduler-occurrence-authority]
repair_evidence_refs: [evidence.scheduler-occurrence-authority-repair]
verification_evidence_refs: [evidence.scheduler-occurrence-authority-verification]
rereview_audit_refs: [audit.scheduler-occurrence-authority-rereview]
discovered_at: f1e9027246760661144786e9e35615cd46d580c6
---

# Scheduler public store mutation 与 runner cache 权威闭合

## 外部 Issue

GitHub Issue: https://github.com/EchoYue-lp/echo-agent/issues/84

## 问题

原独立复审在 `main@e15cc17f` 确认：durable occurrence、callback settlement、
migration collision 和 runner 自身 mutation 已闭合，但 caller 可保留 public
`CronTaskStore` clone 并在 runner 构造后直接 mutation，使 runner cache 继续观察旧 definition。

## 触发条件与影响

修复前，直接 disable/remove/update 与 `list_tasks`、`tick` 或 `run_once` 并发时，
durable store 已提交，runner 仍可能列出或执行旧 Enabled definition。
当前实现先在 Store mutation lock 下取得 runner 写入许可并加载初始 cache；
runner 存活期间，保留 clone、同路径 file handle、同一 backend instance 的公开写入均拒绝。

## 证据

`echo-orchestration/src/scheduler/runner.rs` 与 `scheduler/cron_task.rs` 的更新、cache 和 migration 路径提供证据。

## 处理记录

Framework main 已复用 `DeliveryLedger` 建立 durable occurrence：owner loss 写
`OutcomeUnknown` 并以同一 occurrence ID、新 attempt 重投，known callback result 形成 terminal；
external effect exactly-once 由 callback 按 occurrence ID 幂等实现。Store-owned definition
incarnation 与 durable control revision 阻断 remove/re-add 及 disable/enable ABA，取消后的 runner
不构造 callback。

`main@e15cc17f` 的 independent rereview 发现 public Store mutation 可绕过 runner
cache 同步；本轮在任务分支上将同一路径、共享 backend 与迁移写入收敛到一个 runner
mutation owner，并覆盖 caller 取消后的已提交写入与 owner 释放后的重新开放。
最新 lane-local focused tests 为 scheduler 58 passed，独立只读复审无 action item。
此处 `resolved` 仅表示 Finding 在当前工作树的 framework 实现与证据闭合；Issue #84
本地完整门禁已通过，仍待 PR/CI、合入远端 main 后逐项关闭。Consumer data-root、SDK mapping 与
website 不作为本 Finding 的 blocker。Cron offline misfire、跨进程 Store writer、
不同 backend 对象指向同一物理存储和直接 backend 修改均不在此单进程合同内。
