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
observed_at: 2f4da65cd5b83daa07d6c0f36d47bb50fd259c91
code_refs: [echo-core/src/llm/mod.rs, echo-core/src/llm/capabilities.rs, echo-core/src/budget.rs, echo-core/src/tokenizer.rs, echo-integration/src/providers/client.rs, echo-integration/src/providers/openai.rs, echo-integration/src/providers/anthropic.rs, echo-integration/src/providers/responses.rs, src/agent/react/capabilities.rs, src/agent/react/mod.rs, src/agent/react/builder.rs, src/agent/react/tests.rs, src/agent/snapshot.rs]
rule_refs: [rule.provider-protocol-boundary]
evidence_refs: [evidence.provider-protocol-quality, evidence.sse-eof-framing-acceptance-repair, evidence.sse-eof-framing-acceptance-verification, evidence.provider-stream-terminal-parity-repair, evidence.provider-stream-terminal-parity-verification, evidence.model-facts-authority-repair, evidence.model-facts-authority-verification]
finding_refs: [finding.structured-output-main-path, finding.provider-capability-authority, finding.nonstream-cancellation-parity, finding.sse-eof-framing-acceptance, finding.provider-stream-terminal-parity, finding.runtime-model-client-publication]
---

# LLM 与 Provider 执行

## 重要承诺

Provider adapter 只负责 wire translation；model profile、token budget、timeout、stream terminal 与 structured output 使用 typed framework contract。

## 当前行为

LlmClient接受ChatRequest并返回response/stream；OpenAI Chat、Responses与Anthropic adapters映射各协议并共享严格SSE framing，delimiterless EOF残余不会形成事件。Chat成功finish+`[DONE]`、Responses `response.completed`和Anthropic成功`message_delta`+`message_stop`是各自语义终态，之前的finish/usage不得声称完成。ModelProfileResolver和tokenizer/budget决定harness行为。

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

## Runtime model publication

The configured model, concrete client and resolved model facts publish as one
prepared generation. Different-model changes build and resolve first; failure
preserves the installed generation and sampling. Custom client injection clears
stale rebuild configuration. Prepared commit cannot call arbitrary client traits,
parse configuration or resolve facts; ordinary run snapshots retain their own
freshness safe point. The runtime-publication Finding tracks the actual HTTP
counterexample and its repair independently from historical freshness Findings.

## Explicit configured Builder construction

The Builder now exposes the same original-client/original-config construction
mechanism as the direct constructor. It prepares one validated profile before
Context creation, retains the exact Arc, and preserves full Builder subsystem
injection. Its private pair input is consumed during construction; the live
Agent remains the only client/config authority. Ordinary setter chains keep
config-wins behavior; later ordinary setters discard an explicit pair, and
runtime opaque replacement still clears rebuild configuration.

The bounded source cut and qualification limits are recorded by
evidence.configured-client-construction. This addition does not promote this
broad needs_review Behavior, provider frontiers, SDK pin or language contracts.
