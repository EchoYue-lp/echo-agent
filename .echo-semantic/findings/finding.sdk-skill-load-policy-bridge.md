---
schema_version: 1
id: finding.sdk-skill-load-policy-bridge
kind: finding
type: authority_conflict
status: resolved
severity: high
primary_focus: state_authority
focus: [contract_evidence, time_lifecycle]
boundary_ref: boundary.sdk-facade-parity
behavior_refs: [behavior.sdk-facade-routing]
rule_refs: [rule.sdk-rust-authority]
evidence_refs: [evidence.sdk-contracts]
audit_refs: [audit.sdk-facade-plan08-final]
decision_refs: []
repair_evidence_refs: [evidence.sdk-contracts]
verification_evidence_refs: [evidence.sdk-contracts]
rereview_audit_refs: [audit.sdk-facade-plan08-final]
discovered_at: source:13192164b42c8866c7eefcb6085ce026369709d5eec4500ab6c94bb285436519
---

# SkillLoadPolicy 被误归为process-local

## 问题

第四轮审查发现SkillLoadPolicy会影响真实Session Agent的discovery、plugin registration与reconcile，却仍被manifest归为语言本地实现。

## 触发条件与影响

语言侧policy无法影响Host执行，导致同一公开API在Rust与SDK中的可见Skill集合不对等。

## 证据

trait现为可等待回调，AgentComponent提供skill_load_allows typed operation与完整descriptor投影；真实Session E2E验证discovery过滤及reconcile回调。

## 处理记录

代码修复与focused验证已完成；第十轮独立复审确认该finding闭合。
