---
schema_version: 1
id: finding.mcp-version-doc-drift
kind: finding
type: evidence_gap
status: open
severity: medium
primary_focus: contract_evidence
focus: [trigger_input]
boundary_ref: boundary.extension-lifecycle
behavior_refs: [behavior.extension-publication]
rule_refs: [rule.extension-generation-authority]
evidence_refs: [evidence.effects-extensions]
audit_refs: [audit.extension-lifecycle.contract-evidence]
decision_refs: []
repair_evidence_refs: []
verification_evidence_refs: []
rereview_audit_refs: []
discovered_at: f1e9027246760661144786e9e35615cd46d580c6
---

# MCP 协议版本文档漂移

## 问题

正式 MCP 文档声明 `2025-03-26`，源码协议常量已经是 `2025-11-25`。

## 触发条件与影响

Framework consumer 按文档判断兼容性时会得到与 initialize 实际发送版本不同的信息。

## 证据

`docs/en/08-mcp.md`、对应中文文档与 `echo-integration/src/mcp/types.rs` 的版本常量直接冲突。

## 处理记录

Discovery 记录；下一阶段核对支持矩阵后同步双语文档和协议测试。
