---
schema_version: 1
id: finding.hook-event-producer-contract
kind: finding
type: evidence_gap
status: open
severity: medium
primary_focus: contract_evidence
focus: [state_authority, result_side_effect]
boundary_ref: boundary.observation-persistence-delivery
behavior_refs: [behavior.observation-persistence, behavior.extension-publication]
rule_refs: [rule.fact-projection-separation, rule.extension-generation-authority]
evidence_refs: [evidence.persistence-observation, evidence.effects-extensions]
audit_refs: [audit.observation-persistence-delivery.contract-evidence, audit.extension-lifecycle.state-authority]
decision_refs: []
repair_evidence_refs: []
verification_evidence_refs: []
rereview_audit_refs: []
discovered_at: source:8b3972e1d2bc92f4ad59f511b6674eaaf243c1760f21973caf9e96558f71db90
---

# HookEvent catalog 与自动 producer 不一致

## 问题

HookEvent::ALL 和文档描述完整自动生命周期，但 PermissionDenied 无 dedicated producer，多种 Notification/Config/Skill/Rule lifecycle 只发现通用手动 dispatch/context 分支。

## 触发条件与影响

用户配置 Hook 期待事件自动触发时可能永远收不到；仅凭 enum/catalog 无法证明生产覆盖。

## 证据

`echo-core/src/hooks/types.rs`、Hook 调用点与正式文档构成 producer/contract 矩阵；PluginLoaded/Disabled 另由 plugin lifecycle Finding 承接。

## 处理记录

Contract-evidence Audit 记录；后续逐事件决定补 producer 或收窄 catalog/docs。
