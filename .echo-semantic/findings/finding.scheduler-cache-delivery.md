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
audit_refs: [audit.task-subagent-workflow.data-durability, audit.scheduler-occurrence-authority-rereview]
decision_refs: [decision-adr-0042-scheduler-occurrence-authority]
repair_evidence_refs: [evidence.scheduler-occurrence-authority-repair]
verification_evidence_refs: [evidence.scheduler-occurrence-authority-verification]
rereview_audit_refs: [audit.scheduler-occurrence-authority-rereview]
discovered_at: f1e9027246760661144786e9e35615cd46d580c6
---

# Scheduler public store mutation 与 runner cache 未闭合

## 外部 Issue

GitHub Issue: https://github.com/EchoYue-lp/echo-agent/issues/84

## 问题

Durable occurrence、callback settlement、migration collision 和 runner 自身 mutation 已闭合，
但 caller 可保留 public `CronTaskStore` clone 并在 runner 构造后直接 mutation，使 runner cache
继续观察旧 definition。

## 触发条件与影响

直接 disable/remove/update 与 `list_tasks`、`tick` 或 `run_once` 并发时，durable store 已提交，
runner 仍可能列出或执行旧 Enabled definition。

## 证据

`echo-orchestration/src/scheduler/runner.rs` 与 `scheduler/cron_task.rs` 的更新、cache 和 migration 路径提供证据。

## 处理记录

Framework main 已复用 `DeliveryLedger` 建立 durable occurrence：owner loss 写
`OutcomeUnknown` 并以同一 occurrence ID、新 attempt 重投，known callback result 形成 terminal；
external effect exactly-once 由 callback 按 occurrence ID 幂等实现。Store-owned definition
incarnation 与 durable control revision 阻断 remove/re-add 及 disable/enable ABA，取消后的 runner
不构造 callback。

当前 independent rereview 在 `main@e15cc17f` 发现 public Store mutation 仍可绕过 runner
cache 同步。该 framework 缺口进入 `scheduler-store-cache-authority` Outcome；consumer data-root、
SDK mapping 与 website 不再作为 Issue #84 的 blocker。Cron offline misfire 与跨进程 Store writer
继续是明确非目标。
