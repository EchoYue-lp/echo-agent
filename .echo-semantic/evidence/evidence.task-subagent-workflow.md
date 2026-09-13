---
schema_version: 1
id: evidence.task-subagent-workflow
kind: evidence
observed_at: source:d61c2341a008920576462b3051374115cf1b4da682c341852b052224f022d027
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
  - docs/adr/0033-subagent-factory-singleflight-publication.md
  - docs/adr/0025-deterministic-command-cell-watcher.md
  - docs/adr/0039-background-task-terminal-authority.md
  - docs/en/29-long-running-tasks.md
  - docs/zh/29-long-running-tasks.md
  - tests/facade_smoke.rs
supports: [behavior.task-subagent-execution, rule.task-subagent-authority]
limitations:
  - Task DAG、DagWorkflow 与一般 Graph 的抽象重叠尚未经过 consolidation audit
---

# Task、Subagent 与 Workflow 证据

## 支持的结论

`TaskRevisionService`唯一负责revisioned graph CRUD/关系/校验，`RuntimeTaskService`唯一负责dependency execution；所有Subagent模式经过`SubagentRegistry`与`SubagentExecutor`。Registry entry revision与OnceCell共同拥有lazy factory的single-flight publication。TaskSpawner的BackgroundTaskHandleState原子保存process-local status/result，Clone handle、多waiter、admission/deadline与child-task JoinHandle已形成修复证据；公开BackgroundTaskState checkpoint、Workflow、Scheduler和CommandCell是相邻但不同的运行边界。

## 来源与范围

来源覆盖task spec/execution/claim、runtime controller、Team/SDK TaskClaim adapter、Subagent registry/executor/control/events、Workflow graph/event/checkpoint、Scheduler store/runner、BackgroundTask handle/checkpoint、CommandCell与相关ADR/测试。

## 已知缺口

公共 framework 能力没有 root 应用构造点不等于死代码；Task DAG 与 Workflow 图之间是否存在应归并的重复，需要高风险审计而不是 discovery 阶段删除。
