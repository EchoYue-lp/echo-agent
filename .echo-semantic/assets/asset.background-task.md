---
schema_version: 1
id: asset.background-task
kind: asset
title: Process-local BackgroundTask
asset_type: state_authority
status: needs_review
risk: high
observed_at: f1e9027246760661144786e9e35615cd46d580c6
boundary_refs: [boundary.task-subagent-workflow]
code_refs: [echo-orchestration/src/tasks/background_task.rs, echo-orchestration/src/tasks/background_state.rs]
consumer_refs: [docs/en/16-background-tasks.md]
behavior_refs: [behavior.task-subagent-execution]
rule_refs: []
evidence_refs: [evidence.task-subagent-workflow]
finding_refs: [finding.background-task-wait]
candidate_refs: [asset.command-cell-runtime, asset.task-graph-authority]
---

# Process-local BackgroundTask

## 资产身份

TaskSpawner/BackgroundTask future handle 与遗留 BackgroundTaskState checkpoint contract 的 process-local capability。

## 来源与消费者

Public framework API、文档与 tests 消费；没有 root Agent 构造点不构成删除依据。

## 生命周期

Spawn/run/cancel/wait/panic/drop；wait/多观察者/terminal 缺口由 Finding 跟踪。

## 候选关系

与 revisioned Task graph、CommandCell 的 typed process lifecycle 存在待审关系，但不在 discovery 中自动归并。

## 未知与限制

BackgroundTaskState checkpoint 长期定位仍在 discovery unresolved 中。
