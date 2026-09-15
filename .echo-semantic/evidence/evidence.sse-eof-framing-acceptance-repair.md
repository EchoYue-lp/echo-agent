---
schema_version: 1
id: evidence.sse-eof-framing-acceptance-repair
kind: evidence
observed_at: e842d87bb787fb0b1fd39afbc04f834df07aec84
source_refs:
  - echo-integration/src/providers/client.rs
supports: [behavior.llm-provider-execution, rule.provider-protocol-boundary]
limitations:
  - Provider-specific semantic terminal parity仍由finding.provider-stream-terminal-parity追踪
  - 本修复不改变provider wire payload或retry policy
---

# SSE EOF framing修复证据

## 支持的结论

共享`SseDecoder::finish`只有在buffer完全为空时接受正常EOF。连接在空行事件边界前
关闭时，剩余完整JSON、普通文本、纯空白或截断UTF-8都会返回typed
`InvalidResponse`，不再把delimiterless残余提升为已完成事件。

## 来源与范围

实现固定于commit `e842d87bb787fb0b1fd39afbc04f834df07aec84`。OpenAI Chat通过
`stream_post`，Responses和Anthropic通过同一`stream_json_sse`消费该decoder；没有为
单一provider创建平行framing逻辑。

## 已知缺口

本证据只证明SSE framing boundary，不证明各provider对`[DONE]`、finish reason或协议专属
terminal的语义对等。
