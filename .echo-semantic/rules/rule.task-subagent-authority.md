---
schema_version: 1
id: rule.task-subagent-authority
kind: rule
status: needs_review
expectation: human_confirmed
risk: high
primary_focus: state_authority
focus: [time_lifecycle, failure_concurrency, contract_evidence]
observed_at: 9d1f3f2b5fdc204c08ecdec32ed22e8df95870e9
behavior_refs: [behavior.task-subagent-execution]
code_refs: [echo-orchestration/src/tasks/revisioned.rs, echo-orchestration/src/tasks/runtime_service.rs, echo-orchestration/src/tasks/runtime_executor.rs, echo-orchestration/src/tasks/background_task.rs, echo-orchestration/src/tasks/background_state.rs, src/agent/subagent/registry.rs, src/agent/subagent/executor.rs, docs/adr/0008-canonical-runtime-task-authority.md, docs/adr/0033-subagent-factory-singleflight-publication.md, docs/adr/0039-background-task-terminal-authority.md]
evidence_refs: [evidence.task-subagent-workflow, evidence.subagent-factory-singleflight-repair, evidence.subagent-factory-singleflight-verification, evidence.background-task-terminal-authority-repair, evidence.background-task-terminal-authority-verification]
finding_refs: [finding.task-patch-claim-race, finding.task-subagent-attempt-link, finding.subagent-factory-cancellation, finding.subagent-factory-publication-race, finding.background-task-wait, finding.subagent-definition-catalog]
---

# Task 与 Subagent 单一权威

## 不变量或唯一权威

`TaskRevisionService` 唯一拥有 Task graph CRUD/revision，`RuntimeTaskService` 唯一拥有 dependency execution，`SubagentExecutor` 唯一执行 Subagent attempt。

## 适用行为

适用于单Task、Todo projection、Plan artifact、依赖DAG、Team intent、direct/Fork/Teammate/Team dispatch和恢复；process-local BackgroundTask是相邻能力，必须保持自身唯一handle terminal且不得冒充durable graph。

## 当前实现

Task claim绑定revision/attempt/spec hash/claim ID；Subagent control/event/outcome绑定exact Subagent attempt，但两类identity的生产关联尚未闭合。Registry entry的revision与OnceCell共同约束lazy factory generation，同代只发布一个cached实例。Process-local BackgroundTaskHandleState唯一提交status/result，type-erased list读取同一state；它不替代公开checkpoint BackgroundTaskState或durable graph。Team编译到同一graph。

## 期望行为

不得恢复旧TaskManager/TaskStore/TaskExecutor、plan CRUD、Todo store、Team私有DAG loop或禁用旧术语；也不得把process-local future handle扩成第二Task关系权威。

## 证据

ADR0008/0033/0039、task runtime tests、Subagent registry/executor tests、BackgroundTask并发测试与multi-agent文档提供证据。

## 裁决记录

仓库约束确认标准关系为TaskRun -> PlanTask -> SubagentRun；factory cancellation/publication已修复并复审，claim/link与definition catalog等缺口仍使Rule保持needs_review。
