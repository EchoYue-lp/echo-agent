---
schema_version: 1
id: behavior.task-subagent-execution
kind: behavior
status: needs_review
expectation: human_confirmed
risk: high
primary_focus: state_authority
focus: [time_lifecycle, failure_concurrency, result_side_effect, contract_evidence]
boundary: boundary.task-subagent-workflow
observed_at: f1e9027246760661144786e9e35615cd46d580c6
code_refs: [echo-orchestration/src/tasks/revisioned.rs, echo-orchestration/src/tasks/runtime_service.rs, echo-orchestration/src/tasks/runtime_executor.rs, src/agent/subagent/registry.rs, src/agent/subagent/executor.rs, echo-orchestration/src/workflow/graph.rs, echo-orchestration/src/workflow/dag.rs, echo-orchestration/src/workflow/mod.rs, echo-orchestration/src/scheduler/runner.rs, echo-orchestration/src/scheduler/cron_task.rs, echo-orchestration/src/tasks/background_task.rs, echo-orchestration/src/tasks/command_cell.rs]
rule_refs: [rule.task-subagent-authority]
evidence_refs: [evidence.task-subagent-workflow]
finding_refs: [finding.task-patch-claim-race, finding.task-subagent-attempt-link, finding.subagent-factory-cancellation, finding.workflow-dag-authority, finding.workflow-entry-loop-drift, finding.workflow-checkpoint-claim-recovery, finding.scheduler-cache-delivery, finding.background-task-wait, finding.subagent-definition-catalog]
---

# Task、Subagent 与 Workflow 执行

## 重要承诺

一个 revisioned Task graph 拥有 Task 关系和执行状态；Plan 是 artifact，Todo/TaskEvent 是 projection；所有 Subagent 模式经过同一 registry/executor。

## 当前行为

`TaskRevisionService` 提交完整 revision，`RuntimeTaskService` 计算 ready frontier、claim、retry/pause/cancel/settle；Subagent attempt 使用 typed identity、control、events 和 outcome，但 TaskClaim 关联与 lazy factory cancellation 仍有 Finding；Team intent 编译到同一 graph。

## 期望行为

应用 adapter 不得拥有第二 DAG loop、validator、retry/cancel 或 terminal reducer；stale attempt 不能覆盖新 claim。

## 触发、结果与副作用

Task tools、Team/agent dispatch、programmatic runtime、Workflow 和 Scheduler 可触发执行；Subagent/tool/workflow 产生外部 effect，并通过各自 receipt/claim 结算。

## 失败、重试与恢复

循环依赖、无 ready frontier、timeout、cancel、pause、skip、retry exhaustion、superseded claim 和 restart 必须保留 typed 状态与 safe point。

## 证据

Task service/executor tests、Subagent control/event tests、ADR 0008/0024/0027/0030 和 public facade smoke 提供证据。

## 裁决记录

用户与 ADR 0008 已确认 TaskRun -> PlanTask -> SubagentRun 的单一权威和 Subagent 术语。
