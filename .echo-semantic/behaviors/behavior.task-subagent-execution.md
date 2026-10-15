---
schema_version: 1
id: behavior.task-subagent-execution
kind: behavior
status: needs_review
expectation: human_confirmed
risk: high
primary_focus: state_authority
focus:
- time_lifecycle
- failure_concurrency
- result_side_effect
- contract_evidence
boundary: boundary.task-subagent-workflow
observed_at: 77de6ca90ad842eb4b7b2921a4baa954f41cccfe
code_refs:
- echo-orchestration/src/tasks/revisioned.rs
- echo-orchestration/src/tasks/runtime_service.rs
- echo-orchestration/src/tasks/runtime_executor.rs
- src/agent/subagent/registry.rs
- src/agent/subagent/executor.rs
- echo-orchestration/src/workflow/graph.rs
- echo-orchestration/src/workflow/dag.rs
- echo-orchestration/src/workflow/concurrent.rs
- echo-orchestration/src/workflow/mod.rs
- echo-orchestration/src/scheduler/runner.rs
- echo-orchestration/src/scheduler/cron_task.rs
- echo-orchestration/src/tasks/background_task.rs
- echo-orchestration/src/tasks/background_state.rs
- echo-orchestration/src/tasks/command_cell.rs
- docs/adr/0039-background-task-terminal-authority.md
- docs/adr/0040-workflow-checkpoint-lease-and-sibling-settlement.md
- src/agent/subagent/team/mod.rs
- src/agent/subagent/types.rs
- echo-orchestration/src/tasks/runtime.rs
- docs/adr/0060-task-execution-store-cas-and-local-interruption.md
rule_refs:
- rule.task-subagent-authority
evidence_refs:
- evidence.subagent-physical-settlement-repair
- evidence.task-subagent-workflow
- evidence.subagent-factory-singleflight-repair
- evidence.subagent-factory-singleflight-verification
- evidence.task-subagent-attempt-link-repair
- evidence.task-subagent-attempt-link-verification
- evidence.task-subagent-external-control-handle-repair
- evidence.task-subagent-external-control-handle-verification
- evidence.framework-only-finding-closure-verification
- evidence.background-task-terminal-authority-repair
- evidence.background-task-terminal-authority-verification
- evidence.workflow-parallel-failure-settlement-repair
- evidence.workflow-parallel-failure-settlement-verification
- evidence.workflow-checkpoint-claim-settlement-repair
- evidence.workflow-checkpoint-claim-settlement-verification
- evidence.scheduler-occurrence-authority-repair
- evidence.scheduler-occurrence-authority-verification
- evidence.task-runtime-store-control-repair
- evidence.task-runtime-store-control-verification
- evidence.task-dispatch-parent-drop-repair
- evidence.framework-parent-drop-final-verification
finding_refs:
- finding.subagent-physical-settlement-observation
- finding.task-patch-claim-race
- finding.task-subagent-attempt-link
- finding.subagent-factory-cancellation
- finding.subagent-factory-publication-race
- finding.workflow-dag-authority
- finding.workflow-entry-loop-drift
- finding.workflow-checkpoint-claim-recovery
- finding.workflow-checkpoint-resurrection-race
- finding.workflow-parallel-failure-settlement
- finding.scheduler-cache-delivery
- finding.scheduler-control-fire-race
- finding.scheduler-task-id-uniqueness
- finding.background-task-wait
- finding.subagent-definition-catalog
- finding.task-runtime-store-control
- finding.task-dispatch-parent-drop-owner
---



# Task、Subagent 与 Workflow 执行

## 重要承诺

一个 revisioned Task graph 拥有 Task 关系和执行状态；Plan 是 artifact，Todo/TaskEvent 是 projection；所有 Subagent 模式经过同一 registry/executor。

## 当前行为

本切片中 CRUD 与 claim/retry/resume/settlement 使用同一个已注册 `RevisionedTaskStore`。无副作用 `supports_runtime_execution` 默认为 false；运行时在 load/mutation 前拒绝不支持 backend，InMemory 与显式 opt-in backend 使用 execution CAS（完整 execution map 与 relation revision 比较，关系版本不变，冲突最多重算 32 次）。`CompletedWithResult` 与输出同事务提交，显式 null 与缺失不同，Team 不再维护平行结果缓存。

`TaskRevisionService`提交完整revision，`RuntimeTaskService`计算ready frontier、claim、retry/pause/cancel/settle；Subagent attempt使用TaskClaim-derived typed identity、control、events和outcome。Registry lazy factory以registration revision scoped OnceCell统一构造与发布。Process-local TaskSpawner用BackgroundTaskHandleState原子发布status/result，Clone handle共享cancel与notification；admission和execution共享absolute deadline，child execution task由JoinHandle监督。公开BackgroundTaskState checkpoint类型保持独立。Team intent编译到同一graph。

## 期望行为

应用 adapter 不得拥有第二 DAG loop、validator、retry/cancel 或 terminal reducer；stale attempt 不能覆盖新 claim。

## 触发、结果与副作用

Task tools、Team/agent dispatch、programmatic runtime、Workflow 和 Scheduler 可触发执行；Subagent/tool/workflow 产生外部 effect，并通过各自 receipt/claim 结算。

## 失败、重试与恢复

精确 claim 的中断请求是持久执行事实。Cancelled 强于 Paused；局部停止仅阻断该节点与后代，独立 wave 继续。每次 join 后，review/CAS/cleanup 由 wave 内有界 retained owner 并行结算，与 dispatch join/root grace 同 supervisor 推进；父执行撤销不丢弃未知 CAS。终态 watcher 仅通过单一私有发布 helper 写入：真实 Settled 不可回退或换 status，Unknown 仅在合法结算/恢复后变 Settled，owner 异常退出明确 Unknown，cleanup/reconcile 不得覆盖已提交结果。下一 wave 仍等待全部结算 safe point。

循环依赖、无ready frontier、timeout、cancel、pause、skip、retry exhaustion、superseded claim和restart必须保留typed状态与一致性提交点；Workflow checkpoint使用renewable attempt lease，失败requeue、成功ack且tag只做pending generation CAS。并行Workflow在first failure后取消并drain sibling，成功结果按注册/拓扑顺序投影。

## 证据

2026-10-12 本地未提交 Task 修复快照通过完整 6+17 门禁及对应 red/green；引用新增修复/验证 Evidence。既有 Workflow/Scheduler/definition catalog 等独立问题不由这些测试关闭，Behavior 保持 needs_review。

Task service/executor tests、Subagent registry/control/event tests、ADR 0008/0024/0027/0030/0033 和 public facade smoke 提供证据。

## 裁决记录

用户与 ADR 0008 已确认 TaskRun -> PlanTask -> SubagentRun 的单一权威和 Subagent 术语。

父execute退出增量：原driver持有claim、reservation、dispatch join与结算；callerDrop仅取消child。已返回claim的异常退出实际join后同backend CAS，unknown不假终态；修复与focused证据支持具体窗口，新23/main/消费者仍另验。


## B 取消观察增量（验证中）

用户已选择保留原 physical owner，仅首次 admitted root cancel/whole pause 的观察受 grace 限制。ExecutionNotSettled 不是 AgentFailure/Task 终态，不授权释放原资源或重执行；exact A 与 sibling B 分离。当前新 Finding 保持 open，强回归、资源收尾增量和完整交付门禁尚未全部结束；历史已审范围不因本段自动扩张。
