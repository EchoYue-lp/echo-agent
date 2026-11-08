---
schema_version: 1
id: rule.provider-protocol-boundary
kind: rule
status: needs_review
expectation: inferred
risk: high
primary_focus: contract_evidence
focus: [failure_concurrency, time_lifecycle, trigger_input]
observed_at: source:9aa69fb2ec816b1cb98a941dda9126f1ed8992e16e45d67a9a374411ce934448
behavior_refs: [behavior.llm-provider-execution]
code_refs: [echo-core/src/llm/mod.rs, echo-core/src/llm/capabilities.rs, echo-integration/src/providers/config.rs, echo-integration/src/providers/client.rs, echo-integration/src/providers/anthropic.rs, echo-integration/src/providers/responses.rs, docs/adr/0022-typed-llm-timeouts.md, docs/adr/0064-provider-stream-semantic-terminal.md, src/agent/react/mod.rs, src/agent/react/capabilities.rs, echo-integration/src/providers/thinking_translate.rs, echo-integration/src/providers/anthropic_compatibility_tests.rs]
evidence_refs: [evidence.provider-protocol-quality, evidence.sse-eof-framing-acceptance-repair, evidence.sse-eof-framing-acceptance-verification, evidence.provider-stream-terminal-parity-repair, evidence.provider-stream-terminal-parity-verification, evidence.model-facts-authority-repair, evidence.model-facts-authority-verification]
finding_refs: [finding.structured-output-main-path, finding.provider-capability-authority, finding.nonstream-cancellation-parity, finding.sse-eof-framing-acceptance, finding.provider-stream-terminal-parity, finding.runtime-model-client-publication]
---

# Provider Wire 与 Harness Policy 分离

## 不变量或唯一权威

LlmClient/ChatRequest 定义 provider-neutral 合同，provider adapter 只翻译 wire；ModelProfile、budget、tokenizer 与 timeout 决定 harness policy。

## 适用行为

适用于 OpenAI Chat, Responses, Anthropic 及自定义 LlmClient 的 non-stream/stream 请求、tool calling、structured output 和 usage。

## 当前实现

Typed config构造concrete client，共享SSE transport处理stream并拒绝未以空行完成framing的EOF残余；各adapter各自确认协议语义终态后才发布finish/usage。ModelProfileResolver是独立可注入policy。

## 期望行为

Concrete provider 必须准确声明 capabilities；config/profile 不应并行产生冲突策略；取消与 terminal semantics 在 provider 间对等。

## 证据

LLM traits、provider configs/implementations、SSE tests 与 ADR 0022 提供部分证据。

## 裁决记录

Provider capabilities、structured output 主路径和 non-stream cancellation 已形成待审 Finding。

## Runtime installation invariant

A successful model setter changes the concrete client used by the next invocation,
not only its descriptive model name. Build/identity/profile work precedes one
private frozen-value publisher; rejected changes and dropped preparation leave
all installed fields unchanged. Opaque custom clients require explicit replacement
configuration for a different model and cannot reuse stale provider credentials.
Prepared commit performs no client trait queries or provider resolution. Existing
borrowed-string wrapper snapshots are not a live Session model authority.

## 当前原生观察的实际边界

当前同一Message／Delta的D1固定记录只运输真实响应观察；D2保不完整原件、原失败首因，
不拥有执行终态。原生Source仅实际供应商ID，canonical内部块身份可独立稳定；晚Source
按原分组单向补全，foreign／D1 owner冲突由adapter拒绝，正文和参数不重发。
thinking_translate继续只翻译现协议配置；新增兼容测试是实际loopback准入和无损回放消费者。
Root普通D1须到原adapter校验后才从HTTP wire剥离，失败metadata-only整条排除。
新版本已测的具体函数／源和完整矩阵状态各自记录，本Rule的needs_review不代表全图已闭合。
