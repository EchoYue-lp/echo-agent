---
schema_version: 1
id: finding.workflow-parallel-failure-settlement
kind: finding
type: implementation_bug
status: open
severity: high
primary_focus: failure_concurrency
focus: [time_lifecycle, result_side_effect, contract_evidence]
boundary_ref: boundary.task-subagent-workflow
behavior_refs: [behavior.task-subagent-execution]
rule_refs: []
evidence_refs: [evidence.task-subagent-workflow]
audit_refs: [audit.task-subagent-workflow.failure-concurrency]
decision_refs: []
repair_evidence_refs: []
verification_evidence_refs: []
rereview_audit_refs: []
discovered_at: source:8b3972e1d2bc92f4ad59f511b6674eaaf243c1760f21973caf9e96558f71db90
---

# Workflow 并行 sibling 遮蔽失败并可脱离继续执行

## 问题

Graph `join_all` 等待全部 branch 后才传播首个错误，无 timeout 时挂起 sibling 可永久遮蔽失败；DagWorkflow spawned handles 按序等待且 caller cancel 时 JoinHandle drop 不停止节点。

## 触发条件与影响

一个 branch 已失败而另一个挂起，或外层 future 被取消时，Workflow 可不结算或让节点脱离继续产生外部副作用。

## 证据

`echo-orchestration/src/workflow/graph.rs` 与 `workflow/dag.rs` 的并行 join/abort 路径构成源码反例。

## 处理记录

Failure-concurrency Audit 确认；后续 repair 需明确 sibling cancel/drain/partial-effect contract。
