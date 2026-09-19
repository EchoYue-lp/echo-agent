---
schema_version: 1
id: evidence.extension-cleanup-settlement-repair
kind: evidence
observed_at: source:b214951ece8e09325efc846ad7bd88a402135000e42fe67d2b917317b2d27923
source_refs:
  - echo-integration/src/mcp/transport/mod.rs
  - echo-integration/src/mcp/transport/sse.rs
  - echo-integration/src/mcp/transport/stdio.rs
  - echo-integration/src/mcp/client.rs
  - echo-integration/src/mcp/mod.rs
  - src/agent/react/mod.rs
  - src/plugin/prepared.rs
  - docs/adr/0049-mcp-transport-close-settlement.md
supports: [behavior.extension-publication, rule.extension-generation-authority]
limitations:
  - SDK public inventory and shared source digest are intentionally deferred to the integration branch
  - LSP runtime state and derived-handle lifecycle remain owned by their separate Findings
---

# MCP transport cleanup settlement 修复证据

## 支持的结论

`McpTransport`、`McpClient`、`McpManager` 与 framework Agent 显式 close 现在传播 `Result`。共享 pending registry 用同步 RAII registration guard 保证 endpoint、POST、write、flush、response timeout、channel close 和 caller cancellation 都移除准确请求；close 先 fence 新请求并失败全部 pending caller。

SSE receive、request POST 与 notification POST 观察同一个 cancellation token；close 等待 POST write gate、清空 endpoint，并通过 cancellation-safe single-flight receipt 等待 task。Stdio close 的单一 deadline 覆盖 stdin writer、child wait/kill/reap 与 stdout/stderr task；stdout EOF/read-error失败 pending，task 超时保留 JoinHandle 供后续 close 重试，不丢失其持有的 child owner。

Manager active client 与 cleanup debt 只在成功 close 后移除；失败保持在原 authority 中。Manager close gate 线性化并发 close，caller cancellation 后后续 close 可继续等待或重试。Replacement 在旧 transport close 失败时不发布新 target。

## 来源与范围

ADR 0049 绑定官方 MCP 2024-11-05 shutdown、官方 Rust SDK fallible graceful close 与官方 TypeScript SSE abort/close 做法。实现只修改 framework MCP transport/client/manager 及真实 close adapter，不新增 EKO 产品策略、MCP 连接权限门控或 LSP 状态。

## 已知缺口

完整 workspace 合并门禁、SDK inventory/source contract 刷新与远端 CI 由最终整合分支执行；本切片不关闭其它 Plugin、LSP 或 Agent adapter lifecycle Finding。
