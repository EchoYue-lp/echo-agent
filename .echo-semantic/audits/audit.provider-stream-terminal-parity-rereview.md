---
schema_version: 1
id: audit.provider-stream-terminal-parity-rereview
kind: audit
boundary_ref: boundary.llm-provider-runtime
lens: failure_concurrency
freshness: examined
revision: 2f4da65cd5b83daa07d6c0f36d47bb50fd259c91
finding_refs: [finding.provider-stream-terminal-parity]
challenges:
  semantic-terminal-before-eof:
    revision: 2f4da65cd5b83daa07d6c0f36d47bb50fd259c91
    source_refs: [echo-integration/src/providers/client.rs, echo-integration/src/providers/anthropic.rs, echo-integration/src/providers/responses.rs]
    evidence_refs: [evidence.provider-stream-terminal-parity-repair, evidence.provider-stream-terminal-parity-verification]
  usage-and-finish-publication:
    revision: 2f4da65cd5b83daa07d6c0f36d47bb50fd259c91
    source_refs: [echo-integration/src/providers/client.rs, echo-integration/src/providers/anthropic.rs, src/agent/react/run/phases/think.rs]
    evidence_refs: [evidence.provider-stream-terminal-parity-verification]
---

# Provider stream semantic terminal 独立复审

## 审查范围

独立 reviewer 检查 Chat Completions、Anthropic Messages、Responses adapter、ReAct consumer、协议 fixture 和完整候选 diff。

## 已检查故障假设

检查 partial EOF、缺少成功 finish、finish 后额外 choice、缺少 Anthropic message_stop、未闭合 block、非成功 stop reason，以及 usage 或 finish reason 在语义终态前泄漏。

## 实际实现路径与证据

Chat adapter 暂存成功 finish 与 usage，只有 `[DONE]` 后发布；Anthropic adapter 暂存 message_delta，只有 message_stop 且 block 全部闭合后发布；Responses 继续要求 response.completed。ReAct 将 adapter error 结算为失败。84 项 provider focused tests 覆盖成功、截断和主要非法终态。

## 问题记录

复审未发现阻断项，结论 pass。

## 残余风险

真实 provider 网络验收和完整 workspace 门禁仍由交付分支执行；OpenAI 多 choice 不属于当前单 choice 请求合同。

## 未检查项

未检查远端 CI、真实供应商账号或网络故障注入。
