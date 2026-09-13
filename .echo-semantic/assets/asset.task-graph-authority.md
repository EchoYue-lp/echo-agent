---
schema_version: 1
id: asset.task-graph-authority
kind: asset
title: Revisioned Task Graph Authority
asset_type: state_authority
status: active
risk: high
observed_at: f1e9027246760661144786e9e35615cd46d580c6
boundary_refs: [boundary.task-subagent-workflow]
code_refs: [echo-orchestration/src/tasks/revisioned.rs, echo-orchestration/src/tasks/runtime.rs, echo-orchestration/src/tasks/runtime_service.rs, echo-orchestration/src/tasks/runtime_executor.rs]
consumer_refs: [src/tasks.rs, src/agent/subagent/team/mod.rs, tests/facade_smoke.rs]
behavior_refs: [behavior.task-subagent-execution]
rule_refs: [rule.task-subagent-authority]
evidence_refs: [evidence.task-subagent-workflow]
finding_refs: [finding.task-patch-claim-race, finding.task-subagent-attempt-link, finding.workflow-dag-authority]
candidate_refs: [asset.workflow-runtime]
---

# Revisioned Task Graph Authority

## 资产身份

Task spec/execution、revision、claim、ready frontier、retry/cancel/pause/settlement 的 canonical authority。

## 来源与消费者

Task tools、Team runtime、SDK task facade 与应用 controller 消费。

## 生命周期

Create/patch/load、claim attempt、dispatch、resolve/requeue/pause/cancel、reload safe point。

## 候选关系

与 generic Workflow graph 相邻，是否存在重复需 audit；不得先合并。

## 未知与限制

Relation revision 与 concurrent claim patch 的交错已形成 Finding。
