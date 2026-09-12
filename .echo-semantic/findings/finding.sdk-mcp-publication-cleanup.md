---
schema_version: 1
id: finding.sdk-mcp-publication-cleanup
kind: finding
type: implementation_bug
status: resolved
severity: high
primary_focus: time_lifecycle
focus: [failure_concurrency, contract_evidence]
boundary_ref: boundary.sdk-facade-parity
behavior_refs: [behavior.sdk-facade-routing]
rule_refs: [rule.sdk-rust-authority]
evidence_refs: [evidence.sdk-contracts]
audit_refs: [audit.sdk-facade-plan08-final]
decision_refs: []
repair_evidence_refs: [evidence.sdk-contracts]
verification_evidence_refs: [evidence.sdk-contracts]
rereview_audit_refs: [audit.sdk-facade-plan08-final]
discovered_at: source:524b2f07633e8b5c757b49785e85280c60149f8ff4129632c3cd1e6bfa71d910
---

# MCP 初始化后发布失败未关闭transport

## 问题

第四轮审查发现McpClient::from_transport成功初始化后，facade resource注册失败路径曾直接丢弃client而不关闭语言transport。

## 触发条件与影响

连接达到max_facade_resources时，SDK已经收到initialize与initialized，但不会收到close，造成生命周期泄漏。

## 证据

adapter现于handle publication失败时显式等待client.close()；低配额真实Host E2E验证失败路径close、释放占位后重试成功及正常close。

## 处理记录

代码修复与focused验证已完成；第十轮独立复审确认该finding闭合。
