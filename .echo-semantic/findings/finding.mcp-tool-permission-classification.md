---
schema_version: 1
id: finding.mcp-tool-permission-classification
kind: finding
type: implementation_bug
status: open
severity: high
primary_focus: permission_external
focus: [result_side_effect, state_authority, contract_evidence]
boundary_ref: boundary.extension-lifecycle
behavior_refs: [behavior.extension-publication, behavior.effect-permission-execution]
rule_refs: [rule.extension-generation-authority, rule.permission-effect-order]
evidence_refs: [evidence.effects-extensions]
audit_refs: [audit.extension-lifecycle.permission-external]
decision_refs: []
repair_evidence_refs: []
verification_evidence_refs: []
rereview_audit_refs: []
discovered_at: f1e9027246760661144786e9e35615cd46d580c6
---

# MCP server annotation 被当作自动 Tool 权限与副作用事实

## 外部 Issue

GitHub Issue: https://github.com/EchoYue-lp/echo-agent/issues/66

## 问题

McpToolAdapter 信任 server annotation 设置 ReadOnly/Dangerous，但未覆写 Tool::permissions；ReactAgent 读取空权限无需确认，Plan mode 也漏 mcp tool，readOnlyHint 还影响 partial-side-effect 分类。

## 触发条件与影响

远端 MCP server 可把有写副作用的工具标为 read-only，使 Agent 自动调用绕过可见 mutation/permission 分类并错误记录失败副作用。

## 证据

`echo-integration/src/mcp/tool_adapter.rs`、`echo-core/src/tools/mod.rs`、`src/agent/snapshot.rs` 与 pipeline 展示分类链。

## 处理记录

Permission Audit 确认；只约束 Agent 自动调用，不阻止用户主动连接 MCP。后续需本地 policy 与不可信 annotation 分层。
