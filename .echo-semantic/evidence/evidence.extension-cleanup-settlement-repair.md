---
schema_version: 1
id: evidence.extension-cleanup-settlement-repair
kind: evidence
observed_at: 733d352fc719f922b21bab1cd46206139564367f
source_refs:
  - echo-integration/src/mcp/transport/mod.rs
  - echo-integration/src/mcp/transport/sse.rs
  - echo-integration/src/mcp/transport/stdio.rs
  - echo-integration/src/mcp/client.rs
  - echo-integration/src/mcp/mod.rs
  - src/agent/react/mod.rs
  - src/plugin/prepared.rs
  - docs/adr/0049-mcp-transport-close-settlement.md
  - docs/en/08-mcp.md
  - docs/zh/08-mcp.md
supports: [behavior.extension-publication, rule.extension-generation-authority]
limitations:
  - Consumer public inventory is independently owned and does not block the framework Finding
  - 直接 consumer 若丢弃 preparation waiter，必须保留 scope 并显式 await close；进程强制退出不保证结算
  - LSP runtime state and derived-handle lifecycle remain owned by their separate Findings
---

# MCP transport cleanup settlement 修复证据

## 支持的结论

`McpTransport`、`McpClient`、`McpManager` 与 framework Agent 显式 close 现在传播 `Result`。共享 pending registry 用同步 RAII registration guard 保证 endpoint、POST、write、flush、response timeout、channel close 和 caller cancellation 都移除准确请求；close 先 fence 新请求并失败全部 pending caller。

SSE receive、request POST 与 notification POST 观察同一个 cancellation token；close 等待 POST write gate、清空 endpoint，并通过 cancellation-safe single-flight receipt 等待 task。Stdio close 的单一 deadline 覆盖 stdin writer、child wait/kill/reap 与 stdout/stderr task；stdout EOF/read-error失败 pending，task 超时保留 JoinHandle 供后续 close 重试，不丢失其持有的 child owner。

Manager active client 与 cleanup debt 只在成功 close 后移除；失败保持在原 authority 中。Manager close gate 线性化并发 close，caller cancellation 后后续 close 可继续等待或重试。Replacement 在旧 transport close 失败时不发布新 target。

本轮 `McpClient::prepare` 返回带 cloneable `McpPreparationScope` 的可等待 preparation；manager
在首个资源创建 poll 前登记 scope。scope 取消 admission、等待 build 交接，并在 close 失败时
保留同一 transport 供重试。`close_all` 等待 registered preparation，失败不删除 retry debt；
已取消的 preparation 不再发布 late client。SSE receive task 在 construction 暂停前转入
transport owner。transport close receipt 保留 task handle；runtime 中断后可重新发起 settlement。
stdio child 在等待 exit 期间留在 owner slot，后续 close 可继续 kill/reap。拓扑方法保留独占
`&mut self` 合同，避免 remove 超越尚未完成的连接握手。

## 来源与范围

ADR 0049 绑定官方 MCP 2024-11-05 shutdown、官方 Rust SDK fallible graceful close 与官方 TypeScript SSE abort/close 做法。实现只修改 framework MCP transport/client/manager 及真实 close adapter，不新增 EKO 产品策略、MCP 连接权限门控或 LSP 状态。

## 已知缺口

原 transport close 的完整 workspace 门禁与远端 CI 已由历史 integration/main 交付；本轮
construction 修复已在当前任务分支完成 focused 验证、合入最新 main 后的完整本地门禁与
17项独立feature检查；PR/CI与远端main交付尚未记录。
直接 consumer 必须保留 preparation scope 并等待关闭；Plugin、LSP 与 Agent adapter
lifecycle 仍由各自 Finding 持有。
