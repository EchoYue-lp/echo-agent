---
schema_version: 1
id: evidence.provider-stream-terminal-parity-verification
kind: evidence
observed_at: 2f4da65cd5b83daa07d6c0f36d47bb50fd259c91
source_refs:
  - echo-integration/src/providers/client.rs
  - echo-integration/src/providers/anthropic.rs
  - echo-integration/src/providers/responses.rs
  - docs/en/10-streaming.md
  - docs/zh/10-streaming.md
supports: [behavior.llm-provider-execution, rule.provider-protocol-boundary]
limitations:
  - 本地HTTP/SSE fixture不替代真实provider网络验收
  - 完整workspace门禁、语义baseline刷新和独立rereview由集成分支完成
---

# Provider stream语义终态定向验证

## 支持的结论

真实`LlmClient::chat_stream`入口使用本地HTTP/SSE fixture验证三个adapter。正常完成
的流各发布一次finish和usage；部分delta后缺终态、Chat无finish的`[DONE]`、Chat
非成功reason或finish后继续输出、Anthropic缺`message_delta`/`message_stop`或
`max_tokens`、Responses缺`response.completed`均返回typed `InvalidResponse`。
OpenAI在无`[DONE]`时不提前发布usage或finish。

## 来源与范围

验证目标为本worktree的provider源码和`LlmClient::chat_stream`生产入口；fixture对
`OpenAiClient`、`ResponsesClient`、`AnthropicClient`分别构造真实本地HTTP请求并
完整消费`ChatChunk`流。

## 命令与结果

- 失败基线：新增fixture对旧实现运行`cargo test -p echo_integration
  provider_streams_require_semantic_completion_after_partial_output --locked`，1项失败，
  报告`openai accepted missing semantic terminal`。
- 修改后：`cargo fmt --all -- --check`退出码0；
  `cargo test -p echo_integration providers:: --locked`退出码0，84 passed/0 failed。

## 已知缺口

定向测试不证明合并最新main后的全量Cargo矩阵、远端CI或真实provider端点。
