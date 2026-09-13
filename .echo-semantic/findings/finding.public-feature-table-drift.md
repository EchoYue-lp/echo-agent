---
schema_version: 1
id: finding.public-feature-table-drift
kind: finding
type: evidence_gap
status: resolved
severity: medium
primary_focus: contract_evidence
focus: [trigger_input, state_authority]
boundary_ref: boundary.workspace-architecture
behavior_refs: [behavior.workspace-composition]
rule_refs: [rule.framework-layer-ownership]
evidence_refs: [evidence.workspace-structure, evidence.feature-table-doc-repair, evidence.feature-table-doc-verification]
audit_refs: [audit.workspace-architecture.contract-evidence, audit.feature-table-doc-rereview]
decision_refs: []
repair_evidence_refs: [evidence.feature-table-doc-repair]
verification_evidence_refs: [evidence.feature-table-doc-verification]
rereview_audit_refs: [audit.feature-table-doc-rereview]
discovered_at: f1e9027246760661144786e9e35615cd46d580c6
---

# README feature表列出不存在的tasks feature

## 外部 Issue

GitHub Issue: https://github.com/EchoYue-lp/echo-agent/issues/79

## 问题

Root Cargo.toml没有tasks feature且明确Task API属于core；双语README feature表仍把tasks列为可启用feature，后文又给出相反说明。

## 触发条件与影响

用户按公开表配置`features=["tasks"]`会遇到Cargo feature resolution错误，并无法判断Task API真实可用条件。

## 证据

Cargo.toml与README.md/README.zh.md feature段落形成直接合同反例。

## 处理记录

Workspace Contract Audit确认。双语README已删除不存在的`tasks`行并就近说明Task API属framework core；Cargo-derived exact-set contract在旧README上red、修复后green。Repair、verification与独立复审已闭合本Finding。GitHub Issue保持open，等待远程main交付。
