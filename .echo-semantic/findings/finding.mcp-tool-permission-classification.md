---
schema_version: 1
id: finding.mcp-tool-permission-classification
kind: finding
type: implementation_bug
status: resolved
severity: high
primary_focus: permission_external
focus: [result_side_effect, state_authority, contract_evidence]
boundary_ref: boundary.extension-lifecycle
behavior_refs: [behavior.extension-publication, behavior.effect-permission-execution]
rule_refs: [rule.extension-generation-authority, rule.permission-effect-order]
evidence_refs: [evidence.effects-extensions]
audit_refs: [audit.extension-lifecycle.permission-external]
decision_refs: []
repair_evidence_refs: [evidence.mcp-tool-local-classification-repair]
verification_evidence_refs: [evidence.mcp-tool-local-classification-verification]
rereview_audit_refs: [audit.mcp-tool-local-classification-rereview]
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

修复提交 `ed39fbfb839393eb6a80a89c5be06da1e869900d`、
`ceb3041d86c49562bda0492ea4c6b86d74860be1` 和
`72d1fccf74b85afe9a74e684ca3748b64642affb` 将 annotation 限定为 advisory
metadata，并以本地 `ToolCapabilities` 和 live `PermissionService` mode 统一自动调用的
surface、permission、Plan hard gate 与副作用结算。用户主动连接 MCP 的流程未增加权限门控。
