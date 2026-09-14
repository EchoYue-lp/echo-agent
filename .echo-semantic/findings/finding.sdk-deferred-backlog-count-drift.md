---
schema_version: 1
id: finding.sdk-deferred-backlog-count-drift
kind: finding
type: evidence_gap
status: resolved
severity: high
primary_focus: contract_evidence
focus: [state_authority]
boundary_ref: boundary.protocol-surfaces
behavior_refs: [behavior.protocol-projection, behavior.sdk-facade-routing]
rule_refs: [rule.protocol-role-separation, rule.sdk-rust-authority]
evidence_refs: [evidence.sdk-contracts, evidence.sdk-deferred-backlog-count-repair, evidence.sdk-deferred-backlog-count-verification]
audit_refs: [audit.protocol-surfaces.contract-evidence, audit.semantic-governance-final-rereview]
decision_refs: []
repair_evidence_refs: [evidence.sdk-deferred-backlog-count-repair]
verification_evidence_refs: [evidence.sdk-deferred-backlog-count-verification]
rereview_audit_refs: [audit.semantic-governance-final-rereview]
discovered_at: d492c676d1bf0744452d96a6960124546ed3fff9
---

# SDK backlog仍混用4076 intrinsic旧口径

## 外部 Issue

GitHub Issue: https://github.com/EchoYue-lp/echo-agent/issues/116

## 问题

全workspace discovery与protocol capability map仍把4076个intrinsic route identity写成待分组SDK backlog；ADR 0032已将唯一consumer-facing backlog收敛为1441个deferred identity，且Host/Rust-only、language intrinsic和internal helper不属于逐项语言parity backlog。

## 触发条件与影响

治理状态、计划或报告消费这两处语义材料时会得到两套SDK backlog口径，重新把项目进度误解为Rust identity映射数量，并与已通过的scope reset和continuity retirement冲突。

## 证据

ADR 0032、schema v2 parity manifest、`evidence.sdk-contracts`和当前SDK map共同证明五类scope及1441个deferred；独立最终review定位了workspace discovery和protocol map中的两处旧口径。

## 处理记录

Issue #116已建立。Workspace discovery和protocol map现统一为1441个deferred identity的capability backlog，其它scope明确不属于语言parity backlog。Strict、change-evidence和continuity通过，独立rereview的Critical、Important、Minor均为0；本Finding在本地语义层resolved，Issue保持OPEN直到修复进入远程main。
