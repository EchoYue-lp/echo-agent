---
schema_version: 1
id: evidence.extension-cleanup-settlement-verification
kind: evidence
observed_at: 733d352fc719f922b21bab1cd46206139564367f
source_refs:
  - echo-integration/src/mcp/transport/mod.rs
  - echo-integration/src/mcp/transport/sse.rs
  - echo-integration/src/mcp/transport/stdio.rs
  - echo-integration/src/mcp/client.rs
  - echo-integration/src/mcp/mod.rs
  - docs/en/08-mcp.md
  - docs/zh/08-mcp.md
  - docs/adr/0049-mcp-transport-close-settlement.md
supports: [behavior.extension-publication, rule.extension-generation-authority]
limitations:
  - Transport close的完整workspace门禁与远端CI已由后续integration/main交付
  - 当前 focused 验证覆盖 retained preparation/construction owner；不保证进程强制退出前能完成 cleanup
  - 未连接外部 MCP server；真实子进程与本地 HTTP fault server 覆盖 transport owner 和网络时序
  - 本轮完整 post-main-merge 本地门禁与独立 feature 矩阵已通过；PR/CI 和远端 main 交付尚未记录
---

# MCP transport cleanup settlement 验证证据

## 支持的结论

最终 focused MCP 集合 53 项通过，覆盖 pending registration drop、SSE endpoint/POST/response timeout、notification POST failure、close/notification 竞态、close Future cancellation、stdio EOF/read-error、response timeout、blocked writer、真实 child 优雅退出、强制 kill/reap、server stdout EOF、manager close failure retry、replacement failure、disconnect debt、caller-cancel 与并发 close。

最后一次 `cargo test -p echo_integration --all-features --locked` 为 171 passed、1 ignored，11 doctest 通过；`cargo clippy -p echo_integration --all-targets --all-features --locked -- -D warnings`、`cargo check -p echo_agent --features mcp --locked`、`cargo fmt --all -- --check` 和 `git diff --check` 均退出 0。

## 来源与范围

Fault tests 位于 transport 与 manager 同模块；真实 `/bin/sh` child 验证 close 返回前 process 已不可达，
本地 TCP server 用 accept/read readiness 同步真实 notification POST 后再触发 close。Consumer adapter
不属于本 framework evidence 的验证范围。

## 已知缺口

本轮任务分支基于 `origin/main@4532b3bc`，实现提交 `30792546` 已合入该基线，
复审快照为本工作树。`cargo test -p echo_integration --lib --features mcp mcp:: --locked`
退出 0，110 passed、0 failed，覆盖 preparation 取消后 scope await/retry、cleanup 失败
retry owner、SSE construction 已启动 receive task 的 close、manager `close_all` 等待取消中
preparation、失败重试、prepared/preparing 重叠和 runtime 中断后的 close receipt 重试。
`cargo clippy -p echo_integration --lib --features mcp --locked -- -D warnings`、
`cargo check -p echo_agent --features mcp --locked`、`cargo fmt --all -- --check` 与
`git diff --check` 均退出 0。独立 reviewer 没有自行执行这些命令。

## 当前任务分支完整门禁

合入 `origin/main@4532b3bc` 并修正语义 baseline 的主线祖先锚点后，
`./scripts/verify.sh` 在最后一次源码修改后退出 0：fmt check、两轮 workspace
all-feature Clippy、workspace all-target/all-feature 测试与 workspace lib
no-default-features check 全部通过。公共 API 条件矩阵逐项编译 `acp a2a mcp lsp sqlite
telemetry topology subagent web media data statistics channels git database rag chart`，
17 项均退出 0。门禁运行使用共享 Cargo target、2 个 build jobs、关闭增量缓存及 dev/test
调试符号来控制磁盘占用；检查命令和 feature 组合未变。

首轮完整测试在 `echo_execution` 的 fake K8s 1 秒控制期限下偶发断言失败；单跑原反例
通过。测试夹具仅将该反例期限设为 5 秒，增加失败消息输出；生产 deadline 不变。
`cargo test -p echo_execution --lib --all-features --locked` 随后 325/325 通过，
完整门禁也在同一最终源码上重新执行为 0。旧失败没有记录实际错误正文，不能证明
一定由负载造成；独立测试改动复审通过，若再现可用新诊断确认分支。

PR CI 与远端 main 交付仍待执行。未连接真实第三方 MCP server，也未注入进程强制退出。
