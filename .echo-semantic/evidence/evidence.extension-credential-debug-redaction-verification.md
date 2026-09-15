---
schema_version: 1
id: evidence.extension-credential-debug-redaction-verification
kind: evidence
observed_at: 98a2e11cfb6e88e2f310ae2c2b40cd9e009534a4
source_refs:
  - echo-integration/src/redaction.rs
  - echo-integration/src/mcp/transport/mod.rs
  - echo-integration/src/mcp/server_config.rs
  - echo-integration/src/mcp/config_loader.rs
  - echo-integration/src/channels/channels/qq/channel.rs
  - echo-integration/src/channels/channels/feishu/channel.rs
  - docs/en/08-mcp.md
  - docs/zh/08-mcp.md
  - docs/en/15-im-channels.md
  - docs/zh/15-im-channels.md
supports: [behavior.extension-publication, rule.extension-generation-authority]
limitations:
  - 未运行完整 workspace 合并门禁，按用户要求留到发起 MR 或合入 main 前统一执行
  - 未连接真实 QQ、Feishu 或外部 MCP server；验证覆盖可确定的 formatting、error 和 feature 编译合同
  - echo-website 存在用户未提交改动且此前明确延后同步，本切片未修改该独立仓库
---

# Extension credential diagnostic redaction 验证证据

## 支持的结论

修复前四个 focused Debug tests 稳定暴露 MCP env/header、QQ client secret 和 Feishu webhook credential。修复后测试覆盖：配置 Debug、嵌套 JSON、exact opaque secret、重叠 secret、跨 16,384 字符 retention 边界、无效 URL fail-closed、真实 reqwest status error 附带 URL、Authorization scheme/payload，以及 MCP JSON-RPC error message/data。

最终实现快照执行 `cargo test -p echo_integration --features mcp,channels --lib`，150 项全部通过。独立 reviewer 另执行 `cargo test -p echo_integration --all-features --locked`，160 项通过、1 项 ignored，11 项 doctest 通过；`cargo clippy -p echo_integration --all-targets --all-features --locked -- -D warnings`、`cargo check -p echo_integration --no-default-features --locked`、`cargo fmt --all -- --check` 与 `git diff --check` 均退出码 0。

## 来源与范围

测试位于 integration helper、MCP transport/config 和 QQ/Feishu config 同模块；双语 MCP 与 channel 文档说明脱敏仅作用于 Debug/diagnostic，不改变发送到用户选择 extension 的值。

## 已知缺口

完整 workspace、逐 feature 与远端 CI 将由最终整合分支按仓库合并门禁执行；本证据只证明 #56 的 focused 行为边界。
