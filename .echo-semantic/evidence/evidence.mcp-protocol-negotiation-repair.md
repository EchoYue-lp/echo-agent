---
schema_version: 1
id: evidence.mcp-protocol-negotiation-repair
kind: evidence
observed_at: eadf1a3d5a498bdbccd3742a7e1a457cb27b172d
source_refs:
  - echo-integration/src/mcp/client.rs
  - echo-integration/src/mcp/types.rs
  - docs/en/08-mcp.md
  - docs/zh/08-mcp.md
supports: [behavior.extension-publication, rule.extension-generation-authority]
limitations:
  - 本修复验证 initialize 响应的协议选择，不改变 server-side 版本协商或 MCP capability 模型
  - 未知版本复用现有 typed McpError::InitializationFailed 类别，具体版本与支持矩阵保留在错误消息中
  - 完整 workspace 门禁、远端 CI 和主线语义快照刷新由集成分支统一执行
---

# MCP protocol negotiation 修复证据

## 支持的结论

`SUPPORTED_PROTOCOL_VERSIONS` 是 client 与 server 共享的四版本权威集合。Client 反序列化
initialize 响应后、发送 `notifications/initialized` 与读取 server capabilities 前，验证
server 选择的 `protocolVersion` 属于该集合。未知版本返回
`ReactError::Mcp(McpError::InitializationFailed)`；`McpClient::from_transport` 既有失败结算
负责关闭 transport，因此不发布半初始化 client。

英文与中文 MCP 文档同步记录四个支持版本、client 验证时点、未知选择的 typed failure 与
transport 关闭行为。

## 来源与范围

修复扩展既有 `McpClient::from_transport`、`initialize_with_transport` 和共享版本常量，没有
新增 protocol registry、第二份版本列表或并行协商状态机。

## 已知缺口

本证据不声明第三方 server 对具体 MCP feature 的语义兼容性；只证明 initialize 版本选择受
共享支持集合约束。
