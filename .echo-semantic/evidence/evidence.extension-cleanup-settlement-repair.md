---
schema_version: 1
id: evidence.extension-cleanup-settlement-repair
kind: evidence
observed_at: source:3d3fb558349d604e3762588a5db974417956f949c929c8d8cae30ab94558379b
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
  - Consumer public inventory is independently owned and does not block the framework Finding
  - Preparation and SSE construction cancellation still lack an externally awaitable cleanup owner
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

Transport close 的完整 workspace 门禁与远端 CI 已由后续 integration/main 交付。当前剩余缺口是
preparation/SSE construction Drop 派生的不可等待 cleanup owner；Plugin、LSP 与 Agent adapter
lifecycle 仍由各自 Finding 持有。
