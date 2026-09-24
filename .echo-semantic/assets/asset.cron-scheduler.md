---
schema_version: 1
id: asset.cron-scheduler
kind: asset
title: Cron Scheduler Runtime
asset_type: state_authority
status: active
risk: high
observed_at: source:757f499d4d9a40a4c27791933cb3d5e9d3b2dda76a1a28e4561e317d4719be94
boundary_refs: [boundary.task-subagent-workflow]
code_refs: [echo-orchestration/src/scheduler/runner.rs, echo-orchestration/src/scheduler/cron_task.rs]
consumer_refs: [echo-agent-learning/examples/demo70_scheduler.rs]
behavior_refs: [behavior.task-subagent-execution]
rule_refs: []
evidence_refs: [evidence.task-subagent-workflow, evidence.scheduler-occurrence-authority-repair, evidence.scheduler-occurrence-authority-verification]
finding_refs: [finding.scheduler-cache-delivery]
candidate_refs: []
---

# Cron Scheduler Runtime

## 资产身份

Cron definition store、runner cache、occurrence selection、callback 与 last-run projection 的独立 authority。

## 来源与消费者

Public scheduler API 与 learning example 消费，不与 Task DAG 或 Workflow checkpoint 合并。

## 生命周期

Load/migrate/add/remove/tick/fire/update/shutdown；runner 持有单进程定义写入许可，
Store 是持久权威，cache 是派生快照。Occurrence 使用 DeliveryLedger 的持久
claim/attempt/settlement，callback effect 仍按 occurrence ID 自行幂等。

## 候选关系

Callback 可调用其它 framework capability，但 Scheduler 不拥有被调用 runtime 的终态。

## 未知与限制

真实进程 kill、跨进程 Store writer、不同 backend 对象指向同一物理存储及离线
misfire 不由当前测试和单进程 owner 合同证明。
