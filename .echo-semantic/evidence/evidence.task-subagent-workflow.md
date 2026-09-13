---
schema_version: 1
id: evidence.task-subagent-workflow
kind: evidence
observed_at: f1e9027246760661144786e9e35615cd46d580c6
source_refs:
  - echo-orchestration/src/tasks/revisioned.rs
  - echo-orchestration/src/tasks/runtime.rs
  - echo-orchestration/src/tasks/runtime_service.rs
  - echo-orchestration/src/tasks/runtime_executor.rs
  - echo-orchestration/src/tasks/background_task.rs
  - echo-orchestration/src/tasks/background_state.rs
  - echo-orchestration/src/tasks/command_cell.rs
  - src/agent/subagent/registry.rs
  - src/agent/subagent/executor.rs
  - src/agent/subagent/control.rs
  - src/agent/subagent/events.rs
  - src/agent/subagent/team/mod.rs
  - echo-sdk-host/src/core_profile/facade/task_runtime.rs
  - echo-orchestration/src/workflow/mod.rs
  - echo-orchestration/src/workflow/graph.rs
  - echo-orchestration/src/workflow/checkpoint_store.rs
  - echo-orchestration/src/workflow/dag.rs
  - echo-orchestration/src/scheduler/runner.rs
  - echo-orchestration/src/scheduler/cron_task.rs
  - docs/adr/0008-canonical-runtime-task-authority.md
  - docs/adr/0024-unified-subagent-prompt-compilation.md
  - docs/adr/0027-subagent-communication-primitives.md
  - docs/adr/0030-versioned-subagent-event-envelope.md
  - docs/adr/0025-deterministic-command-cell-watcher.md
  - tests/facade_smoke.rs
supports: [behavior.task-subagent-execution, rule.task-subagent-authority]
limitations:
  - Task DAG、DagWorkflow 与一般 Graph 的抽象重叠尚未经过 consolidation audit
---

# Task、Subagent 与 Workflow 证据

## 支持的结论

`TaskRevisionService` 唯一负责 revisioned graph CRUD/关系/校验，`RuntimeTaskService` 唯一负责 dependency execution；所有 Subagent 模式经过 `SubagentRegistry` 与 `SubagentExecutor`；Workflow、Scheduler、BackgroundTask 和 CommandCell 是相邻但不同的运行边界。

## 来源与范围

来源覆盖 task spec/execution/claim、runtime controller、Team/SDK TaskClaim adapter、Subagent registry/executor/control/events、Workflow graph/event/checkpoint、Scheduler store/runner、BackgroundTask、CommandCell 与相关 ADR/测试。

## 已知缺口

公共 framework 能力没有 root 应用构造点不等于死代码；Task DAG 与 Workflow 图之间是否存在应归并的重复，需要高风险审计而不是 discovery 阶段删除。
