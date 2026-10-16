---
schema_version: 1
id: map.task-subagent-workflow
kind: capability_map
title: Task、Subagent、Workflow 与 Scheduler
risk: high
observed_at: 77de6ca90ad842eb4b7b2921a4baa954f41cccfe
boundary_refs:
- boundary.task-subagent-workflow
behavior_refs:
- behavior.task-subagent-execution
rule_refs:
- rule.task-subagent-authority
evidence_refs:
- evidence.subagent-physical-settlement-repair
- evidence.task-subagent-workflow
- evidence.high-risk-audit-frontier
- evidence.task-patch-claim-cas-repair
- evidence.task-patch-claim-cas-verification
- evidence.subagent-factory-singleflight-repair
- evidence.subagent-factory-singleflight-verification
- evidence.task-subagent-attempt-link-repair
- evidence.task-subagent-attempt-link-verification
- evidence.task-subagent-external-control-handle-repair
- evidence.task-subagent-external-control-handle-verification
- evidence.framework-only-finding-closure-verification
- evidence.background-task-terminal-authority-repair
- evidence.background-task-terminal-authority-verification
- evidence.command-cell-cancel-artifact-settlement-repair
- evidence.command-cell-cancel-artifact-settlement-verification
- evidence.workflow-parallel-failure-settlement-repair
- evidence.workflow-parallel-failure-settlement-verification
- evidence.workflow-checkpoint-claim-settlement-repair
- evidence.workflow-checkpoint-claim-settlement-verification
- evidence.workflow-entry-loop-authority-repair
- evidence.workflow-entry-loop-authority-verification
- evidence.scheduler-occurrence-authority-repair
- evidence.scheduler-occurrence-authority-verification
- evidence.framework-concept-navigation
- evidence.task-runtime-store-control-repair
- evidence.task-runtime-store-control-verification
- evidence.task-dispatch-parent-drop-repair
- evidence.framework-parent-drop-final-verification
- evidence.framework-final-34d5-full-verification
- evidence.framework-final-34d5-gate-archive
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
- finding.scheduler-task-id-uniqueness
- finding.scheduler-control-fire-race
- finding.background-task-wait
- finding.command-cell-retention-lease-prune-race
- finding.command-cell-cancel-artifact-settlement
- finding.subagent-definition-catalog
- finding.task-runtime-store-control
- finding.task-dispatch-parent-drop-owner
audit_refs:
- audit.task-subagent-workflow.state-authority
- audit.task-subagent-workflow.failure-concurrency
- audit.task-subagent-workflow.data-durability
- audit.task-subagent-workflow.time-lifecycle
- audit.task-patch-claim-cas-rereview
- audit.subagent-factory-singleflight-rereview
- audit.background-task-terminal-authority-rereview
- audit.workflow-parallel-failure-settlement-rereview
- audit.workflow-checkpoint-claim-settlement-rereview
- audit.workflow-entry-loop-authority-rereview
- audit.scheduler-occurrence-authority-rereview
- audit.framework-final-34d5-gate-rereview
related_map_refs:
- map.agent-session-turn
- map.observation-persistence-delivery
- map.tool-permission-sandbox
scenarios:
  revisioned-task-graph:
    status: mapped
    source_refs:
    - echo-orchestration/src/tasks/revisioned.rs
    - echo-orchestration/src/tasks/runtime.rs
    behavior_refs:
    - behavior.task-subagent-execution
    rule_refs:
    - rule.task-subagent-authority
  runtime-claim-settlement:
    status: mapped
    source_refs:
    - echo-orchestration/src/tasks/runtime_service.rs
    - echo-orchestration/src/tasks/runtime_executor.rs
    finding_refs:
    - finding.task-patch-claim-race
    evidence_refs:
    - evidence.task-subagent-workflow
    - evidence.task-patch-claim-cas-repair
    - evidence.task-patch-claim-cas-verification
  subagent-attempt-control:
    status: needs_review
    source_refs:
    - src/agent/subagent/registry.rs
    - src/agent/subagent/executor.rs
    - src/agent/subagent/control.rs
    - src/agent/subagent/events.rs
    finding_refs:
    - finding.task-subagent-attempt-link
    - finding.subagent-factory-cancellation
    - finding.subagent-factory-publication-race
    - finding.subagent-definition-catalog
    rule_refs:
    - rule.task-subagent-authority
    evidence_refs:
    - evidence.subagent-factory-singleflight-repair
    - evidence.subagent-factory-singleflight-verification
    - evidence.task-subagent-attempt-link-repair
    - evidence.task-subagent-attempt-link-verification
    - evidence.task-subagent-external-control-handle-repair
    - evidence.task-subagent-external-control-handle-verification
    - evidence.framework-only-finding-closure-verification
    audit_refs:
    - audit.subagent-factory-singleflight-rereview
    - audit.task-subagent-attempt-link-rereview
    - audit.task-subagent-external-control-handle-rereview
    unknown: TaskClaim到SubagentAttempt identity、factory cancellation/publication已闭合；definition catalog的产品预期仍未裁决
    next_step: 仅处理definition catalog裁决，不重建attempt identity或第二套live control registry
  workflow-graph-and-events:
    status: needs_review
    source_refs:
    - echo-orchestration/src/workflow/graph.rs
    - echo-orchestration/src/workflow/node.rs
    - echo-orchestration/src/workflow/checkpoint_store.rs
    - echo-orchestration/src/workflow/dag.rs
    finding_refs:
    - finding.subagent-physical-settlement-observation
    - finding.workflow-dag-authority
    - finding.workflow-entry-loop-drift
    - finding.workflow-checkpoint-claim-recovery
    - finding.workflow-checkpoint-resurrection-race
    - finding.workflow-parallel-failure-settlement
    evidence_refs:
    - evidence.subagent-physical-settlement-repair
    - evidence.task-subagent-workflow
    - evidence.workflow-parallel-failure-settlement-repair
    - evidence.workflow-parallel-failure-settlement-verification
    - evidence.workflow-checkpoint-claim-settlement-repair
    - evidence.workflow-checkpoint-claim-settlement-verification
    - evidence.workflow-entry-loop-authority-repair
    - evidence.workflow-entry-loop-authority-verification
    audit_refs:
    - audit.subagent-physical-settlement-focused-review
    - audit.workflow-parallel-failure-settlement-rereview
    - audit.workflow-checkpoint-claim-settlement-rereview
    - audit.workflow-entry-loop-authority-rereview
    unknown: checkpoint claim/lease、并行sibling settlement及四执行入口事件对等已关闭；Task/Graph/DagWorkflow长期边界已由ADR 0059裁决，仍待独立复审、严格语义门禁与远端交付
    next_step: B/A原框架663代码6+17及新指南runtime合同已实际通过；继续SDK/CLI最终main消费和精确main CI，不把本地资格冒远端交付
  cron-scheduler:
    status: mapped
    source_refs:
    - echo-orchestration/src/scheduler/runner.rs
    - echo-orchestration/src/scheduler/cron_task.rs
    finding_refs:
    - finding.scheduler-cache-delivery
    - finding.scheduler-task-id-uniqueness
    - finding.scheduler-control-fire-race
    evidence_refs:
    - evidence.subagent-physical-settlement-repair
    - evidence.task-subagent-workflow
    - evidence.scheduler-occurrence-authority-repair
    - evidence.scheduler-occurrence-authority-verification
    audit_refs:
    - audit.subagent-physical-settlement-focused-review
    - audit.scheduler-occurrence-authority-rereview
    behavior_refs:
    - behavior.task-subagent-execution
    rule_refs:
    - rule.task-subagent-authority
  process-local-background-task:
    status: mapped
    source_refs:
    - echo-orchestration/src/tasks/background_task.rs
    - docs/en/29-long-running-tasks.md
    - docs/zh/29-long-running-tasks.md
    - docs/adr/0039-background-task-terminal-authority.md
    finding_refs:
    - finding.background-task-wait
    behavior_refs:
    - behavior.task-subagent-execution
    rule_refs:
    - rule.task-subagent-authority
    evidence_refs:
    - evidence.background-task-terminal-authority-repair
    - evidence.background-task-terminal-authority-verification
    audit_refs:
    - audit.background-task-terminal-authority-rereview
  command-cell-runtime:
    status: mapped
    source_refs:
    - echo-core/src/tools/cell.rs
    - echo-orchestration/src/tasks/command_cell.rs
    - docs/adr/0025-deterministic-command-cell-watcher.md
    evidence_refs:
    - evidence.task-subagent-workflow
    - evidence.effects-extensions
    - evidence.command-cell-cancel-artifact-settlement-repair
    - evidence.command-cell-cancel-artifact-settlement-verification
    behavior_refs:
    - behavior.task-subagent-execution
    - behavior.effect-permission-execution
    finding_refs:
    - finding.command-cell-retention-lease-prune-race
    - finding.command-cell-cancel-artifact-settlement
    audit_refs:
    - audit.command-cell-settlement-rereview
  registered-backend-runtime-result-and-exact-control:
    status: mapped
    source_refs:
    - echo-orchestration/src/tasks/revisioned.rs
    - echo-orchestration/src/tasks/runtime_service.rs
    - echo-orchestration/src/tasks/runtime_executor.rs
    - src/agent/subagent/team/mod.rs
    evidence_refs:
    - evidence.task-runtime-store-control-repair
    - evidence.task-runtime-store-control-verification
    finding_refs:
    - finding.task-runtime-store-control
---





# Task、Subagent、Workflow 与 Scheduler

## 能力范围

覆盖 revisioned Task graph、runtime execution、Subagent attempt、Team、Workflow、Scheduler、BackgroundTask 和 CommandCell。

## 入口与输出

Task tools、programmatic runtime、Agent dispatch、Workflow Rust/JSON/YAML、cron tick 与 command tools 触发；输出 claim/receipt/event/checkpoint/effect。

## 行为关系

Task graph 和 Subagent 是主任务执行关系；Workflow/Scheduler/Background 是相邻通用能力，不凭采用量判死或自动归并。

## 状态与数据流

Task spec/execution/claim、Subagent identity/control/envelope、Workflow state/checkpoint、Scheduler store/cache各有明确owner。Process-local BackgroundTaskHandleState原子保存status与单消费者result；公开BackgroundTaskState checkpoint类型仍是独立历史合同。

## 策略来源与优先级

Task policy/controller、Subagent prompt/isolation/admission、Workflow graph definition、cron store 与 CommandCell config 决定行为。

## 生命周期与失败路径

Revision/claim/wave/settle/retry/pause/cancel、dispatch/message/interrupt/terminal、checkpoint/resume、tick/shutdown、spawn/admit/execute/cancel/deadline/wait/terminal。

## 权限与敏感信息

Subagent 复用 Agent tool policy；Workflow node/cron callback 本身不替代外层 permission；CommandCell 可要求 sandbox。

## 用户侧投影

TaskEvent/progress、Subagent envelopes、Workflow events 和 command snapshots 是消费者视图，不取代各自 authority。

## 场景处置清单

所有主要入口已映射；Task relation patch、Subagent factory竞态与BackgroundTask terminal authority已关闭。Scheduler 的单进程定义写入由 live runner 许可约束，cache 从同一 Store 快照派生；跨进程 Store writer 和离线 misfire 不在该合同内。其它当前缺口继续进入Finding/needs_review；Workflow、Scheduler、BackgroundTask与CommandCell不合成一个authority。

## 未展开项

具体 Finding 修复由后续独立 delivery outcome 处理。

父execute退出增量：原driver持有claim、reservation、dispatch join与结算；callerDrop仅取消child。已返回claim的异常退出实际join后同backend CAS，unknown不假终态；修复与focused证据支持具体窗口，新23/main/消费者仍另验。


当前工程补充：同source34d5的新23门禁真实全通过，3471 passed/0 failed/3外部optin ignored、86targets/59examples；见 evidence.framework-final-34d5-full-verification 与原始 evidence.framework-final-34d5-gate-archive。原历史门禁和局部Audit不改写，mapped不表示所有lens/消费者/main已验收。


当前候选最终复审：audit.framework-final-34d5-gate-rereview 绑定新23真实工程收据与完整已审源码，implementation review PASS/action0；不替代独立SDK/main验收，不升级原broad状态。


## B 取消观察增量（验证中）

用户已选择保留原 physical owner，仅首次 admitted root cancel/whole pause 的观察受 grace 限制。ExecutionNotSettled 不是 AgentFailure/Task 终态，不授权释放原资源或重执行；exact A 与 sibling B 分离。当前新 Finding 保持 open，强回归、资源收尾增量和完整交付门禁尚未全部结束；历史已审范围不因本段自动扩张。
