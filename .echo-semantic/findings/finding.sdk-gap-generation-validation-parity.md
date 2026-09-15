---
schema_version: 1
id: finding.sdk-gap-generation-validation-parity
kind: finding
type: implementation_bug
status: resolved
severity: high
primary_focus: contract_evidence
focus: [state_authority, failure_concurrency, data_durability]
boundary_ref: boundary.protocol-surfaces
behavior_refs: [behavior.protocol-projection, behavior.sdk-facade-routing]
rule_refs: [rule.protocol-role-separation, rule.sdk-rust-authority]
evidence_refs: [evidence.provider-protocol-quality, evidence.sdk-contracts, evidence.sdk-gap-generation-validation-repair, evidence.sdk-gap-generation-validation-verification]
audit_refs: [audit.protocol-surfaces.contract-evidence, audit.sdk-gap-generation-validation-rereview]
decision_refs: []
repair_evidence_refs: [evidence.sdk-gap-generation-validation-repair]
verification_evidence_refs: [evidence.sdk-gap-generation-validation-verification]
rereview_audit_refs: [audit.sdk-gap-generation-validation-rereview]
discovered_at: f1e9027246760661144786e9e35615cd46d580c6
---

# 三语言SDK gap generation校验不对等

## 外部 Issue

GitHub Issue: https://github.com/EchoYue-lp/echo-agent/issues/88

## 问题

TypeScript用完整handle校验gap generation；Python/Java主要按stream ID路由并可能使用外来watermark，未一致验证WireHandle generation。

## 触发条件与影响

Stale/wrong-generation gap可推进当前feed cursor或ACK，导致代次隔离在三语言表现不同。

## 证据

三语言client/publisher与Host handle/gap E2E展示校验差异和测试空洞。

## 处理记录

Python与Java已在incoming event/gap边界校验完整WireHandle、sequence与gap cursor；验证失败不推进cursor/ACK，预订阅错代缓存也不会泄露给consumer。独立复审通过。Host gap ACK replay水位的独立缺口由Finding #120继续追踪。
