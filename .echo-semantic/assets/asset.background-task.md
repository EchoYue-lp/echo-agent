---
schema_version: 1
id: asset.background-task
kind: asset
title: Process-local BackgroundTask
asset_type: state_authority
status: needs_review
risk: high
observed_at: source:d61c2341a008920576462b3051374115cf1b4da682c341852b052224f022d027
boundary_refs: [boundary.task-subagent-workflow]
code_refs: [echo-orchestration/src/tasks/background_task.rs, echo-orchestration/src/tasks/background_state.rs, docs/adr/0039-background-task-terminal-authority.md]
consumer_refs: [docs/en/29-long-running-tasks.md, docs/zh/29-long-running-tasks.md, echo-sdk-protocol/tests/facade_inventory.rs, scripts/check-language-sdks.sh]
behavior_refs: [behavior.task-subagent-execution]
rule_refs: []
evidence_refs: [evidence.task-subagent-workflow, evidence.background-task-terminal-authority-repair, evidence.background-task-terminal-authority-verification]
finding_refs: [finding.background-task-wait]
candidate_refs: [asset.command-cell-runtime, asset.task-graph-authority]
---

# Process-local BackgroundTask

## 资产身份

TaskSpawner/BackgroundTask future handle与公开BackgroundTaskState checkpoint contract是相邻但独立的process-local capability；私有BackgroundTaskHandleState只拥有handle lifecycle。

## 来源与消费者

Public framework API、文档与 tests 消费；没有 root Agent 构造点不构成删除依据。

## 生命周期

Spawn/admit/run/cancel/deadline/wait/panic/terminal/drop；handle state原子保存status、单消费者result与typed panic provenance，Clone只共享观察与取消scope。

## 候选关系

与公开BackgroundTaskState checkpoint、revisioned Task graph和CommandCell typed process lifecycle保持不同owner，不自动归并。

## 未知与限制

Handle terminal authority已有repair、verification与rereview证据；公开BackgroundTaskState checkpoint长期定位仍在discovery unresolved中。
