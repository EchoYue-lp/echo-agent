---
schema_version: 1
id: evidence.tool-terminal-observation-repair
kind: evidence
observed_at: source:PENDING
source_refs:
  - src/agent/react/run/pipeline.rs
  - src/agent/snapshot.rs
  - echo-state/src/audit/mod.rs
  - echo-state/src/audit/memory.rs
supports: [finding.tool-terminal-observation-divergence, behavior.effect-permission-execution, rule.permission-effect-order]
limitations:
  - Deferred SDK parity of tool terminal observation is tracked by the SDK repository and is outside this framework repair
  - Trace with no attached RunStore records callbacks only; durable trace coverage is bound to the configured RunStore
---

# 工具终态观察一致性修复

## 支持的结论

PostToolUse block 之后的观察阶段不再被管线短路：TraceRecordingStage、CallbackStage::END、OutputGuardStage
与 TruncationStage 通过 `runs_after_block` 在 block 后继续运行；执行前拦截（intervention/visibility/plan/
permission 等无 result 的 block）仍短路，不伪造执行事实。CallbackEnd 依据真实 ToolResult 分流：
失败结果走 `on_tool_error`，成功结果走 `on_tool_end`，失败不再被报告为成功。

PostToolUse 拦截保留已执行调用的输出与恢复事实：拦截后 ToolResult.success 置为 false，保留
output/artifact/metadata，`ToolFailure` 继承原 failure 或升级为 PartialSideEffect 并附加 postcondition，
caller（`execute_tool_with_policy`）返回携带真实输出的失败结果，不再用合成失败覆盖已发生的事实。

## 来源与范围

实现位于既有 16-stage ToolExecutionPipeline 与 canonical caller；AuditCallback 沿用既有
on_tool_error 通道，不新增第二观察权威。回归测试从真实 caller 入口驱动，断言 caller 失败终态、
保留输出、trace ToolResult(success=false)+ToolError、audit success=false 一致。

## 已知缺口

on_tool_error 事件粒度以 AuditLogger backend 为准；SDK 侧等价观察合同属于独立 SDK 仓库后续事项。
