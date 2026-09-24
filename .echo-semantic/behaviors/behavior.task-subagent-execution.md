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
observed_at: source:87b717676a7b51e213630677989947777b4bed441acd8c7d655fb6c96dca77ad
code_refs: [echo-orchestration/src/tasks/revisioned.rs, echo-orchestration/src/tasks/runtime_service.rs, echo-orchestration/src/tasks/runtime_executor.rs, src/agent/subagent/registry.rs, src/agent/subagent/executor.rs, echo-orchestration/src/workflow/graph.rs, echo-orchestration/src/workflow/dag.rs, echo-orchestration/src/workflow/concurrent.rs, echo-orchestration/src/workflow/mod.rs, echo-orchestration/src/scheduler/runner.rs, echo-orchestration/src/scheduler/cron_task.rs, echo-orchestration/src/tasks/background_task.rs, echo-orchestration/src/tasks/background_state.rs, echo-orchestration/src/tasks/command_cell.rs, docs/adr/0039-background-task-terminal-authority.md, docs/adr/0040-workflow-checkpoint-lease-and-sibling-settlement.md]
rule_refs: [rule.task-subagent-authority]
evidence_refs: [evidence.task-subagent-workflow, evidence.subagent-factory-singleflight-repair, evidence.subagent-factory-singleflight-verification, evidence.task-subagent-attempt-link-repair, evidence.task-subagent-attempt-link-verification, evidence.task-subagent-external-control-handle-repair, evidence.task-subagent-external-control-handle-verification, evidence.framework-only-finding-closure-verification, evidence.background-task-terminal-authority-repair, evidence.background-task-terminal-authority-verification, evidence.workflow-parallel-failure-settlement-repair, evidence.workflow-parallel-failure-settlement-verification, evidence.workflow-checkpoint-claim-settlement-repair, evidence.workflow-checkpoint-claim-settlement-verification, evidence.scheduler-occurrence-authority-repair, evidence.scheduler-occurrence-authority-verification]
finding_refs: [finding.task-patch-claim-race, finding.task-subagent-attempt-link, finding.subagent-factory-cancellation, finding.subagent-factory-publication-race, finding.workflow-dag-authority, finding.workflow-entry-loop-drift, finding.workflow-checkpoint-claim-recovery, finding.workflow-checkpoint-resurrection-race, finding.workflow-parallel-failure-settlement, finding.scheduler-cache-delivery, finding.scheduler-control-fire-race, finding.scheduler-task-id-uniqueness, finding.background-task-wait, finding.subagent-definition-catalog]
---

# Task、Subagent 与 Workflow 执行

## 重要承诺

一个 revisioned Task graph 拥有 Task 关系和执行状态；Plan 是 artifact，Todo/TaskEvent 是 projection；所有 Subagent 模式经过同一 registry/executor。

## 当前行为

`TaskRevisionService`提交完整revision，`RuntimeTaskService`计算ready frontier、claim、retry/pause/cancel/settle；Subagent attempt使用TaskClaim-derived typed identity、control、events和outcome。Registry lazy factory以registration revision scoped OnceCell统一构造与发布。Process-local TaskSpawner用BackgroundTaskHandleState原子发布status/result，Clone handle共享cancel与notification；admission和execution共享absolute deadline，child execution task由JoinHandle监督。公开BackgroundTaskState checkpoint类型保持独立。Team intent编译到同一graph。

## 期望行为

应用 adapter 不得拥有第二 DAG loop、validator、retry/cancel 或 terminal reducer；stale attempt 不能覆盖新 claim。

## 触发、结果与副作用

Task tools、Team/agent dispatch、programmatic runtime、Workflow 和 Scheduler 可触发执行；Subagent/tool/workflow 产生外部 effect，并通过各自 receipt/claim 结算。

## 失败、重试与恢复

循环依赖、无ready frontier、timeout、cancel、pause、skip、retry exhaustion、superseded claim和restart必须保留typed状态与一致性提交点；Workflow checkpoint使用renewable attempt lease，失败requeue、成功ack且tag只做pending generation CAS。并行Workflow在first failure后取消并drain sibling，成功结果按注册/拓扑顺序投影。

## 证据

Task service/executor tests、Subagent registry/control/event tests、ADR 0008/0024/0027/0030/0033 和 public facade smoke 提供证据。

## 裁决记录

用户与 ADR 0008 已确认 TaskRun -> PlanTask -> SubagentRun 的单一权威和 Subagent 术语。
