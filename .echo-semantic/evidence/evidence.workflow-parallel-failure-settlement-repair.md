---
schema_version: 1
id: evidence.workflow-parallel-failure-settlement-repair
kind: evidence
observed_at: 8c8fdffc4a2dfb341d9b25e32a321c9fb41c7350
source_refs:
  - echo-orchestration/src/workflow/graph.rs
  - echo-orchestration/src/workflow/dag.rs
  - echo-orchestration/src/workflow/concurrent.rs
  - docs/adr/0040-workflow-checkpoint-lease-and-sibling-settlement.md
supports: [behavior.task-subagent-execution]
limitations:
  - Checkpoint claim settlement与lease续租仍由finding.workflow-checkpoint-claim-recovery和finding.workflow-checkpoint-resurrection-race追踪
  - 四个Graph执行入口的事件归约仍由finding.workflow-entry-loop-drift追踪
---

# Workflow sibling failure结算修复证据

## 支持的结论

Graph、DagWorkflow与ConcurrentWorkflow不再按提交顺序等待全部并行任务才观察错误。
它们通过受监督future或`JoinSet::join_next`观察first-completed failure，立即取消其余sibling，
并在返回前drain handle；外层future被取消时，JoinSet drop也会abort仍存活任务。

成功路径将completion-order结果按registration或topological batch index缓存，全部成功后再
生成merge输入、`WorkflowOutput.steps`和DAG ready frontier，因此fail-fast修复不改变既有
稳定输出顺序。

## 来源与范围

初始failure-settlement由commit `90c43d95`引入，成功顺序修正固定于
`8c8fdffc4a2dfb341d9b25e32a321c9fb41c7350`。实现复用现有Workflow/JoinSet，不新增
第二scheduler或terminal reducer。

## 已知缺口

本证据不证明checkpoint crash recovery、远端Store settlement或多个Graph入口完全同构。
