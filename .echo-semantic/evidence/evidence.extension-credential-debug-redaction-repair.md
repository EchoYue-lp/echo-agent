---
schema_version: 1
id: evidence.extension-credential-debug-redaction-repair
kind: evidence
observed_at: 98a2e11cfb6e88e2f310ae2c2b40cd9e009534a4
source_refs:
  - echo-integration/src/redaction.rs
  - echo-integration/src/mcp/server_config.rs
  - echo-integration/src/mcp/config_loader.rs
  - echo-integration/src/mcp/client.rs
  - echo-integration/src/mcp/transport/mod.rs
  - echo-integration/src/mcp/transport/http.rs
  - echo-integration/src/mcp/transport/sse.rs
  - echo-integration/src/mcp/transport/stdio.rs
  - echo-integration/src/channels/channels/qq/channel.rs
  - echo-integration/src/channels/channels/qq/api.rs
  - echo-integration/src/channels/channels/qq/gateway.rs
  - echo-integration/src/channels/channels/feishu/channel.rs
  - echo-integration/src/channels/channels/feishu/api.rs
  - echo-integration/src/channels/channels/feishu/long_poll.rs
  - echo-integration/src/channels/channels/feishu/webhook.rs
supports: [behavior.extension-publication, rule.extension-generation-authority]
limitations:
  - 仅约束框架拥有的配置 Debug、transport 日志与返回错误；embedding application 自行序列化原始配置仍由调用方负责
  - 不改变 extension 连接、权限判定、请求值或全局 Trace/Audit retention 合同
---

# Extension credential diagnostic redaction 修复证据

## 支持的结论

`echo-integration` 现在由一个 feature-gated helper 统一处理 diagnostic redaction。MCP stdio args/env、HTTP/SSE headers 和 URL credential 不进入公开配置 Debug；QQ client secret 与 Feishu app secret、verification token、signing key 使用手写 Debug 显示固定占位符。

原始 transport 文本先按当前配置的 exact secret 做 UTF-8 字符级最长单次匹配，再应用既有 `ContentRetentionPolicy` 的 shape redaction 与有界保留。Authorization 同时登记完整 header 和 scheme 后 payload；reqwest error 先移除附带 URL，再只附回脱敏 URL。HTTP session id、SSE event id、Feishu connection id 只记录 presence，不记录值。MCP JSON-RPC error message/data 在 transport 边界完成相同处理，成功 result 不被修改。

## 来源与范围

修复只修改 `echo-integration` 的 MCP、QQ、Feishu 适配边界和对应双语文档。曾尝试扩展共享 retention 的普通 `session_id` 规则，但独立复审发现会破坏 Trace/Audit session 查询后已完整撤销；最终 revision 不含任何 `echo-core` 变化。

## 已知缺口

本证据不宣称关闭 `finding.trace-audit-secret-boundary`，不审计第三方 tracing subscriber，也不阻止用户显式访问或序列化其自行提供的原始配置。
