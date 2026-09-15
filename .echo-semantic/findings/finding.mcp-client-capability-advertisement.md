---
schema_version: 1
id: finding.mcp-client-capability-advertisement
kind: finding
type: intent_gap
status: resolved
severity: high
primary_focus: contract_evidence
focus: [result_side_effect, time_lifecycle, permission_external]
boundary_ref: boundary.extension-lifecycle
behavior_refs: [behavior.extension-publication]
rule_refs: [rule.extension-generation-authority]
evidence_refs: [evidence.effects-extensions, evidence.mcp-client-capability-advertisement-repair, evidence.mcp-client-capability-advertisement-verification]
audit_refs: [audit.extension-lifecycle.time-lifecycle, audit.extension-lifecycle.permission-external, audit.mcp-client-capability-advertisement-rereview]
decision_refs: []
repair_evidence_refs: [evidence.mcp-client-capability-advertisement-repair]
verification_evidence_refs: [evidence.mcp-client-capability-advertisement-verification]
rereview_audit_refs: [audit.mcp-client-capability-advertisement-rereview]
discovered_at: f1e9027246760661144786e9e35615cd46d580c6
---

# MCP client 广告未实现 capability

## 外部 Issue

GitHub Issue: https://github.com/EchoYue-lp/echo-agent/issues/65

## 问题

Initialize 广告 roots/list-changed、sampling 和 elicitation，但未发现 server-to-client request handler；stdio 对无 id 消息直接忽略。

## 触发条件与影响

Server 根据协商结果发出对应 request/notification 时，Client 可能丢弃消息或永远不响应，违反协议能力声明。

## 证据

`echo-integration/src/mcp/client.rs` 的 capabilities 与 transport/notification 路径提供静态证据。

## 处理记录

commit `f30a1fc05153832870c420d5415d436aadb8b07f`停止广告未实现client capability，
四版本协商与空capabilities wire通过独立复审。
