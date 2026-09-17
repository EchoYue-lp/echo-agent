---
schema_version: 1
id: evidence.tool-terminal-observation-repair
kind: evidence
observed_at: source:e35a087894af8f5cca0ff6c6ac2887db78e8d7ed6748d0fdadae5d16509a2896
source_refs:
  - src/agent/react/run/pipeline.rs
  - src/agent/snapshot.rs
  - echo-state/src/audit/mod.rs
  - echo-state/src/audit/memory.rs
  - docs/en/security.md
  - docs/zh/security.md
  - echo-agent-learning/tests/example_contracts/demo64_tool_pipeline.rs
supports: [finding.tool-terminal-observation-divergence, behavior.effect-permission-execution, rule.permission-effect-order]
limitations:
  - Deferred SDK parity of tool terminal observation is tracked by the SDK repository and is outside this framework repair
  - Trace with no attached RunStore records callbacks only; durable trace coverage is bound to the configured RunStore
---

# 工具终态观察一致性修复

## 支持的结论

PostToolUse block 之后的观察阶段不再被管线短路：AuditStage、TraceRecordingStage、CallbackStage::END、OutputGuardStage
与 TruncationStage 通过 `runs_after_block` 在 block 后继续运行；执行前拦截（intervention/visibility/plan/
permission 等无 result 的 block）仍短路，不伪造执行事实。CallbackEnd 依据真实 ToolResult 分流：
失败结果走 `on_tool_error`，成功结果走 `on_tool_end`，失败不再被报告为成功。

PostToolUse 拦截保留已执行调用的输出与恢复事实：拦截后 ToolResult.success 置为 false，保留
output/artifact/metadata，`ToolFailure` 继承原 failure 或升级为 PartialSideEffect 并附加 postcondition，
caller（`execute_tool_with_policy`）返回携带真实输出的失败结果，不再用合成失败覆盖已发生的事实。

## 来源与范围

实现位于既有 16-stage ToolExecutionPipeline 与 canonical caller；公开 builder 的
`audit_logger` 由 AuditStage 在 post-hook、输出护栏与预算处理后记录一个终态 ToolCall，删除执行异常
分支的重复审计。AuditCallback 仍是显式 callback adapter，不由 builder 隐式注册。
回归测试从真实 caller 入口驱动，断言 caller 失败终态、
保留输出、trace ToolResult(success=false)+ToolError、audit success=false 一致。

所有实际执行结果的输出均经同一 OutputGuardStage，caller 成功和失败都使用处理后输出；
护栏返回空串不回退到原始输出。无输出失败的错误文本只形成经过护栏的内部审计投影，
caller 的错误诊断不变。公开 audit_logger 的测试覆盖执行后拦截、执行前拦截、失败结果、
成功/失败输出清空和 error-only 错误清空。
demo64 可执行示例同步为实际 16-stage 顺序与执行后 block 的观察义务，移除已不存在的
ParseValidateStage 文案，并将工具参数校验明确归入 ToolManager 执行边界。

## 已知缺口

on_tool_error 事件粒度以 AuditLogger backend 为准；SDK 侧等价观察合同属于独立 SDK 仓库后续事项。
