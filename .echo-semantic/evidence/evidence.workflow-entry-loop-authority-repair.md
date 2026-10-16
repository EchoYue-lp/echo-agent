---
{
  "schema_version": 1,
  "id": "evidence.workflow-entry-loop-authority-repair",
  "kind": "evidence",
  "observed_at": "7110a27e7b0e9841dcfa0d4cca1cdbc053d96177",
  "source_refs": [
    "echo-orchestration/src/workflow/graph.rs",
    "echo-orchestration/src/workflow/node.rs",
    "echo-orchestration/src/workflow/pipelines/data_pipeline.rs",
    "echo-orchestration/src/workflow/pipelines/writing_pipeline.rs",
    "docs/adr/0052-workflow-entry-loop-authority.md",
    "docs/en/17-graph-workflow.md",
    "docs/zh/17-graph-workflow.md",
    "echo-orchestration/src/workflow/checkpoint_store.rs",
    "echo-orchestration/src/workflow/mod.rs"
  ],
  "supports": [
    "behavior.task-subagent-execution"
  ],
  "limitations": [
    "Current7110 native observation only; original unavailable history, old executions and old reviews are not reconstructed or reexecuted.",
    "Current source/test/limit mapping is in evidence.framework-history-current-reobservation; full05 is existing execution, not a newly run historical command."
  ]
}
---

# Current observation and historical record

The machine header now records only the bounded native observation at `7110a27e7b0e9841dcfa0d4cca1cdbc053d96177`; current source/tests and limitations are in `evidence.framework-history-current-reobservation`. Original observation `c7e54e6785f85422af83d4b384f01c2493eddc79` and its complete metadata remain in [the immutable875 document](https://github.com/EchoYue-lp/echo-agent/blob/875cc22f2e99c7ccd83c07e02c03fe221c5e3f6c/.echo-semantic/evidence/evidence.workflow-entry-loop-authority-repair.md). The original execution counts, before/after claims and review prose below are preserved verbatim as historical statements, not rerun or equivalence claims for7110.


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
