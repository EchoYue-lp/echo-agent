---
schema_version: 1
id: evidence.subagent-definition-catalog-verification
kind: evidence
observed_at: d0ff62ec8b17e847d6abc57837327321297c39c6
source_refs:
  - src/agent/subagent/registry.rs
  - src/tools/builtin/agent_dispatch.rs
  - docs/adr/0033-subagent-factory-singleflight-publication.md
supports: [behavior.task-subagent-execution, rule.task-subagent-authority]
limitations:
  - 本证据只覆盖macOS本地subagent feature的focused tests与格式检查
  - 完整workspace/feature矩阵、远端CI、独立复审和合并后的semantic strict仍待执行
---

# Definition-only Subagent catalog 验证证据

## 支持的结论

`cargo test -p echo_agent --features subagent --lib cached_schema_tracks_shared_registry_revision --locked`
运行 1 个测试且通过：definition-only 不进入 `agent_tool` enum/description，装配
实例后进入，移除后退出。
`cargo test -p echo_agent --features subagent --lib agent::subagent::registry::tests --locked`
运行 15 个测试且全部通过，包含 pending
definition 的 `get`/`contains`、`list_available`/`list_by_tag`/`agent_names`/catalog
过滤、同名装配、factory 解析与 remove。`cargo fmt --all -- --check` 和
`git diff --check` 退出 0。

## 来源与范围

以上测试与格式检查在修复提交 `9e9b0191220aa0899fc1e8849ff8745ee78abe63`
对应源码上执行。最初未启用 `subagent` feature 的命令虽退出 0，却运行 0 个目标
测试，不作为本验证结论。`examples` 没有对应的 definition-only catalog 示例；
官网不展示框架内部注册合同，无需同步修改。

## 已知缺口

语义 strict snapshot 目前受主线 `#134` 三个历史 `source:` 引用与新增源码摘要不一致
影响，仍需在集成基线上锚定其可恢复 Git revision、刷新 baseline 并重验。
