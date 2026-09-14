---
schema_version: 1
id: asset.workflow-runtime
kind: asset
title: Workflow Graph 与 Continuation Checkpoint
asset_type: state_authority
status: candidate
risk: high
observed_at: f1e9027246760661144786e9e35615cd46d580c6
boundary_refs: [boundary.task-subagent-workflow]
code_refs: [echo-orchestration/src/workflow/mod.rs, echo-orchestration/src/workflow/graph.rs, echo-orchestration/src/workflow/checkpoint_store.rs, echo-orchestration/src/workflow/dag.rs]
consumer_refs: [echo-agent-learning/tests/example_contracts/demo34_workflow_stream.rs, src/workflow/loader.rs]
behavior_refs: [behavior.task-subagent-execution]
rule_refs: [rule.task-subagent-authority]
evidence_refs: [evidence.task-subagent-workflow]
finding_refs: [finding.workflow-dag-authority, finding.workflow-entry-loop-drift, finding.workflow-checkpoint-claim-recovery]
candidate_refs: [asset.task-graph-authority]
---

# Workflow Graph 与 Continuation Checkpoint

## 资产身份

Generic Workflow graph/state、continuation checkpoint 与 event stream 的独立 framework runtime。

## 来源与消费者

Public workflow APIs、declarative loader 与 learning contracts 消费；不由 Task graph 自动替代。

## 生命周期

Compile/run/interrupt/checkpoint/claim/resume/stream；多入口与 crash recovery 缺口由 Findings 跟踪。

## 候选关系

Graph、DagWorkflow 与 Task DAG 的算法重叠进入 consolidation audit，不包含 Scheduler/BackgroundTask。

## 未知与限制

NodeError producer、多入口一致性与 claim recovery 尚未闭合。
