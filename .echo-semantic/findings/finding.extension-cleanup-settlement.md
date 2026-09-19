---
schema_version: 1
id: finding.extension-cleanup-settlement
kind: finding
type: implementation_bug
status: open
severity: high
primary_focus: failure_concurrency
focus: [time_lifecycle, result_side_effect, state_authority]
boundary_ref: boundary.extension-lifecycle
behavior_refs: [behavior.extension-publication]
rule_refs: [rule.extension-generation-authority]
evidence_refs: [evidence.effects-extensions, evidence.extension-cleanup-settlement-repair, evidence.extension-cleanup-settlement-verification]
audit_refs: [audit.extension-lifecycle.time-lifecycle, audit.extension-cleanup-settlement-rereview]
decision_refs: [decision-adr-0049-mcp-transport-close-settlement]
repair_evidence_refs: [evidence.extension-cleanup-settlement-repair]
verification_evidence_refs: [evidence.extension-cleanup-settlement-verification]
rereview_audit_refs: [audit.extension-cleanup-settlement-rereview]
discovered_at: f1e9027246760661144786e9e35615cd46d580c6
---

# MCP SSE 与 SDK LSP cleanup 未等待结算

## 外部 Issue

GitHub Issue: https://github.com/EchoYue-lp/echo-agent/issues/55

## 问题

SSE endpoint/POST/timeout 失败可遗留 pending sender，close 只 cancel 不 drain/await；SDK Host LSP close 清 Arc map 而未调用 manager shutdown_all。

## 触发条件与影响

Transport 建立失败、连接关闭或 Host shutdown 时，pending request/child process 可能在终态后继续存在或只依赖 Drop。

## 证据

`echo-integration/src/mcp/transport/sse.rs` 与 `echo-sdk-host/src/core_profile/facade/integrations.rs` 显示当前 close 路径。

## 修复范围（框架 vs SDK 所有权）

本仓库（echo-agent 框架）修复 MCP transport/client/manager 侧：`McpTransport::close`、
`McpClient::close`、`McpManager::disconnect`/`close_all` 传播 `Result`，SSE/stdio 排空
pending 并有界等待 owned task/child（详见 `docs/adr/0049-mcp-transport-close-settlement.md`）。
SDK Host 的 LSP resource-map close（原 `echo-sdk-host/src/core_profile/facade/integrations.rs`）
已随 SDK 抽取（ADR 0051）迁移到独立 `echo-agent-sdk` 仓库，由 main agent 在 SDK 侧处理；
本框架不拥有 SDK Host 代码，SDK 侧需要对其 Host facade 做同等 close `Result` 适配。

## 处理记录

Discovery 记录；修复与验证证据见 `evidence.extension-cleanup-settlement-repair.md` /
`evidence.extension-cleanup-settlement-verification.md`，复用干净修复提交 dbbfff27
（`.worktrees/mcp-transport-close-settlement`）的框架部分，并保留 main 较新的 MCP
redaction/protocol/tool-classification 改动。
