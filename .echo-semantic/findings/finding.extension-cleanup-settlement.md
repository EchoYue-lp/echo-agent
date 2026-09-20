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

# MCP construction 与 transport cleanup 未完全等待结算

## 外部 Issue

GitHub Issue: https://github.com/EchoYue-lp/echo-agent/issues/55

## 问题

Transport close 已能排空 pending request 并等待 task/child，但取消 in-progress MCP preparation
或 SSE construction 仍通过 `Drop` 派生无 receipt 的后台清理。

## 触发条件与影响

Runtime shutdown、caller cancellation 或重复 close 发生在 transport 建立阶段时，cleanup task
可能在调用方无法等待、观察或重试的情况下继续存在或被进程退出截断。

## 证据

`echo-integration/src/mcp/client.rs` 的 `McpPreparationOwner::drop` 与
`echo-integration/src/mcp/transport/sse.rs` 的 construction owner 显示当前缺口。

## Framework 修复范围

本仓库（echo-agent 框架）修复 MCP transport/client/manager 侧：`McpTransport::close`、
`McpClient::close`、`McpManager::disconnect`/`close_all` 传播 `Result`，SSE/stdio 排空
pending 并有界等待 owned task/child（详见 `docs/adr/0049-mcp-transport-close-settlement.md`）。
剩余 repair 必须让 preparation/construction cancellation 也交付可保留、可等待和可重试的 owner；
SDK Host 或 LSP adapter 不属于本 Finding 的完成边界。

## 处理记录

Transport close 主体已进入 framework main，修复与验证证据见
`evidence.extension-cleanup-settlement-repair.md` /
`evidence.extension-cleanup-settlement-verification.md`。当前 independent rereview 在
`main@e15cc17f` 发现 construction Drop owner 仍不可等待，因此 Finding 与 Issue #55 保持 open。
