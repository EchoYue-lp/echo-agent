---
schema_version: 1
id: finding.lsp-manager-derived-handle-resurrection
kind: finding
type: authority_conflict
status: resolved
severity: high
primary_focus: time_lifecycle
focus: [state_authority, failure_concurrency, result_side_effect]
boundary_ref: boundary.extension-lifecycle
behavior_refs: [behavior.extension-publication, behavior.protocol-projection]
rule_refs: [rule.extension-generation-authority, rule.protocol-role-separation]
evidence_refs: [evidence.effects-extensions, evidence.provider-protocol-quality, evidence.lsp-derived-handle-lifecycle-repair, evidence.lsp-derived-handle-lifecycle-verification]
audit_refs: [audit.extension-lifecycle.time-lifecycle, audit.lsp-derived-handle-lifecycle-rereview]
decision_refs: []
repair_evidence_refs: [evidence.lsp-derived-handle-lifecycle-repair]
verification_evidence_refs: [evidence.lsp-derived-handle-lifecycle-verification]
rereview_audit_refs: [audit.lsp-derived-handle-lifecycle-rereview]
discovered_at: f1e9027246760661144786e9e35615cd46d580c6
---

# 派生 LSP client handle 可在 manager 关闭后复活进程

## 外部 Issue

GitHub Issue: https://github.com/EchoYue-lp/echo-agent/issues/63

## 问题

SDK get_client 注册独立 client handle 且无 parent/generation link；关闭 manager 只关闭 manager ID并保留不同 ID 的 client handle，后者仍可 initialize 新进程。

## 触发条件与影响

Manager close 被消费者理解为资源树关闭后，旧派生 handle 可启动脱离 manager authority 的 LSP child，造成生命周期和资源 ownership 分叉。

## 证据

`echo-sdk-host/src/core_profile/facade/integrations.rs` 的 get_client/close 与 LSP initialize 路径提供源码证据。

## 处理记录

ADR 0043确认manager级联ownership；commits `20ddba34`与
`2eab1ac9923e0f99a70cc08d88de0ee64ea90ee6`实现generation/closed fence与Host级联结算，
独立复审通过。
