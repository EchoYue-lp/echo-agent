---
schema_version: 1
id: evidence.workflow-dag-authority-repair
kind: evidence
observed_at: d75bd27b1cee425fad4fdfa20c99f528d925ff8d
source_refs:
  - docs/adr/0059-task-workflow-dag-authority.md
  - docs/en/09-tasks.md
  - docs/zh/09-tasks.md
  - docs/en/17-graph-workflow.md
  - docs/zh/17-graph-workflow.md
  - echo-orchestration/src/tasks/revisioned.rs
  - echo-orchestration/src/tasks/runtime_service.rs
  - echo-orchestration/src/workflow/graph.rs
  - echo-orchestration/src/workflow/dag.rs
supports: [behavior.task-subagent-execution]
limitations:
  - 本轮不改变三套既有运行时、公共API、序列化或SDK合同
  - Finding仍待独立复审、严格语义快照与远端主线交付
---

# Task 与 Workflow 图权威决策证据

## 支持的结论

ADR 0059 按边含义、运行时权威与恢复事实区分三套公共能力：Task graph 的
`TaskRevisionService` 持有 revision/关系修改，`RuntimeTaskService` 持有
ready frontier 和 exact claim settlement；`Graph::execute_loop` 持有条件路由、循环、
`SharedState`、checkpoint continuation；`DagWorkflow::run` 持有静态 Agent 文本管道的
拓扑批处理。三个所有者各有有效消费者，不按 EKO 采用量删 Workflow API。

Task 派发 Workflow 的组合仅把 Workflow 输出当作原 Task claim 的执行证据，必须由
Task owner 提交终态；不存在从 Task graph 到任意 Workflow 的自动转换。双语任务与
Workflow 章节均引用 ADR，后者纠正了此前把静态管道写作“DAG tasks”的歧义。

## 来源与范围

Issue #111 与 `finding.workflow-dag-authority` 提出三者长期边界缺失。决策复用
ADR 0040 的 Workflow checkpoint lease、ADR 0052 的 Graph 唯一入口循环，以及
现有 Task revision/runtime service，未引入平行 state machine。官方 Temporal
Workflow history、LangGraph checkpoint/Pregel 和 Tokio graceful shutdown 文档用于
比较所有权与结算原则；具体 EKO/echo-agent 合同以本仓库源码为准。

## 已知缺口

这份决策不声称现存所有 Workflow validation、Task recovery 或外部副作用缺陷已修复；
它也不充当独立复审。完整 workspace 与远端 CI 由整合任务分支执行。
