---
schema_version: 1
id: finding.workflow-parallel-failure-settlement
kind: finding
type: implementation_bug
status: resolved
severity: high
primary_focus: failure_concurrency
focus: [time_lifecycle, result_side_effect, contract_evidence]
boundary_ref: boundary.task-subagent-workflow
behavior_refs: [behavior.task-subagent-execution]
rule_refs: []
evidence_refs: [evidence.task-subagent-workflow, evidence.workflow-parallel-failure-settlement-repair, evidence.workflow-parallel-failure-settlement-verification]
audit_refs: [audit.task-subagent-workflow.failure-concurrency, audit.workflow-parallel-failure-settlement-rereview]
decision_refs: []
repair_evidence_refs: [evidence.workflow-parallel-failure-settlement-repair]
verification_evidence_refs: [evidence.workflow-parallel-failure-settlement-verification]
rereview_audit_refs: [audit.workflow-parallel-failure-settlement-rereview]
discovered_at: f1e9027246760661144786e9e35615cd46d580c6
---

# Workflow 并行 sibling 遮蔽失败并可脱离继续执行

## 外部 Issue

GitHub Issue: https://github.com/EchoYue-lp/echo-agent/issues/113

## 问题

Graph `join_all` 等待全部 branch 后才传播首个错误，无 timeout 时挂起 sibling 可永久遮蔽失败；DagWorkflow spawned handles 按序等待且 caller cancel 时 JoinHandle drop 不停止节点。

## 触发条件与影响

一个 branch 已失败而另一个挂起，或外层 future 被取消时，Workflow 可不结算或让节点脱离继续产生外部副作用。

## 证据

`echo-orchestration/src/workflow/graph.rs` 与 `workflow/dag.rs` 的并行 join/abort 路径构成源码反例。

## 处理记录

commits `90c43d95`与`8c8fdffc4a2dfb341d9b25e32a321c9fb41c7350`完成first-failure
cancel/drain并保留成功结果顺序；Workflow定向测试与独立复审通过。
