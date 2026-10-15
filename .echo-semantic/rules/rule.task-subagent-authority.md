---
schema_version: 1
id: rule.task-subagent-authority
kind: rule
status: needs_review
expectation: human_confirmed
risk: high
primary_focus: state_authority
focus:
- time_lifecycle
- failure_concurrency
- contract_evidence
observed_at: 77de6ca90ad842eb4b7b2921a4baa954f41cccfe
behavior_refs:
- behavior.task-subagent-execution
code_refs:
- echo-orchestration/src/tasks/revisioned.rs
- echo-orchestration/src/tasks/runtime_service.rs
- echo-orchestration/src/tasks/runtime_executor.rs
- echo-orchestration/src/tasks/background_task.rs
- echo-orchestration/src/tasks/background_state.rs
- src/agent/subagent/registry.rs
- src/agent/subagent/executor.rs
- docs/adr/0008-canonical-runtime-task-authority.md
- docs/adr/0033-subagent-factory-singleflight-publication.md
- docs/adr/0039-background-task-terminal-authority.md
- echo-orchestration/src/tasks/runtime.rs
- src/agent/subagent/team/mod.rs
- docs/adr/0060-task-execution-store-cas-and-local-interruption.md
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
- evidence.workflow-checkpoint-claim-settlement-repair
- evidence.workflow-checkpoint-claim-settlement-verification
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
- finding.workflow-checkpoint-claim-recovery
- finding.workflow-checkpoint-resurrection-race
- finding.background-task-wait
- finding.subagent-definition-catalog
- finding.task-runtime-store-control
- finding.task-dispatch-parent-drop-owner
---



# Task 与 Subagent 单一权威

## 不变量或唯一权威

runtime execution CAS、输出与精确中断请求均由注册的同一个 graph backend 原子提交。Adapter 只转换/组合，不重新拥有调度、取消、恢复或终态。Watcher 是投影：已提交真实 receipt 不可被后续 cleanup、owner 异常或公开 reconcile 改为 Unknown/Superseded。

`TaskRevisionService` 唯一拥有 Task graph CRUD/revision，`RuntimeTaskService` 唯一拥有 dependency execution，`SubagentExecutor` 唯一执行 Subagent attempt。

## 适用行为

适用于单Task、Todo projection、Plan artifact、依赖DAG、Team intent、direct/Fork/Teammate/Team dispatch和恢复；process-local BackgroundTask是相邻能力，必须保持自身唯一handle terminal且不得冒充durable graph。

## 当前实现

TaskRevisionService 的无副作用执行支持声明与同 backend execution CAS 是唯一入口；TaskExecution 保存 result/interruption_requested，TaskResolution/Settlement receipt 携带实际提交 disposition/status。单任务取消不取消独立节点，root cancel 仍结算全部 unfinished；root pause 不复活已接受的精确取消。

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

父execute退出增量：原driver持有claim、reservation、dispatch join与结算；callerDrop仅取消child。已返回claim的异常退出实际join后同backend CAS，unknown不假终态；修复与focused证据支持具体窗口，新23/main/消费者仍另验。


## B 取消观察增量（验证中）

用户已选择保留原 physical owner，仅首次 admitted root cancel/whole pause 的观察受 grace 限制。ExecutionNotSettled 不是 AgentFailure/Task 终态，不授权释放原资源或重执行；exact A 与 sibling B 分离。当前新 Finding 保持 open，强回归、资源收尾增量和完整交付门禁尚未全部结束；历史已审范围不因本段自动扩张。
