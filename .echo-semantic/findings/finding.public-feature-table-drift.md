---
schema_version: 1
id: finding.public-feature-table-drift
kind: finding
type: evidence_gap
status: open
severity: medium
primary_focus: contract_evidence
focus: [trigger_input, state_authority]
boundary_ref: boundary.workspace-architecture
behavior_refs: [behavior.workspace-composition]
rule_refs: [rule.framework-layer-ownership]
evidence_refs: [evidence.workspace-structure]
audit_refs: [audit.workspace-architecture.contract-evidence]
decision_refs: []
repair_evidence_refs: []
verification_evidence_refs: []
rereview_audit_refs: []
discovered_at: source:8b3972e1d2bc92f4ad59f511b6674eaaf243c1760f21973caf9e96558f71db90
---

# README feature表列出不存在的tasks feature

## 问题

Root Cargo.toml没有tasks feature且明确Task API属于core；双语README feature表仍把tasks列为可启用feature，后文又给出相反说明。

## 触发条件与影响

用户按公开表配置`features=["tasks"]`会遇到Cargo feature resolution错误，并无法判断Task API真实可用条件。

## 证据

Cargo.toml与README.md/README.zh.md feature段落形成直接合同反例。

## 处理记录

Workspace Contract Audit确认；属于双语文档修复并应补manifest-derived contract check。
