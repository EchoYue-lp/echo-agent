---
schema_version: 1
id: evidence.task-subagent-external-control-handle-repair
kind: evidence
observed_at: source:a8b11a40111a41cb6dc345b3798947d426bbdf0318f641868b819c0d5f58a88b
source_refs:
  - src/agent/subagent/executor.rs
  - src/agent/subagent/team/mod.rs
  - src/agent/subagent/mod.rs
  - tests/facade_smoke.rs
  - docs/adr/0058-task-claim-subagent-attempt-control.md
supports: [behavior.task-subagent-execution, rule.task-subagent-authority]
limitations:
  - The handle is process-local and does not provide durable TaskClaim validation or command replay
  - echo-agent-sdk still requires a framework pin, persistent Task graph, command journal, language contract update, and cross-process E2E
---

# External Task adapter attempt-control handle 修复证据

## 支持的结论

`SubagentExecutor::attempt_control_handle`现在返回一个固定`control_scope_id`的完整live capability。
reservation与dispatch只从`TaskSubagentContext`派生identity；interrupt、retire与reconcile只从
`run/task/TaskClaim`和固定scope派生identity。handle不暴露`SubagentControlRegistry`，也不加载Task
store、判断claim currency或写Task terminal。

programmatic Team与React Team已改用同一handle，删除了各自重复的identity assembly和直接私有helper
调用。外部`RuntimeDagController`因此可以实现完整hook，而durable public command仍必须先经过
`RuntimeTaskService::request_attempt_interrupt`。

## 来源与范围

实现复用既有`SubagentExecutor`、`SubagentControlRegistry`、`TaskSubagentContext`、
`SubagentAttemptIdentity`与runtime typed receipts，没有新增依赖、registry、store、scheduler或terminal。
ADR 0058与双语长任务文档记录了live capability和durable authority边界。

## 已知缺口

本证据只覆盖framework public adapter capability。SDK Host当前仍使用旧framework pin和内存Task store，
且没有durable task-control command ledger；这些后续Outcome完成前Finding与Issue #99保持open。
