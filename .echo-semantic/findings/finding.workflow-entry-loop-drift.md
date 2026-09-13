---
schema_version: 1
id: finding.workflow-entry-loop-drift
kind: finding
type: implementation_bug
status: open
severity: medium
primary_focus: contract_evidence
focus: [failure_concurrency, time_lifecycle]
boundary_ref: boundary.task-subagent-workflow
behavior_refs: [behavior.task-subagent-execution]
rule_refs: [rule.task-subagent-authority]
evidence_refs: [evidence.task-subagent-workflow]
audit_refs: []
decision_refs: []
repair_evidence_refs: []
verification_evidence_refs: []
rereview_audit_refs: []
discovered_at: source:8b3972e1d2bc92f4ad59f511b6674eaaf243c1760f21973caf9e96558f71db90
---

# Workflow 多入口主循环已发生事件漂移

## 问题

Graph 的 run、run-until-interrupt、resume 和 stream 分别实现循环；`WorkflowEvent::NodeError` 被公开定义，但生产 stream 对错误直接返回 Err，未发现该事件生产点。

## 触发条件与影响

不同执行入口遇到 node error、interrupt 或 resume 时可能出现不同事件与终态，消费者依赖 NodeError 时无法观察文档合同。

## 证据

`echo-orchestration/src/workflow/graph.rs` 的四个入口和 `workflow/mod.rs` 的事件定义构成源码反例。

## 处理记录

Discovery 记录；下一阶段先补入口行为矩阵，再决定抽取共享 loop 或修正文档/事件。
