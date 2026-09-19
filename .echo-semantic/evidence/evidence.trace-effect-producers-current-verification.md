---
schema_version: 1
id: evidence.trace-effect-producers-current-verification
kind: evidence
observed_at: source:c692702d1e9c1752aa396348037aea8baab1b4a8f1bbc2979fe95fc5ec9c7323
source_refs:
  - echo-tools/src/files/files.rs
  - echo-tools/src/files/apply_patch.rs
  - src/eval/runner.rs
  - src/eval/replay.rs
  - src/eval/regression.rs
  - src/trace/analyzer.rs
  - src/improve/analyzer.rs
  - src/improve/trajectory.rs
  - echo-agent-learning/tests/example_contracts/demo50_eval.rs
  - echo-agent-learning/tests/example_contracts/demo51_self_improvement.rs
  - src/agent/subagent/executor.rs
  - src/tools/builtin/agent_dispatch.rs
  - src/agent/react/run/pipeline.rs
  - src/agent/react/run/phases/tools.rs
  - src/trace/mod.rs
supports: [finding.trace-effect-event-producers]
limitations:
  - All previously recorded package results predate final FileRead and ToolExecutionSkipped consumers
  - Cross-repository SDK mapping and CLI consumer gates remain separate
---

# Issue 104 verification frontier

## 支持的结论

测试入口包括文件真实变更/无变更、partial patch/dry-run、Eval TestPass/SWE-bench
实际命令与 126/127、typed effect 前于 stalled post-use stage、SubagentExecutor 的真实
terminal 分类、后台 Subagent 反序完成与父 trace 关联、timeout/cancel 后调用级终态数量，
以及 Trace/Eval/Improvement analyzer、Replay/Regression 不把 skipped call 计作执行而
trajectory 仍保留 admitted pair。须在最终源码摘要上运行对应测试，
确认事件 payload、demo50 trajectory 与 demo51 improvement 示例和 caller 事实一致。
当前 Finding 仍 open。

## 来源与范围

所列源码包含 effect producer、projection、replay/eval/improvement consumer、demo50/demo51
可执行示例与相关回归入口；完整工程命令见 `evidence.observation-current-final-gates`。

## 已知缺口

本地 focused/root/workspace 已通过；独立复审与 SDK 消费者仍未有通过收据。
