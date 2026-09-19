---
schema_version: 1
id: finding.plugin-mcp-owner-isolation
kind: finding
type: authority_conflict
status: open
severity: high
primary_focus: state_authority
focus: [failure_concurrency, time_lifecycle, result_side_effect]
boundary_ref: boundary.extension-lifecycle
behavior_refs: [behavior.extension-publication]
rule_refs: [rule.extension-generation-authority]
evidence_refs: [evidence.effects-extensions]
audit_refs: [audit.extension-lifecycle.state-authority]
decision_refs: []
repair_evidence_refs: [evidence.plugin-mcp-owner-isolation-repair]
verification_evidence_refs: [evidence.plugin-mcp-owner-isolation-verification]
rereview_audit_refs: []
discovered_at: f1e9027246760661144786e9e35615cd46d580c6
---

# Plugin MCP server 名缺少 owner 隔离

## 外部 Issue

GitHub Issue: https://github.com/EchoYue-lp/echo-agent/issues/75

## 问题

Plugin parser 与 McpManager 都用裸 server name；多个 Plugin 同名 reconcile 后，各自 receipt 仍记录相同名称，旧 owner unwire 可关闭新 owner 连接。

## 触发条件与影响

两个启用 Plugin 声明同名 MCP server，随后 disable/uninstall 其中一个时，另一个 Plugin 的 live Tool/Resource 可能被撤销。

## 证据

`echo-integration/src/mcp/config_loader.rs`、`mcp/mod.rs` 与 `src/plugin/prepared.rs` 的 reconcile/unwire 路径提供证据。

## 处理记录

候选修复已建立 owner-qualified identity、投影与 focused 验证证据，并在包含
`main@ed7d6028` 的组合树通过完整门禁。Finding 在最终独立复审和远端主线交付前保持 open。
