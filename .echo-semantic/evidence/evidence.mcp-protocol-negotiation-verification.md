---
{
  "schema_version": 1,
  "id": "evidence.mcp-protocol-negotiation-verification",
  "kind": "evidence",
  "observed_at": "7110a27e7b0e9841dcfa0d4cca1cdbc053d96177",
  "source_refs": [
    "echo-integration/src/mcp/client.rs",
    "echo-integration/src/mcp/types.rs",
    "docs/en/08-mcp.md",
    "docs/zh/08-mcp.md"
  ],
  "supports": [
    "behavior.extension-publication",
    "rule.extension-generation-authority"
  ],
  "limitations": [
    "Current7110 native observation only; original unavailable history, old executions and old reviews are not reconstructed or reexecuted.",
    "Current source/test/limit mapping is in evidence.framework-history-current-reobservation; full05 is existing execution, not a newly run historical command."
  ]
}
---

# Current observation and historical record

The machine header now records only the bounded native observation at `7110a27e7b0e9841dcfa0d4cca1cdbc053d96177`; current source/tests and limitations are in `evidence.framework-history-current-reobservation`. Original observation `eadf1a3d5a498bdbccd3742a7e1a457cb27b172d` and its complete metadata remain in [the immutable875 document](https://github.com/EchoYue-lp/echo-agent/blob/875cc22f2e99c7ccd83c07e02c03fe221c5e3f6c/.echo-semantic/evidence/evidence.mcp-protocol-negotiation-verification.md). The original execution counts, before/after claims and review prose below are preserved verbatim as historical statements, not rerun or equivalence claims for7110.


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
