---
schema_version: 1
id: evidence.extension-cleanup-settlement-verification
kind: evidence
observed_at: source:bc45db0f4d95af798280da901278b1f70c372807c5606c1281f4f92145c2dedd
source_refs:
  - echo-integration/src/mcp/transport/mod.rs
  - echo-integration/src/mcp/transport/sse.rs
  - echo-integration/src/mcp/transport/stdio.rs
  - echo-integration/src/mcp/mod.rs
  - docs/en/08-mcp.md
  - docs/zh/08-mcp.md
supports: [behavior.extension-publication, rule.extension-generation-authority]
limitations:
  - 完整 workspace 合并门禁和远端 CI 留给整合 MR
  - 未连接外部 MCP server；真实子进程与本地 HTTP fault server 覆盖 transport owner 和网络时序
---

# MCP transport cleanup settlement 验证证据

## 支持的结论

最终 focused MCP 集合 53 项通过，覆盖 pending registration drop、SSE endpoint/POST/response timeout、notification POST failure、close/notification 竞态、close Future cancellation、stdio EOF/read-error、response timeout、blocked writer、真实 child 优雅退出、强制 kill/reap、server stdout EOF、manager close failure retry、replacement failure、disconnect debt、caller-cancel 与并发 close。

最后一次 `cargo test -p echo_integration --all-features --locked` 为 171 passed、1 ignored，11 doctest 通过；`cargo clippy -p echo_integration --all-targets --all-features --locked -- -D warnings`、`cargo check -p echo_agent --features mcp --locked`、`cargo fmt --all -- --check` 和 `git diff --check` 均退出 0。

## 来源与范围

Fault tests 位于 transport 与 manager 同模块；真实 `/bin/sh` child 验证 close 返回前 process 已不可达，本地 TCP server 用 accept/read readiness 同步真实 notification POST 后再触发 close。上层 SDK adapter 不属于本 framework evidence 的验证范围。

## 已知缺口

未运行完整 workspace all-target/all-feature 门禁，遵循用户要求只在整合 MR/main 前执行；SDK inventory 与 semantic shared snapshot 由整合分支统一刷新。
