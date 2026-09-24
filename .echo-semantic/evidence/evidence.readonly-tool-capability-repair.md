---
schema_version: 1
id: evidence.readonly-tool-capability-repair
kind: evidence
observed_at: bd17c73075d6b3cf8e00877fa0fb10d36694ea54
source_refs:
  - src/agent/config.rs
  - src/agent/react/builder.rs
  - src/agent/react/capabilities.rs
  - src/agent/react/mod.rs
  - src/agent/react/run/pipeline.rs
  - src/agent/snapshot.rs
  - echo-orchestration/src/tasks/task_tools.rs
  - src/tools/builtin/cell_tools.rs
  - src/tools/builtin/subagent_message.rs
  - docs/adr/0071-protected-path-and-readonly-tool-boundary.md
  - docs/en/02-tools.md
  - docs/zh/02-tools.md
supports: [behavior.effect-permission-execution, rule.permission-effect-order]
limitations:
  - 自定义 Tool 作者须正确声明本地 ToolCapabilities；本修复不推断任意 Tool 的真实副作用
  - 完整 workspace 门禁与 17 项独立 feature 编译由 verification Evidence 记录；PR/CI 与远端 main 交付待完成
---

# Read-only Tool capability 修复证据

## 支持的结论

`readonly_tools` 使用已有 `ToolCapabilities::is_read_only` 判定 Agent 自动工具入口。
Builder 构造时排除 mutating custom Tool；之后 `add_tool`、`add_tools`、
`replace_tool` 与 Agent trait 注册维持相同边界。即使 caller 绕过 Agent API 直接
写 ToolManager，LLM invocation view 仍隐藏 mutating Tool，PlanModeStage 在执行前
再次阻断。没有新建工具名称黑名单或第二个 permission authority。

Framework 观察工具 `task_list`、`list_cells`、`subagent_list` 声明 read-only；
task 更新、cell stop、subagent message 保持 mutating。`recall`、`search_memory`
会通过 MemoryRecaller 写持久 recall telemetry，layered search 还可能 reconcile，
因此不错误地暴露给只读 Agent。

## 来源与范围

修复位于 `cd37e5d3`；ADR 0071 与双语 Tools 文档记录构造、可见性、执行三处
同一 capability 事实。源码涉及 builder/config、ReactAgent 注册入口、snapshot、
pipeline 和三组内置 Tool 实现。

## 已知缺口

这是 framework Agent 自动工具合同；ToolManager 的公开 programmatic primitive
仍由 caller 拥有策略。本证据不声称工具能力声明可以替代实测副作用审计。
