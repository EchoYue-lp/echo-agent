---
schema_version: 1
id: evidence.task-subagent-attempt-link-repair
kind: evidence
observed_at: f310825c418932cde02f661f0686f83f216771d6
source_refs:
  - echo-orchestration/src/tasks/runtime.rs
  - echo-orchestration/src/tasks/runtime_executor.rs
  - echo-orchestration/src/tasks/runtime_service.rs
  - src/agent/subagent/control.rs
  - src/agent/subagent/executor.rs
  - src/agent/subagent/team/mod.rs
  - docs/adr/0058-task-claim-subagent-attempt-control.md
supports: [behavior.task-subagent-execution, rule.task-subagent-authority]
limitations:
  - echo-agent-sdk Host adapter and durable command replay remain pending
  - remote effects accepted before cancellation cannot be retracted by a local token
---

# TaskClaim 与 SubagentAttempt framework 修复证据

## 支持的结论

`TaskClaim::execution_id(run_id, task_id)`现在是 Team/runtime physical Subagent attempt
identity 的唯一来源。`TaskSubagentContext`无损携带 run、task、revision、attempt、execution
与 task-child cancellation；默认 Team、React Team 与 caller-supplied runtime 均通过完整
`TeamDispatchController`和同源 `RuntimeTaskService`执行，不再由 adapter 重建第二套 lineage。

精确 interrupt 在 reservation 前可有界排队，在 reserved/active 阶段只取消目标 child；
non-cooperative target 由原 JoinSet 的 exact AbortHandle 在 grace 后定向中止，root cancellation
才使用 wave-wide abort。joined attempt 在 durable CAS 前仍可寻址；settlement/claim authority
同时未知时发布 typed `AuthorityUnknown`，恢复读取 durable snapshot 后先释放 stale waiter、退休
joined supervisor 状态，再尽力清理 live controller。post-CAS cleanup failure 只形成 observer
retry debt，不反转持久终态。

## 来源与范围

Task graph 与 TaskClaim CAS 仍是唯一持久 authority。pending/reserved/active/settled registry、
abort handle、watch 与 retained Team runtime 都是有界进程投影；它们不能提交 task terminal。

## 已知缺口

SDK Host 仍负责 durable command ledger 与重放，本次 framework commit 不宣称跨进程 command
恢复已经完成。已被远端接纳的 effect 也不能由本地 cancellation 撤回。
