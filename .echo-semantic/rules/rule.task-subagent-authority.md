---
schema_version: 1
id: rule.task-subagent-authority
kind: rule
status: needs_review
expectation: human_confirmed
risk: high
primary_focus: state_authority
focus: [time_lifecycle, failure_concurrency, contract_evidence]
observed_at: source:64131952ceb6f498fe94fc34482afe3ecf1e1e77f5a1d31a6ff3ce81b7e0eb01
behavior_refs: [behavior.task-subagent-execution]
code_refs: [echo-orchestration/src/tasks/revisioned.rs, echo-orchestration/src/tasks/runtime_service.rs, echo-orchestration/src/tasks/runtime_executor.rs, src/agent/subagent/executor.rs, docs/adr/0008-canonical-runtime-task-authority.md]
evidence_refs: [evidence.task-subagent-workflow]
finding_refs: [finding.task-patch-claim-race, finding.task-subagent-attempt-link, finding.subagent-factory-cancellation, finding.subagent-definition-catalog]
---

# Task 与 Subagent 单一权威

## 不变量或唯一权威

`TaskRevisionService` 唯一拥有 Task graph CRUD/revision，`RuntimeTaskService` 唯一拥有 dependency execution，`SubagentExecutor` 唯一执行 Subagent attempt。

## 适用行为

适用于单 Task、Todo projection、Plan artifact、依赖 DAG、Team intent、direct/Fork/Teammate/Team dispatch 和恢复。

## 当前实现

Task claim 绑定 revision/attempt/spec hash/claim ID；Subagent control/event/outcome 绑定 exact Subagent attempt，但两类 identity 的生产关联尚未闭合；Team 编译到同一 graph。

## 期望行为

不得恢复旧 TaskManager/TaskStore/TaskExecutor、plan CRUD、Todo store、Team 私有 DAG loop 或 Worker 术语。

## 证据

ADR 0008、task runtime tests、Subagent tests 与 multi-agent 文档提供证据。

## 裁决记录

仓库约束确认标准关系为 TaskRun -> PlanTask -> SubagentRun；当前 claim/link/factory 缺口进入 Finding，故 Rule 保持 needs_review。
