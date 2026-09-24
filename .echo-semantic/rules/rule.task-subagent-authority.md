---
schema_version: 1
id: rule.task-subagent-authority
kind: rule
status: needs_review
expectation: human_confirmed
risk: high
primary_focus: state_authority
focus: [time_lifecycle, failure_concurrency, contract_evidence]
observed_at: source:757f499d4d9a40a4c27791933cb3d5e9d3b2dda76a1a28e4561e317d4719be94
behavior_refs: [behavior.task-subagent-execution]
code_refs: [echo-orchestration/src/tasks/revisioned.rs, echo-orchestration/src/tasks/runtime_service.rs, echo-orchestration/src/tasks/runtime_executor.rs, echo-orchestration/src/tasks/background_task.rs, echo-orchestration/src/tasks/background_state.rs, src/agent/subagent/registry.rs, src/agent/subagent/executor.rs, docs/adr/0008-canonical-runtime-task-authority.md, docs/adr/0033-subagent-factory-singleflight-publication.md, docs/adr/0039-background-task-terminal-authority.md]
evidence_refs: [evidence.task-subagent-workflow, evidence.subagent-factory-singleflight-repair, evidence.subagent-factory-singleflight-verification, evidence.task-subagent-attempt-link-repair, evidence.task-subagent-attempt-link-verification, evidence.task-subagent-external-control-handle-repair, evidence.task-subagent-external-control-handle-verification, evidence.framework-only-finding-closure-verification, evidence.background-task-terminal-authority-repair, evidence.background-task-terminal-authority-verification, evidence.workflow-checkpoint-claim-settlement-repair, evidence.workflow-checkpoint-claim-settlement-verification]
finding_refs: [finding.task-patch-claim-race, finding.task-subagent-attempt-link, finding.subagent-factory-cancellation, finding.subagent-factory-publication-race, finding.workflow-checkpoint-claim-recovery, finding.workflow-checkpoint-resurrection-race, finding.background-task-wait, finding.subagent-definition-catalog]
---

# Task 与 Subagent 单一权威

## 不变量或唯一权威

`TaskRevisionService` 唯一拥有 Task graph CRUD/revision，`RuntimeTaskService` 唯一拥有 dependency execution，`SubagentExecutor` 唯一执行 Subagent attempt。

## 适用行为

适用于单Task、Todo projection、Plan artifact、依赖DAG、Team intent、direct/Fork/Teammate/Team dispatch和恢复；process-local BackgroundTask是相邻能力，必须保持自身唯一handle terminal且不得冒充durable graph。

## 当前实现

Task claim绑定revision/attempt/spec hash/claim ID；Subagent control/event/outcome使用由exact TaskClaim
派生的同一 physical attempt identity。Process-local live registry 只投影 pending/reserved/active/settled，
不提交 durable terminal。Registry entry的revision与OnceCell共同约束lazy factory generation，同代只发布一个cached实例。Process-local BackgroundTaskHandleState唯一提交status/result，type-erased list读取同一state；它不替代公开checkpoint BackgroundTaskState或durable graph。Team编译到同一graph。

## 期望行为

不得恢复旧TaskManager/TaskStore/TaskExecutor、plan CRUD、Todo store、Team私有DAG loop或禁用旧术语；也不得把process-local future handle扩成第二Task关系权威。

## 证据

ADR0008/0033/0039、task runtime tests、Subagent registry/executor tests、BackgroundTask并发测试与multi-agent文档提供证据。

## 裁决记录

仓库约束确认标准关系为 TaskRun -> PlanTask -> SubagentRun；factory cancellation/publication 与
TaskClaim-derived attempt link 已修复并复审。Definition catalog 与其它独立 Finding 仍使 Rule
保持 needs_review。
