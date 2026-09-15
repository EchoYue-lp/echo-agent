---
schema_version: 1
id: evidence.workflow-entry-loop-authority-verification
kind: evidence
observed_at: c7e54e6785f85422af83d4b384f01c2493eddc79
source_refs:
  - echo-orchestration/src/workflow/graph.rs
  - echo-orchestration/src/workflow/node.rs
  - echo-orchestration/src/workflow/pipelines/data_pipeline.rs
  - echo-orchestration/src/workflow/pipelines/writing_pipeline.rs
  - echo-agent-learning/tests/example_contracts/demo34_workflow_stream.rs
  - echo-agent-learning/tests/example_contracts/demo39_workflow.rs
supports: [behavior.task-subagent-execution]
limitations:
  - 完整workspace门禁、all-features、远端Linux/Windows CI与真实provider副作用未执行
  - 独立rereview已PASS；Finding关闭与共享source snapshot刷新等待integration final gate
---

# Workflow 入口循环权威验证证据

## 支持的结论

三条回归先在旧实现上稳定失败：Agent Token列表为空、finish-node `interrupt_after`直接返回
Completed、parallel branch错误前没有`NodeError`。实现后30项Graph测试通过；独立review随后
发现非流式Agent producer取消缺口，修复并新增四类取消回归后，整个`workflow::`定向集合70项
全部通过。

demo34 stream合同1项和启用`testing` feature的demo39 Agent workflow合同1项实际执行通过。
`cargo check -p echo_orchestration --locked`通过；package all-target `-D warnings` Clippy和lib/bins
unwrap/expect/panic/unreachable严格Clippy通过；package formatter写入后check通过，`git diff --check`
无错误。

## 来源与范围

验证绑定候选提交`c7e54e67`；命令均在独立#112 worktree、Rust 2024工具链和低debug、
无incremental、2 jobs约束下执行。仅覆盖Workflow Graph及其直接example contract，不把
SDK、Scheduler或完整workspace结果推导为通过。

## 已知缺口

第一次未启用`testing`的demo39命令收集到0项，不计为验收；随后使用真实feature入口重跑1项
通过。未运行完整workspace、all-features、逐feature矩阵或远端CI。
