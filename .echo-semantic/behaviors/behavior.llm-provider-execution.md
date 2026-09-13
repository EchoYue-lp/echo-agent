---
schema_version: 1
id: behavior.llm-provider-execution
kind: behavior
status: needs_review
expectation: inferred
risk: high
primary_focus: failure_concurrency
focus: [trigger_input, contract_evidence, time_lifecycle, result_side_effect]
boundary: boundary.llm-provider-runtime
observed_at: source:8b3972e1d2bc92f4ad59f511b6674eaaf243c1760f21973caf9e96558f71db90
code_refs: [echo-core/src/llm/mod.rs, echo-core/src/llm/capabilities.rs, echo-core/src/budget.rs, echo-core/src/tokenizer.rs, echo-integration/src/providers/client.rs, echo-integration/src/providers/openai.rs, echo-integration/src/providers/anthropic.rs, echo-integration/src/providers/responses.rs]
rule_refs: [rule.provider-protocol-boundary]
evidence_refs: [evidence.provider-protocol-quality]
finding_refs: [finding.structured-output-main-path, finding.provider-capability-authority, finding.nonstream-cancellation-parity]
---

# LLM 与 Provider 执行

## 重要承诺

Provider adapter 只负责 wire translation；model profile、token budget、timeout、stream terminal 与 structured output 使用 typed framework contract。

## 当前行为

LlmClient 接受 ChatRequest 并返回 response/stream；OpenAI Chat, Responses 与 Anthropic adapters 映射各协议，ModelProfileResolver 和 tokenizer/budget 决定 harness 行为。

## 期望行为

外部 stream 截断、malformed chunk、first/idle/overall timeout、tool delta、usage 和 finish reason 不得被静默解释为完成。

## 触发、结果与副作用

Agent think/grade/summary 等路径发出网络请求并消费 tokens；结果进入 AgentEvent、usage、trace 或 typed structured value。

## 失败、重试与恢复

连接失败、rate limit、流中断、超时和 provider capability mismatch 需保持 typed error/retry boundary，不能由 adapter 发明业务终态。

## 证据

LLM contracts、provider implementations、ADR 0022 和 provider fixture/tests 提供当前证据。

## 裁决记录

快速变化的模型事实由消费方显式配置；全 provider failure matrix 留待高风险 audit。
