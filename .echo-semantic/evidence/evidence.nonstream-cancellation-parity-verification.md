---
schema_version: 1
id: evidence.nonstream-cancellation-parity-verification
kind: evidence
observed_at: 13a6a742a4eec2dcc971945d9887662310c3ce43
source_refs:
  - echo-integration/src/providers/client.rs
  - echo-integration/src/providers/openai.rs
  - echo-integration/src/providers/responses.rs
  - echo-integration/src/providers/anthropic.rs
supports: [behavior.llm-provider-execution, rule.provider-protocol-boundary]
limitations:
  - 完整workspace门禁与远端CI留到汇总MR前执行
  - 测试使用本地stalled HTTP server而非真实provider网络
---

# Non-stream provider cancellation验证证据

## 支持的结论

三provider乘以headers前/后两阶段的stalled-server矩阵通过；decode-start cancellation与
Anthropic非法UTF-8反例通过。Provider client模块13项、echo_integration all-feature
152项通过、1项live test ignored；warnings Clippy、panic-policy Clippy、fmt与diff check
全部通过。

## 来源与范围

测试直接调用三种真实`LlmClient::chat`路径，不以helper单测替代provider接线验证。
Cancellation future返回前还会再次检查token，避免传输或projection与取消同时ready时
错误选择成功。

## 已知缺口

未运行完整workspace/all-language SDK门禁；按用户要求统一在发起MR前运行。
