---
schema_version: 1
id: evidence.trace-effect-producers-current-repair
kind: evidence
observed_at: source:b214951ece8e09325efc846ad7bd88a402135000e42fe67d2b917317b2d27923
source_refs:
  - echo-core/src/tools/mod.rs
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
  - src/agent/subagent/events.rs
  - src/tools/builtin/agent_dispatch.rs
  - src/agent/react/run/pipeline.rs
  - src/agent/snapshot.rs
  - src/agent/react/run/phases/tools.rs
  - src/trace/mod.rs
  - docs/adr/0059-observed-tool-effects-and-background-dispatch.md
supports: [finding.trace-effect-event-producers, behavior.effect-permission-execution, behavior.observation-persistence]
limitations:
  - Proposed producer paths remain uncommitted and current full validation/review is pending
  - Optional failure_count stays unknown when the test runner has no structured count
  - Background Subagent observation is in-process; process abort still needs an external durable owner
---

# Issue 104 typed event producer repair candidate

## 支持的结论

`ToolEffect` 由执行 effect 的工具拥有。file/read 和真实 file mutation 工具产生 FileRead/
FileEdit；dry-run、未执行与无确认写入不伪造 FileEdit。ReactAgent 把 ToolResult.effects
投影到准确的 trace run，且在执行完成后、PostToolUse presentation 之前追加。EvalRunner 的
TestPass/SWE-bench criterion 在已执行的测试命令终态直接追加 TestRun，exit 126/127/
launch failure 不等于测试运行；failure_count 不知道时为 None。`agent_tool` 前台结果
携带 Subagent terminal effect；后台只在真实 Subagent terminal 后通过 invocation-scoped
sink 绑定父 trace 与 call_id，启动 ack 不冒充 Subagent 完成。

`SubagentExecutor` 根据真实 outcome 区分 DispatchCompleted/DispatchFailed/DispatchCancelled，
返回非终态 Running 被归一为失败，避免用 launch 或非终态回填成功。

外层 timeout/cancel 仅为未结算调用补 synthetic failed terminal，已完成调用保留真实结果；
trace effect 仍是诊断投影，不替代实际文件、测试或 Subagent authority。ADR 0059
记录适配与不推断副作用的取舍。

成功 `read_file` 以解析后路径产生 `FileRead`，空文件也成立，失败读取不产生；runtime
read-before-edit、Replay 与 Eval 消费该确认事实而非 ToolCall 请求参数。未进入 ExecuteStage
的调用以 `ToolExecutionSkipped` 标记；Trace/Eval/Improvement analyzer、Replay 与 regression
criteria 将该 call_id 从“已执行工具”集合排除，同时 trajectory 导出保留已接纳的请求/
结果配对用于诊断与恢复分析。

## 来源与范围

生产路径在 `echo-tools` file tools、EvalRunner、SubagentExecutor、agent_tool 与 ReactAgent pipeline；
Analyzer/Replay/Regression/Improvement 是确认 effect 与 skipped execution 的消费者；`echo-core::tools`
仅声明 typed effect contract。demo50/demo51 的可执行 trajectory/improvement 示例显式提供
FileRead/FileEdit 事实，不再从 ToolCall 名称或参数推断 effect。

## 已知缺口

最终源码验证、SDK 映射与后台进程 abort 后的外部恢复均不由本候选证明。
