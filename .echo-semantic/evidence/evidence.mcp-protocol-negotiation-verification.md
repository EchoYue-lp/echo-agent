---
schema_version: 1
id: evidence.mcp-protocol-negotiation-verification
kind: evidence
observed_at: eadf1a3d5a498bdbccd3742a7e1a457cb27b172d
source_refs:
  - echo-integration/src/mcp/client.rs
  - echo-integration/src/mcp/types.rs
  - docs/en/08-mcp.md
  - docs/zh/08-mcp.md
supports: [behavior.extension-publication, rule.extension-generation-authority]
limitations:
  - 未执行 echo-agent workspace 完整合并门禁或远端 CI
  - semantic strict snapshot 当前受共享 baseline/source digest 旧 revision 约束，由集成分支统一刷新
  - 测试使用确定性 transport fixture，不声明真实第三方 server 的网络可用性
---

# MCP protocol negotiation 验证证据

## 支持的结论

- `cargo test -p echo_integration --features mcp client_accepts_supported_versions_and_rejects_unknown_selection`：1 passed，0 failed。
- `cargo test -p echo_integration --features mcp mcp::client::tests`：3 passed，0 failed。
- 新测试逐个验证 `SUPPORTED_PROTOCOL_VERSIONS` 中的四个版本均被接受、发送 initialized 且不关闭 transport。
- 未知 `2099-01-01` 匹配 typed `McpError::InitializationFailed`，不发送 initialized 并关闭 transport。
- `cargo clippy -p echo_integration --features mcp,channels --lib --locked -- -D warnings`：通过。
- `cargo fmt --all -- --check` 与 `git diff --check`：通过。
- 独立 reviewer 对提交 `eadf1a3d` 复审，Critical、Important、Minor 均为 0，结论 PASS。

## 来源与范围

验证覆盖 shared 版本常量、initialize 响应反序列化、client 版本校验顺序、initialized 通知和
初始化失败 transport 结算；双语 MCP 文档与代码合同一致。

## 已知缺口

单独 `mcp` feature 的 Clippy 暴露 `echo-integration/src/redaction.rs` 既有 dead-code 警告；
加入该模块真实 consumer 的 `channels` feature 后 focused Clippy 通过。本切片未修改该无关
警告。完整 feature 矩阵和共享语义 digest 由集成分支处理。
