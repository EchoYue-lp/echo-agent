---
schema_version: 1
id: evidence.model-facts-authority-verification
kind: evidence
observed_at: source:983de91986bfb711ff3bf6586ef34bdcfad12189905d4a6a8551ddea697f82f9
source_refs:
  - echo-core/src/llm/capabilities.rs
  - echo-core/src/llm/mod.rs
  - echo-integration/src/providers/config.rs
  - echo-integration/src/providers/mod.rs
  - echo-integration/src/providers/openai.rs
  - echo-integration/src/providers/responses.rs
  - echo-integration/src/providers/anthropic.rs
  - src/config.rs
  - src/lib.rs
  - src/llm.rs
  - src/agent/react/builder.rs
  - src/agent/react/mod.rs
  - src/agent/react/run/phases/prepare.rs
  - src/agent/snapshot.rs
  - docs/en/38-factory-modes.md
  - docs/zh/38-factory-modes.md
supports: [behavior.llm-provider-execution, rule.provider-protocol-boundary]
limitations:
  - 完整workspace all-feature门禁、SDK contract生成与远端CI按父任务要求留到集成分支
  - shared semantic baseline和全局source digest未在并行worktree刷新
---

# Model facts authority 验证证据

## 支持的结论

严格档 red test 先覆盖 ModelFact 类型、freshness API 和 conservative capability 的缺失路径；当前 worktree 已建立针对五层 precedence、source normalization、future/expired/empty、built-in observation time、历史 provider label compatibility、unknown provider/model isolation、Anthropic custom-label capabilities、capability/profile 不变量、tokenizer receipt、serde、protocol baseline 和 runtime snapshot freshness 的 focused 覆盖。tokenizer dispatch 本身不在本切片范围，留给 #100。

`echo_integration providers::` 的 focused 覆盖包含 SourcedLlmConfig roundtrip、legacy LlmConfig decode、fresh/stale thinking 和 budget facts、partial provider 与三个 adapter 固定 protocol baseline 及 provider label 隔离。root config 和 snapshot/profile focused tests 覆盖 SourcedModelConfig、legacy context provenance、过期窗口降级、跨 identity 隔离、caller 重放、RuntimeConfig budget 重建、prepare safe point 和 Tool policy；这些结果需要在最新修复（含 Anthropic adapter 与显式 flag precedence）后重新取得。

`cargo fmt --all -- --check` 与 `git diff --check` 在当前改动后通过；受影响 package 的 focused Clippy 仍待空间恢复后重新执行。

## 来源与范围

命令receipt由本轮执行器捕获并在父任务handoff中逐项报告；验证仅覆盖本切片受影响packages和行为，不替代父任务合并到main或发起MR前的完整门禁。

## 已知缺口

独立rereview单独记录；并行分支合并后必须统一刷新semantic baseline/source引用并生成SDK contract artifacts，再执行完整门禁。
