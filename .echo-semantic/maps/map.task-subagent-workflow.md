---
schema_version: 1
id: map.task-subagent-workflow
kind: capability_map
title: Task、Subagent、Workflow 与 Scheduler
risk: high
observed_at: source:252362472c35fc62836123fbf064477b407af7bce21a21f16d231d594eebb136
boundary_refs: [boundary.task-subagent-workflow]
behavior_refs: [behavior.task-subagent-execution]
rule_refs: [rule.task-subagent-authority]
evidence_refs: [evidence.task-subagent-workflow, evidence.high-risk-audit-frontier, evidence.task-patch-claim-cas-repair, evidence.task-patch-claim-cas-verification, evidence.subagent-factory-singleflight-repair, evidence.subagent-factory-singleflight-verification]
finding_refs: [finding.task-patch-claim-race, finding.task-subagent-attempt-link, finding.subagent-factory-cancellation, finding.subagent-factory-publication-race, finding.workflow-dag-authority, finding.workflow-entry-loop-drift, finding.workflow-checkpoint-claim-recovery, finding.workflow-checkpoint-resurrection-race, finding.workflow-parallel-failure-settlement, finding.scheduler-cache-delivery, finding.scheduler-task-id-uniqueness, finding.scheduler-control-fire-race, finding.background-task-wait, finding.command-cell-retention-lease-prune-race, finding.command-cell-cancel-artifact-settlement, finding.subagent-definition-catalog]
audit_refs: [audit.task-subagent-workflow.state-authority, audit.task-subagent-workflow.failure-concurrency, audit.task-subagent-workflow.data-durability, audit.task-subagent-workflow.time-lifecycle, audit.task-patch-claim-cas-rereview, audit.subagent-factory-singleflight-rereview]
related_map_refs: [map.agent-session-turn, map.observation-persistence-delivery, map.tool-permission-sandbox]
scenarios:
  revisioned-task-graph:
    status: mapped
    source_refs: [echo-orchestration/src/tasks/revisioned.rs, echo-orchestration/src/tasks/runtime.rs]
    behavior_refs: [behavior.task-subagent-execution]
    rule_refs: [rule.task-subagent-authority]
  runtime-claim-settlement:
    status: mapped
    source_refs: [echo-orchestration/src/tasks/runtime_service.rs, echo-orchestration/src/tasks/runtime_executor.rs]
    finding_refs: [finding.task-patch-claim-race]
    evidence_refs: [evidence.task-subagent-workflow, evidence.task-patch-claim-cas-repair, evidence.task-patch-claim-cas-verification]
  subagent-attempt-control:
    status: needs_review
    source_refs: [src/agent/subagent/registry.rs, src/agent/subagent/executor.rs, src/agent/subagent/control.rs, src/agent/subagent/events.rs]
    finding_refs: [finding.task-subagent-attempt-link, finding.subagent-factory-cancellation, finding.subagent-factory-publication-race, finding.subagent-definition-catalog]
    rule_refs: [rule.task-subagent-authority]
    evidence_refs: [evidence.subagent-factory-singleflight-repair, evidence.subagent-factory-singleflight-verification]
    audit_refs: [audit.subagent-factory-singleflight-rereview]
    unknown: TaskClaim与SubagentAttempt identity及definition catalog仍未闭合；factory cancellation/publication已关闭
    next_step: 分别处理Task/Subagent attempt route与definition catalog裁决
  workflow-graph-and-events:
    status: needs_review
    source_refs: [echo-orchestration/src/workflow/graph.rs, echo-orchestration/src/workflow/checkpoint_store.rs, echo-orchestration/src/workflow/dag.rs]
    finding_refs: [finding.workflow-dag-authority, finding.workflow-entry-loop-drift, finding.workflow-checkpoint-claim-recovery, finding.workflow-checkpoint-resurrection-race, finding.workflow-parallel-failure-settlement]
    evidence_refs: [evidence.task-subagent-workflow]
    unknown: Graph/DagWorkflow authority、四执行入口事件对等与 checkpoint claim recovery 尚未闭合
    next_step: 分别执行 consolidation、entry parity 与 crash-cut audit
  cron-scheduler:
    status: needs_review
    source_refs: [echo-orchestration/src/scheduler/runner.rs, echo-orchestration/src/scheduler/cron_task.rs]
    finding_refs: [finding.scheduler-cache-delivery, finding.scheduler-task-id-uniqueness, finding.scheduler-control-fire-race]
    evidence_refs: [evidence.task-subagent-workflow]
    unknown: store/cache 可见状态、migration overwrite 与 callback delivery guarantee 未闭合
    next_step: audit cache refresh、持久 claim/ledger 与 migration target collision
  process-local-background-task:
    status: needs_review
    source_refs: [echo-orchestration/src/tasks/background_task.rs]
    finding_refs: [finding.background-task-wait]
    unknown: wait lost-wakeup、多观察者结果与 panic settlement 未闭合
    next_step: 用确定性调度测试审计 process-local terminal authority
  command-cell-runtime:
    status: needs_review
    source_refs: [echo-core/src/tools/cell.rs, echo-orchestration/src/tasks/command_cell.rs, docs/adr/0025-deterministic-command-cell-watcher.md]
    evidence_refs: [evidence.task-subagent-workflow, evidence.effects-extensions]
    behavior_refs: [behavior.task-subagent-execution, behavior.effect-permission-execution]
    finding_refs: [finding.command-cell-retention-lease-prune-race, finding.command-cell-cancel-artifact-settlement]
    unknown: retained lease prune 与普通 cancel artifact settlement 存在确认的生命周期竞态
    next_step: 分别 repair lease-aware prune 与 cancellation-bounded finalization
---

# Task、Subagent、Workflow 与 Scheduler

## 能力范围

覆盖 revisioned Task graph、runtime execution、Subagent attempt、Team、Workflow、Scheduler、BackgroundTask 和 CommandCell。

## 入口与输出

Task tools、programmatic runtime、Agent dispatch、Workflow Rust/JSON/YAML、cron tick 与 command tools 触发；输出 claim/receipt/event/checkpoint/effect。

## 行为关系

Task graph 和 Subagent 是主任务执行关系；Workflow/Scheduler/Background 是相邻通用能力，不凭采用量判死或自动归并。

## 状态与数据流

Task spec/execution/claim、Subagent identity/control/envelope、Workflow state/checkpoint、Scheduler store/cache 各有明确 owner，但存在记录的冲突。

## 策略来源与优先级

Task policy/controller、Subagent prompt/isolation/admission、Workflow graph definition、cron store 与 CommandCell config 决定行为。

## 生命周期与失败路径

Revision/claim/wave/settle/retry/pause/cancel、dispatch/message/interrupt/terminal、checkpoint/resume、tick/shutdown、spawn/wait。

## 权限与敏感信息

Subagent 复用 Agent tool policy；Workflow node/cron callback 本身不替代外层 permission；CommandCell 可要求 sandbox。

## 用户侧投影

TaskEvent/progress、Subagent envelopes、Workflow events 和 command snapshots 是消费者视图，不取代各自 authority。

## 场景处置清单

所有主要入口已映射；Task relation patch与Subagent factory的两个竞态已关闭，其余十三个当前缺口继续进入Finding/needs_review。Workflow、Scheduler、BackgroundTask与CommandCell不合成一个authority。

## 未展开项

具体 Finding 修复由后续独立 delivery outcome 处理。
