---
schema_version: 1
id: evidence.tool-terminal-current-repair
kind: evidence
observed_at: source:b214951ece8e09325efc846ad7bd88a402135000e42fe67d2b917317b2d27923
source_refs:
  - echo-core/src/agent/mod.rs
  - echo-core/src/audit.rs
  - echo-core/src/tools/mod.rs
  - echo-state/src/audit/mod.rs
  - src/agent/react/run/pipeline.rs
  - src/agent/react/run/phases/tools.rs
  - src/agent/snapshot.rs
  - src/trace/mod.rs
  - docs/adr/0059-observed-tool-effects-and-background-dispatch.md
supports: [finding.tool-terminal-observation-divergence, behavior.effect-permission-execution, rule.permission-effect-order]
limitations:
  - Historical Issue 102 repair remains valid for its old revision; this evidence describes new uncommitted regressions
  - Current final-digest tests, consumer checks and independent rereview are pending
---

# Issue 102 current regression repair candidate

## 支持的结论

OutputGuard 使输出失效时同步清除先前 artifact/metadata，不把无效原文通过 artifact
重新暴露。失败 trace 的 ToolResult preview 取真实经处理的输出，错误文本只作为
ToolError 诊断。执行中 stage Err、外层 timeout/cancel 为已启动调用产生一次 failed
terminal，已结算调用不重复补写；同步 caller 收到与 trace/audit 相符的结果。
从未进入 ExecuteStage 的调用先记录 `ToolCall` 和 `ToolExecutionSkipped`，再记录合成的
失败 `ToolResult`/`ToolError`；已执行或已结算调用不添加 skipped marker。AgentCallback
新增可选的 call_id-aware start/end/error/interrupted 桥。中断桥接携带 admitted input，
既能在 start callback 前持久化一次失败终态，也能在 start 后取回同一 call_id 的原输入，
不伪造重复 start。AuditCallback 以 canonical call_id 相关并发同名调用，仍保留旧 callback
合同供已有实现使用。

## 来源与范围

涉及框架公共 callback 合同、ReactAgent 的 16-stage pipeline、调用级 trace terminal 与
callback/audit adapter；`ToolExecutionSkipped` 只说明未进入执行，不改写 caller failure。

## 已知缺口

旧 Issue 102 的历史全门禁不能覆盖当前 uncommitted 差异；最终复审待完成。
