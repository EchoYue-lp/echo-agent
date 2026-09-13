---
schema_version: 1
id: rule.provider-protocol-boundary
kind: rule
status: needs_review
expectation: inferred
risk: high
primary_focus: contract_evidence
focus: [failure_concurrency, time_lifecycle, trigger_input]
observed_at: f1e9027246760661144786e9e35615cd46d580c6
behavior_refs: [behavior.llm-provider-execution]
code_refs: [echo-core/src/llm/mod.rs, echo-core/src/llm/capabilities.rs, echo-integration/src/providers/config.rs, echo-integration/src/providers/client.rs, docs/adr/0022-typed-llm-timeouts.md]
evidence_refs: [evidence.provider-protocol-quality]
finding_refs: [finding.structured-output-main-path, finding.provider-capability-authority, finding.nonstream-cancellation-parity]
---

# Provider Wire 与 Harness Policy 分离

## 不变量或唯一权威

LlmClient/ChatRequest 定义 provider-neutral 合同，provider adapter 只翻译 wire；ModelProfile、budget、tokenizer 与 timeout 决定 harness policy。

## 适用行为

适用于 OpenAI Chat, Responses, Anthropic 及自定义 LlmClient 的 non-stream/stream 请求、tool calling、structured output 和 usage。

## 当前实现

Typed config 构造 concrete client，共享 SSE transport 处理 stream；ModelProfileResolver 是独立可注入 policy。

## 期望行为

Concrete provider 必须准确声明 capabilities；config/profile 不应并行产生冲突策略；取消与 terminal semantics 在 provider 间对等。

## 证据

LLM traits、provider configs/implementations、SSE tests 与 ADR 0022 提供部分证据。

## 裁决记录

Provider capabilities、structured output 主路径和 non-stream cancellation 已形成待审 Finding。
