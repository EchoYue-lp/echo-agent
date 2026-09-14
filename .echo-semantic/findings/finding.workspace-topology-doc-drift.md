---
schema_version: 1
id: finding.workspace-topology-doc-drift
kind: finding
type: evidence_gap
status: resolved
severity: medium
primary_focus: contract_evidence
focus: [state_authority]
boundary_ref: boundary.workspace-architecture
behavior_refs: [behavior.workspace-composition]
rule_refs: [rule.framework-layer-ownership]
evidence_refs: [evidence.workspace-structure, evidence.workspace-topology-doc-repair, evidence.workspace-topology-doc-verification]
audit_refs: [audit.workspace-architecture.contract-evidence, audit.workspace-topology-doc-rereview]
decision_refs: []
repair_evidence_refs: [evidence.workspace-topology-doc-repair]
verification_evidence_refs: [evidence.workspace-topology-doc-verification]
rereview_audit_refs: [audit.workspace-topology-doc-rereview]
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

Workspace Contract Audit确认。双语README已补齐SDK protocol/Host并更正为Cargo-derived 8+2+1分组；新documentation contract在旧README上red、修复后green。Repair、verification与独立复审已闭合本Finding。GitHub Issue保持open，等待远程main交付。
