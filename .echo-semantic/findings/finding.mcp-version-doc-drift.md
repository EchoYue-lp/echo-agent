---
schema_version: 1
id: finding.mcp-version-doc-drift
kind: finding
type: evidence_gap
status: resolved
severity: medium
primary_focus: contract_evidence
focus: [trigger_input]
boundary_ref: boundary.extension-lifecycle
behavior_refs: [behavior.extension-publication]
rule_refs: [rule.extension-generation-authority]
evidence_refs: [evidence.effects-extensions, evidence.mcp-protocol-negotiation-repair, evidence.mcp-protocol-negotiation-verification]
audit_refs: [audit.extension-lifecycle.contract-evidence, audit.mcp-protocol-negotiation-rereview]
decision_refs: []
repair_evidence_refs: [evidence.mcp-protocol-negotiation-repair]
verification_evidence_refs: [evidence.mcp-protocol-negotiation-verification]
rereview_audit_refs: [audit.mcp-protocol-negotiation-rereview]
discovered_at: f1e9027246760661144786e9e35615cd46d580c6
---

# MCP 协议版本文档漂移

## 外部 Issue

GitHub Issue: https://github.com/EchoYue-lp/echo-agent/issues/67

## 问题

正式 MCP 文档声明 `2025-03-26`，源码协议常量已经是 `2025-11-25`。

## 触发条件与影响

Framework consumer 按文档判断兼容性时会得到与 initialize 实际发送版本不同的信息。

## 证据

`docs/en/08-mcp.md`、对应中文文档与 `echo-integration/src/mcp/types.rs` 的版本常量直接冲突。

## 处理记录

双语文档现列出同一四版本支持矩阵；client在发送`notifications/initialized`与发现capability
之前验证server选择的`protocolVersion`，未知版本以typed initialization failure失败并关闭
transport。四个支持版本与未知版本测试、focused验证和独立复审均通过，本Finding已关闭。
