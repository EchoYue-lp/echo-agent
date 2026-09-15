---
schema_version: 1
id: evidence.workflow-entry-loop-authority-repair
kind: evidence
observed_at: source:89eee6de1f98710cd7a2f520432226375921ea3175a8f8f61ea900feaf4e7248
source_refs:
  - echo-orchestration/src/workflow/graph.rs
  - echo-orchestration/src/workflow/node.rs
  - echo-orchestration/src/workflow/pipelines/data_pipeline.rs
  - echo-orchestration/src/workflow/pipelines/writing_pipeline.rs
  - docs/adr/0052-workflow-entry-loop-authority.md
  - docs/en/17-graph-workflow.md
  - docs/zh/17-graph-workflow.md
supports: [behavior.task-subagent-execution]
limitations:
  - 完整workspace门禁、all-features与远端CI留给任务汇总分支
  - 不修改CheckpointStore、SDK facade、Task DAG、DagWorkflow或Scheduler合同
evidence_type: behavior_equivalence
before_revision: c5f7688212d45d5bdcdbf60342605e8bfb176cae
after_revision: source:89eee6de1f98710cd7a2f520432226375921ea3175a8f8f61ea900feaf4e7248
scenario_results:
  four-entry-routing-authority:
    status: matched
    source_refs: [echo-orchestration/src/workflow/graph.rs, docs/adr/0052-workflow-entry-loop-authority.md]
  checkpoint-resume-settlement:
    status: matched
    source_refs: [echo-orchestration/src/workflow/graph.rs, echo-orchestration/src/workflow/checkpoint_store.rs]
  workflow-event-terminal-order:
    status: matched
    source_refs: [echo-orchestration/src/workflow/graph.rs, echo-orchestration/src/workflow/mod.rs]
  agent-token-and-final-state:
    status: matched
    source_refs: [echo-orchestration/src/workflow/node.rs, echo-orchestration/src/workflow/graph.rs]
  agent-producer-cancellation:
    status: matched
    source_refs: [echo-orchestration/src/workflow/node.rs, echo-orchestration/src/workflow/graph.rs]
command_results:
  - command: "CARGO_INCREMENTAL=0 CARGO_PROFILE_DEV_DEBUG=0 CARGO_PROFILE_TEST_DEBUG=0 CARGO_BUILD_JOBS=2 cargo test -p echo_orchestration --lib workflow:: --locked"
    exit_code: 0
  - command: "CARGO_INCREMENTAL=0 CARGO_PROFILE_DEV_DEBUG=0 CARGO_PROFILE_TEST_DEBUG=0 CARGO_BUILD_JOBS=2 cargo test -p echo-agent-learning --test example_contracts contract_demo34_workflow_stream --locked"
    exit_code: 0
  - command: "CARGO_INCREMENTAL=0 CARGO_PROFILE_DEV_DEBUG=0 CARGO_PROFILE_TEST_DEBUG=0 CARGO_BUILD_JOBS=2 cargo test -p echo-agent-learning --test example_contracts --features testing contract_demo39_workflow --locked"
    exit_code: 0
coverage:
  - Fresh run, interruptible run, claimed resume, and public stream delegation
  - Linear, conditional, loop, fan-out, finish, cancellation, timeout, and checkpoint continuation
  - Token, NodeError, NodeEnd, Completed, path, step, and SharedState ordering
  - Agent producer cancellation on stream drop, Graph cancel, timeout, and sibling failure
---

# Workflow 入口循环权威修复证据

## 支持的结论

`Graph::execute_loop`成为run、run-until-interrupt、resume和run-stream唯一节点推进权威，统一
拥有cancel/max-step、node execution、route、fan-out、finish、interrupt checkpoint、path/step、
`NodeError`和`Completed`。四个公开入口只构造cursor、管理既有checkpoint claim lease或投影
event channel，不再复制节点主循环。

Graph Agent节点统一排空带CancellationToken的Agent stream。stream drop、Graph cancel、node
timeout和parallel sibling failure都会通过drop guard取消producer；有消费者时Token实时投影，
无消费者时丢弃Token但仍观察同一FinalAnswer/Error/Cancelled终态。旧Node buffered执行路径已删除。

## 来源与范围

实现基于GitHub Issue #112和ADR 0052。LangGraph以stream/astream作为invoke/ainvoke共同执行
路径，Temporal以单一Event History replay恢复状态；本修复采用相同的“多入口投影同一执行
authority”原则，但保留echo-agent既有in-process Graph与CheckpointStore合同。

## 已知缺口

本修复不归并Task DAG、DagWorkflow或Scheduler，也不改变远端SDK wire。全局semantic source
snapshot由integration branch统一刷新，避免并行Finding分支机械覆盖共享digest。
