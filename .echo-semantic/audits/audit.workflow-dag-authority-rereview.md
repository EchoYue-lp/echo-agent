---
schema_version: 1
id: audit.workflow-dag-authority-rereview
kind: audit
boundary_ref: boundary.task-subagent-workflow
lens: contract_evidence
freshness: examined
revision: d75bd27b1cee425fad4fdfa20c99f528d925ff8d
finding_refs: [finding.workflow-dag-authority]
challenges:
  keep-separate-authorities:
    revision: d75bd27b1cee425fad4fdfa20c99f528d925ff8d
    source_refs: [docs/adr/0059-task-workflow-dag-authority.md, echo-orchestration/src/tasks/revisioned.rs, echo-orchestration/src/workflow/graph.rs, echo-orchestration/src/workflow/dag.rs]
    evidence_refs: [evidence.workflow-dag-authority-repair, evidence.workflow-dag-authority-verification]
---

# Task 与 Workflow 图权威独立复审

## 审查范围

独立 reviewer 检查 ADR 0059、三类 graph 的真实入口、双语文档和结构化文档合同。

## 已检查故障假设

检查 Task claim/retry 语义是否被 Workflow 吸收、Workflow public API 是否按采用量误删、DagWorkflow 是否被误写为 durable Task graph，以及 adapter 是否重建第二调度器。

## 实际实现路径与证据

ADR 分别绑定 revisioned Task graph、Graph continuation 与 DagWorkflow 静态 pipeline 的 owner、状态和恢复边界，并限制组合 adapter 只传递执行结果。文档合同读取真实 public 入口并验证双语导航。

## 问题记录

复审未发现阻断项，结论 pass。

## 残余风险

本决策不修复其它 Workflow validation、checkpoint 或 Task recovery Finding。

## 未检查项

纯文档决策未改变运行时，未重复执行完整 Task/Workflow 行为测试。
