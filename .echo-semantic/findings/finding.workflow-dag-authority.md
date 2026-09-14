---
schema_version: 1
id: finding.workflow-dag-authority
kind: finding
type: consolidation_candidate
status: open
severity: medium
primary_focus: state_authority
focus: [contract_evidence, failure_concurrency]
boundary_ref: boundary.task-subagent-workflow
behavior_refs: [behavior.task-subagent-execution]
rule_refs: [rule.task-subagent-authority]
evidence_refs: [evidence.task-subagent-workflow]
audit_refs: [audit.task-subagent-workflow.failure-concurrency]
decision_refs: []
repair_evidence_refs: []
verification_evidence_refs: []
rereview_audit_refs: []
discovered_at: f1e9027246760661144786e9e35615cd46d580c6
candidate_asset_refs: [asset.task-graph-authority, asset.workflow-runtime]
decision: defer
---

# Task DAG 与 Workflow DAG 平行实现候选

## 外部 Issue

GitHub Issue: https://github.com/EchoYue-lp/echo-agent/issues/111

## 问题

Revisioned Task graph、Workflow Graph 与 DagWorkflow 各自拥有节点、边、校验和执行算法，但当前 ADR 没有定义它们长期保持独立的完整语义边界。

## 触发条件与影响

新增 DAG 能力或修复失败恢复时，多个 loop 可能得到不同状态、事件和 checkpoint 行为；直接删除又会破坏合理 public framework API。

## 证据

`echo-orchestration/src/tasks`、`workflow/graph.rs` 与 `workflow/dag.rs` 展示三类实现和不同消费者。

## 处理记录

Failure-concurrency Audit 已确认三者合同不同，不支持直接归并；下一步以 ADR 决定 keep-separate，并仅审查可共享的纯算法，不据当前采用量删除。
