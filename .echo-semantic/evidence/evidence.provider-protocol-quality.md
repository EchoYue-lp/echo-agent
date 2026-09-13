---
schema_version: 1
id: evidence.provider-protocol-quality
kind: evidence
observed_at: source:8b3972e1d2bc92f4ad59f511b6674eaaf243c1760f21973caf9e96558f71db90
source_refs:
  - echo-core/src/llm/mod.rs
  - echo-core/src/llm/capabilities.rs
  - echo-core/src/budget.rs
  - echo-core/src/tokenizer.rs
  - echo-integration/src/providers/client.rs
  - echo-integration/src/providers/openai.rs
  - echo-integration/src/providers/anthropic.rs
  - echo-integration/src/providers/responses.rs
  - src/acp/adapter.rs
  - src/acp/runtime.rs
  - src/a2a/server.rs
  - echo-integration/src/channels/manager.rs
  - src/channels.rs
  - src/headless.rs
  - echo-sdk-protocol/src/lib.rs
  - echo-sdk-host/src/lib.rs
  - echo-sdk-host/tests/core_profile_e2e.rs
  - src/eval/runner.rs
  - src/eval/replay.rs
  - src/improve/mod.rs
  - src/improve/loop.rs
  - src/improve/eval_improvement.rs
  - src/improve/trajectory.rs
  - src/evolution/mod.rs
  - src/evolution/background_review.rs
  - src/evolution/dreaming.rs
  - src/evolution/layer.rs
  - src/evolution/audit.rs
  - src/evolution/curator.rs
  - src/evolution/draft.rs
  - src/evolution/merge.rs
  - src/evolution/patch.rs
  - src/evolution/review.rs
  - src/evolution/security.rs
  - docs/adr/0022-typed-llm-timeouts.md
  - docs/adr/0028-source-first-multilanguage-sdk-runtime.md
  - docs/adr/0031-sdk-identity-governance-scope.md
supports: [behavior.llm-provider-execution, behavior.protocol-projection, behavior.eval-evolution, rule.provider-protocol-boundary, rule.protocol-role-separation, rule.quality-observation-boundary]
limitations:
  - Provider 模型事实会随外部服务变化；SDK intrinsic backlog 与产品 UI/backend 行为不是本 Evidence 的闭合目标
---

# Provider、Protocol 与 Quality 证据

## 支持的结论

LLM provider adapter、模型能力/预算/超时构成 typed framework 边界；ACP、A2A、Channels、Headless 和 SDK Host 是不同入口投影；Trace/Eval/Improve 与分层 Evolution 能力的已知行为和反例均可从本 Evidence 复核。

## 来源与范围

来源覆盖 LLM traits/capabilities/budget/tokenizer/provider 实现、ACP/A2A/Channel/Headless、SDK protocol/Host、Eval/Improve，以及 Background Review/Dreaming、memory、Skill lifecycle、安全检查等 Evolution 入口与相关 ADR/E2E。

## 已知缺口

外部 provider 的当前限制必须由消费方更新；全语言 intrinsic parity、发布和产品 surface 不纳入全 workspace baseline 完成条件。
