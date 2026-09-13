---
schema_version: 1
id: asset.cron-scheduler
kind: asset
title: Cron Scheduler Runtime
asset_type: state_authority
status: needs_review
risk: high
observed_at: f1e9027246760661144786e9e35615cd46d580c6
boundary_refs: [boundary.task-subagent-workflow]
code_refs: [echo-orchestration/src/scheduler/runner.rs, echo-orchestration/src/scheduler/cron_task.rs]
consumer_refs: [echo-agent-learning/examples/demo70_scheduler.rs]
behavior_refs: [behavior.task-subagent-execution]
rule_refs: []
evidence_refs: [evidence.task-subagent-workflow]
finding_refs: [finding.scheduler-cache-delivery]
candidate_refs: []
---

# Cron Scheduler Runtime

## 资产身份

Cron definition store、runner cache、occurrence selection、callback 与 last-run projection 的独立 authority。

## 来源与消费者

Public scheduler API 与 learning example 消费，不与 Task DAG 或 Workflow checkpoint 合并。

## 生命周期

Load/migrate/add/remove/tick/fire/update/shutdown；delivery guarantee 和 cache/store 一致性尚待审计。

## 候选关系

Callback 可调用其它 framework capability，但 Scheduler 不拥有被调用 runtime 的终态。

## 未知与限制

Crash 后 duplicate/loss、run_once/tick 并发与 migration collision 已由 Finding 跟踪。
