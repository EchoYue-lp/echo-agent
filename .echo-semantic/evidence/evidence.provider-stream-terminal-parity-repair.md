---
schema_version: 1
id: evidence.provider-stream-terminal-parity-repair
kind: evidence
observed_at: 2f4da65cd5b83daa07d6c0f36d47bb50fd259c91
source_refs:
  - echo-integration/src/providers/client.rs
  - echo-integration/src/providers/anthropic.rs
  - echo-integration/src/providers/responses.rs
  - docs/adr/0064-provider-stream-semantic-terminal.md
supports: [behavior.llm-provider-execution, rule.provider-protocol-boundary]
limitations:
  - 该候选尚未合入远端main且未完成独立rereview
  - 未改变Responses低层create_raw_stream的原始事件合同
---

# Provider stream语义终态修复候选

## 支持的结论

Chat Completions `stream_post` 在成功choice finish reason后仍要求`[DONE]`，期间暂存
finish/usage；缺失或非成功finish、无`[DONE]`的EOF返回typed `InvalidResponse`。
Anthropic在`message_delta`后暂存finish/usage，只有`message_stop`可释放成功终态；
此前的EOF、`[DONE]`、不完整block或非成功stop reason均失败。Responses已有的
`response.completed`权威保留在原adapter，没有新建第二套provider状态机。

## 来源与范围

修复仅修改`echo-integration` provider adapter内的SSE语义归约。共享`stream_json_sse`
仍负责framing、timeout和cancel；`LlmClient`/`ChatChunk`公共类型、EKO产品策略与
Responses raw events未改变。ADR 0064记录备选方案与兼容影响。

## 已知缺口

本证据不证明真实公网provider行为、全workspace gate或远端main交付。
