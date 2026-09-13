---
schema_version: 1
id: rule.task-subagent-authority
kind: rule
status: needs_review
expectation: human_confirmed
risk: high
primary_focus: state_authority
focus: [time_lifecycle, failure_concurrency, contract_evidence]
observed_at: source:252362472c35fc62836123fbf064477b407af7bce21a21f16d231d594eebb136
behavior_refs: [behavior.task-subagent-execution]
code_refs: [echo-orchestration/src/tasks/revisioned.rs, echo-orchestration/src/tasks/runtime_service.rs, echo-orchestration/src/tasks/runtime_executor.rs, src/agent/subagent/registry.rs, src/agent/subagent/executor.rs, docs/adr/0008-canonical-runtime-task-authority.md, docs/adr/0033-subagent-factory-singleflight-publication.md]
evidence_refs: [evidence.task-subagent-workflow, evidence.subagent-factory-singleflight-repair, evidence.subagent-factory-singleflight-verification]
finding_refs: [finding.task-patch-claim-race, finding.task-subagent-attempt-link, finding.subagent-factory-cancellation, finding.subagent-factory-publication-race, finding.subagent-definition-catalog]
---

# Task 与 Subagent 单一权威

## 不变量或唯一权威

`TaskRevisionService` 唯一拥有 Task graph CRUD/revision，`RuntimeTaskService` 唯一拥有 dependency execution，`SubagentExecutor` 唯一执行 Subagent attempt。

## 适用行为

适用于单 Task、Todo projection、Plan artifact、依赖 DAG、Team intent、direct/Fork/Teammate/Team dispatch 和恢复。

## 当前实现

Task claim 绑定 revision/attempt/spec hash/claim ID；Subagent control/event/outcome 绑定 exact Subagent attempt，但两类 identity 的生产关联尚未闭合。Registry entry 的 revision 与 OnceCell 共同约束 lazy factory generation，同代只发布一个cached实例；Team 编译到同一 graph。

## 期望行为

不得恢复旧 TaskManager/TaskStore/TaskExecutor、plan CRUD、Todo store、Team 私有 DAG loop 或 Worker 术语。

## 证据

ADR 0008/0033、task runtime tests、Subagent registry/executor tests 与 multi-agent 文档提供证据。

## 裁决记录

仓库约束确认标准关系为TaskRun -> PlanTask -> SubagentRun；factory cancellation/publication已修复并复审，claim/link与definition catalog等缺口仍使Rule保持needs_review。
