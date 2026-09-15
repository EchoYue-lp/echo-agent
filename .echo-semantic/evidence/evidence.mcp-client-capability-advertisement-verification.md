---
schema_version: 1
id: evidence.mcp-client-capability-advertisement-verification
kind: evidence
observed_at: f30a1fc05153832870c420d5415d436aadb8b07f
source_refs:
  - echo-integration/src/mcp/client.rs
  - echo-integration/src/mcp/server.rs
supports: [behavior.extension-publication, rule.extension-generation-authority]
limitations:
  - 完整workspace门禁与远端CI留到汇总MR前执行
  - 不连接公网第三方MCP server
---

# MCP client capability advertisement验证证据

## 支持的结论

Wire测试捕获真实initialize request并断言`capabilities == {}`；server测试覆盖四个支持版本
回显和未知版本拒绝。独立reviewer搜索其它initialize入口，未发现重新广告未实现能力的路径。

## 来源与范围

最终review结论pass，Critical、Important、Minor均为0；#67的client-side negotiated version
验证由独立Evidence覆盖。

## 已知缺口

不证明transport close或未来新增capability handler。
