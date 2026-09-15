---
schema_version: 1
id: evidence.mcp-client-capability-advertisement-repair
kind: evidence
observed_at: f30a1fc05153832870c420d5415d436aadb8b07f
source_refs:
  - echo-integration/src/mcp/client.rs
  - echo-integration/src/mcp/server.rs
  - docs/en/08-mcp.md
  - docs/zh/08-mcp.md
supports: [behavior.extension-publication, rule.extension-generation-authority]
limitations:
  - 不实现roots、sampling或elicitation，只停止广告
  - MCP transport close settlement由finding.extension-cleanup-settlement独立追踪
---

# MCP client capability advertisement修复证据

## 支持的结论

MCP client唯一initialize入口使用空`ClientCapabilities::default()`；roots、sampling、
elicitation与experimental均不再广告。Framework没有对应server-to-client handler时，远端
server不会基于虚假协商发送这些request/notification。

MCP server继续对四个支持版本回显并拒绝未知版本，双语文档与当前兼容矩阵一致。

## 来源与范围

实现固定于commit `f30a1fc05153832870c420d5415d436aadb8b07f`，选择停止广告而非在
本切片引入三套未需要的client capability实现。

## 已知缺口

未来实现任一client capability时必须同时增加handler、协商测试和生命周期结算。
