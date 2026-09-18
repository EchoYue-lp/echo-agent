---
schema_version: 1
id: evidence.subagent-definition-catalog-repair
kind: evidence
observed_at: d0ff62ec8b17e847d6abc57837327321297c39c6
source_refs:
  - src/agent/subagent/registry.rs
  - src/agent/react/capabilities.rs
  - src/tools/builtin/agent_dispatch.rs
  - docs/en/26-multi-agent.md
  - docs/zh/26-multi-agent.md
  - docs/adr/0033-subagent-factory-singleflight-publication.md
supports: [behavior.task-subagent-execution, rule.task-subagent-authority]
limitations:
  - 当前修复只统一definition-only文档与可执行catalog合同，不更改注册和调度逻辑
  - 独立复审、完整合并门禁与远端main交付仍需后续完成
---

# Definition-only Subagent catalog 修复证据

## 支持的结论

`SubagentRegistry` 的单一 `RegistryState` 保留未装配定义，`get` 与 `contains` 可读取；
`publish_catalog`、`list_available`、`list_by_tag` 和 `agent_names` 只枚举绑定实例或
factory 的定义。`AgentDispatchTool` 读取同一 executable catalog 和 revision，因此
definition-only 条目不进入模型可见候选。`ReactAgent` 的公开说明及双语正式文档现与
该生产路径一致；同名装配后进入候选，移除后退出。

## 来源与范围

修复提交 `9e9b0191220aa0899fc1e8849ff8745ee78abe63` 基于远端 main
`0415ba15eb8d348f357fe55df4448897677e6960`。复用现有 registry 与 dispatch
schema 缓存，不新增 API、状态权威或依赖。ADR 0033 补充其既有 executable catalog
决策，registry 与工具 schema 测试补充真实过滤和阶段切换覆盖。

## 已知缺口

显式程序化调用仍可尝试未装配的名字，并按原合同在执行解析时失败；这个低层入口不
等同于模型可见可执行列表。此证据不替代独立复审、完整合并门禁或远端关闭条件。
