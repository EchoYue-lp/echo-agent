---
schema_version: 1
id: finding.extension-cleanup-settlement
kind: finding
type: implementation_bug
status: resolved
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

# MCP construction 与 transport cleanup 等待结算

## 外部 Issue

GitHub Issue: https://github.com/EchoYue-lp/echo-agent/issues/55

## 问题

原 transport close 已能排空 pending request 并等待 task/child，但取消 in-progress MCP
preparation 或 SSE construction 曾通过 `Drop` 派生无 receipt 的后台清理。

## 触发条件与影响

修复前，runtime shutdown、caller cancellation 或重复 close 发生在 transport 建立阶段时，
cleanup task 可能在调用方无法等待、观察或重试的情况下继续存在或被进程退出截断。

## 证据

原 `echo-integration/src/mcp/client.rs` 的 preparation Drop owner 与
`echo-integration/src/mcp/transport/sse.rs` 的 construction owner 构成故障反例；当前源码、
专项测试与独立复审分别由三个闭合引用记录。

## Framework 修复范围

本仓库（echo-agent 框架）修复 MCP transport/client/manager 侧：`McpTransport::close`、
`McpClient::close`、`McpManager::disconnect`/`close_all` 传播 `Result`，SSE/stdio 排空
pending 并有界等待 owned task/child（详见 `docs/adr/0049-mcp-transport-close-settlement.md`）。
本轮让 preparation/construction cancellation 交付可保留、可等待和可重试的 owner；
SDK Host 或 LSP adapter 不属于本 Finding 的完成边界。

## 处理记录

Transport close 主体此前已进入 framework main。`main@e15cc17f` 的 independent rereview
发现 construction Drop owner 仍不可等待。本任务分支在 `origin/main@4532b3bc` 上完成
preparation scope、SSE construction 与 runtime 中断重试修复，focused MCP 110/110 通过，
独立只读复审 0 findings。此处 `resolved` 只表示当前工作树的 framework Finding 已由
repair、verification、rereview 三项证据闭合；本地完整门禁与 17 项 feature 矩阵已通过。Issue #55 仍 OPEN，待 PR/CI 和
远端 main 交付后逐项关闭。
