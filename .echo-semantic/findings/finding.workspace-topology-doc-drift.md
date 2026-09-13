---
schema_version: 1
id: finding.workspace-topology-doc-drift
kind: finding
type: evidence_gap
status: open
severity: medium
primary_focus: contract_evidence
focus: [state_authority]
boundary_ref: boundary.workspace-architecture
behavior_refs: [behavior.workspace-composition]
rule_refs: [rule.framework-layer-ownership]
evidence_refs: [evidence.workspace-structure]
audit_refs: [audit.workspace-architecture.contract-evidence]
decision_refs: []
repair_evidence_refs: []
verification_evidence_refs: []
rereview_audit_refs: []
discovered_at: f1e9027246760661144786e9e35615cd46d580c6
---

# 双语README workspace拓扑遗漏SDK crates

## 外部 Issue

GitHub Issue: https://github.com/EchoYue-lp/echo-agent/issues/114

## 问题

Cargo定义root加10 members共11 package；双语README拓扑遗漏echo-sdk-protocol与echo-sdk-host，并声称8 production crates+1 teaching crate。

## 触发条件与影响

用户按README理解架构时看不到协议/Host边界，容易把SDK能力误解为root crate内部实现。

## 证据

Root Cargo.toml与README.md/README.zh.md的成员图和计数形成直接反例。

## 处理记录

Workspace Contract Audit确认；属于双语文档修复，不需要runtime/API改动。
