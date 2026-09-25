---
schema_version: 1
id: evidence.tool-pipeline-example-repair
kind: evidence
observed_at: source:b9df4130cf07d2561e8e61ec24d127607ce21112eca995952c64c1290ede3b74
source_refs:
  - src/agent/react/run/pipeline.rs
  - echo-agent-learning/tests/example_contracts/demo64_tool_pipeline.rs
  - docs/en/02-tools.md
  - docs/zh/02-tools.md
supports: [finding.tool-pipeline-example-drift, behavior.effect-permission-execution, rule.permission-effect-order]
limitations:
  - The learning contract covers the default pipeline on a successful tool call, not every custom pipeline or blocked branch
  - Structured tracing names are an internal observation surface, not a new public framework API
  - Full workspace gate, remote CI, and mainline delivery remain pending
---

# demo64 Tool pipeline executable contract repair

## 支持的结论

demo64 从真实 Agent 工具调用中收集 `ToolExecutionPipeline::run` 发出的
`stage.name()` 结构化 tracing 字段，以观测到的顺序打印阶段。测试要求每个预期阶段
恰好出现一次、无未知阶段，并检查干预、可见性、计划模式、PreToolUse、权限、
先读后改、输入守卫、canonical invocation、执行、后置 Hook、
输出守卫、预算、Trace、Audit 和终态回调之间的关键顺序。它不再维护第二份
生产阶段的有序数组或固定阶段编号；说明目录按名称排序，不参与执行顺序。

中英文工具文档删除已过时的 ParseValidate/Trace-before-PostHook 阶段图，
说明执行前和结算后的责任，并链接到可执行合同。文档中的 builder 示例也
恢复为当前 `.model(...).build()?` API。

## 来源与范围

真实阶段注册和执行仍由 `src/agent/react/run/pipeline.rs` 唯一拥有。learning
测试只消费已有运行时观察元数据，不增加框架公开 introspection API，不复制
生产阶段顺序。独立复审已通过；完整门禁和 main 交付仍待验证。

## 已知缺口

此测试观测默认管线的一次成功工具调用，不覆盖自定义管线或所有执行前
阻断分支。语义摘要已按本候选的整合源码快照刷新；合并前如 main 再推进，
须重新对账并刷新。
