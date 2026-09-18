---
schema_version: 1
id: asset.llm-provider-runtime
kind: asset
title: LLM Provider 与 Harness Contract
asset_type: protocol
status: needs_review
risk: high
observed_at: 2f4da65cd5b83daa07d6c0f36d47bb50fd259c91
boundary_refs: [boundary.llm-provider-runtime]
code_refs: [echo-core/src/llm/mod.rs, echo-core/src/llm/capabilities.rs, echo-core/src/budget.rs, echo-core/src/tokenizer.rs, echo-integration/src/providers/config.rs, echo-integration/src/providers/client.rs]
consumer_refs: [src/agent/react/run/phases/think.rs, echo-state/src/compression/compressor/summary.rs]
behavior_refs: [behavior.llm-provider-execution]
rule_refs: [rule.provider-protocol-boundary]
evidence_refs: [evidence.provider-protocol-quality, evidence.provider-stream-terminal-parity-repair, evidence.provider-stream-terminal-parity-verification]
finding_refs: [finding.structured-output-main-path, finding.provider-capability-authority, finding.nonstream-cancellation-parity, finding.provider-stream-terminal-parity]
candidate_refs: []
---

# LLM Provider 与 Harness Contract

## 资产身份

Provider-neutral LlmClient/request types、wire adapters、ModelProfile、budget/tokenizer/timeout 的协议边界。

## 来源与消费者

ReactAgent think、compression/grader 和外部 consumers 使用。

## 生命周期

Build client、resolve profile/budget、request/stream、cancel/timeout/terminal、usage calibration。

## 候选关系

LlmConfig、client capabilities 与 ModelProfile 是需闭合的策略来源。

## 未知与限制

Structured output、capabilities 和 non-stream cancel 已形成 Findings。
