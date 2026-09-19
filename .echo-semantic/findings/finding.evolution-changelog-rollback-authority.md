---
schema_version: 1
id: finding.evolution-changelog-rollback-authority
kind: finding
type: intent_gap
status: open
severity: high
primary_focus: data_durability
focus: [contract_evidence, failure_concurrency, state_authority]
boundary_ref: boundary.eval-evolution
behavior_refs: [behavior.eval-evolution]
rule_refs: [rule.quality-observation-boundary]
evidence_refs: [evidence.evolution-memory-rollback-repair, evidence.evolution-memory-rollback-verification, evidence.skill-lifecycle-authority-repair, evidence.skill-lifecycle-authority-verification]
audit_refs: [audit.eval-evolution.data-durability]
decision_refs: []
repair_evidence_refs: [evidence.evolution-memory-rollback-repair, evidence.skill-lifecycle-authority-repair]
verification_evidence_refs: [evidence.evolution-memory-rollback-verification, evidence.skill-lifecycle-authority-verification]
rereview_audit_refs: [audit.evolution-memory-rollback-rereview, audit.skill-lifecycle-authority-rereview]
discovered_at: f1e9027246760661144786e9e35615cd46d580c6
---

# Evolution ChangeLog不提供later rollback authority

## 外部 Issue

GitHub Issue: https://github.com/EchoYue-lp/echo-agent/issues/52

## 问题

ChangeLog只有record/query/latest/len，没有rollback apply API；模块和文档却宣称rollback-capable，draft/merge/patch仅在同调用内best-effort补偿。

## 触发条件与影响

进程在mutation后、audit或补偿前中断，或用户稍后请求rollback时，日志不能驱动可验证恢复。

## 证据

`src/evolution/audit.rs`、`evolution/mod.rs`与双语self-improvement文档展示trait和承诺差异。

## 处理记录

Data-durability Audit确认；memory rollback 已由 PR #140 交付远端主线。ADR 0069 候选补充
framework Skill lifecycle 的 owner-applied inverse、generation CAS 与 approval，并通过四轮独立
复审、完整 workspace 门禁及17-feature matrix。Finding 仍保持 open，等待 remote-main 交付，
并保留 Rule persistence/rollback 的 typed HostOwned 边界。
