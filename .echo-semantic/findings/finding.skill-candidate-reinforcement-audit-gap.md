---
schema_version: 1
id: finding.skill-candidate-reinforcement-audit-gap
kind: finding
type: implementation_bug
status: open
severity: medium
primary_focus: data_durability
focus: [contract_evidence, state_authority]
boundary_ref: boundary.eval-evolution
behavior_refs: [behavior.eval-evolution]
rule_refs: [rule.quality-observation-boundary]
evidence_refs: [evidence.provider-protocol-quality]
audit_refs: [audit.eval-evolution.data-durability]
decision_refs: []
repair_evidence_refs: []
verification_evidence_refs: []
rereview_audit_refs: []
discovered_at: f1e9027246760661144786e9e35615cd46d580c6
---

# Skill candidate reinforcement不写audit

## 外部 Issue

GitHub Issue: https://github.com/EchoYue-lp/echo-agent/issues/94

## 问题

新candidate写ChangeLog，但已有candidate的sample/confidence reinforcement在Store更新后直接返回，不记录audit。

## 触发条件与影响

后续draft依据的证据强度可持续改变却无法追溯来源、时间或回滚，破坏candidate lifecycle审计。

## 证据

`src/evolution/candidate.rs`的新建与reinforcement分支展示不对等。

## 处理记录

Data-durability Audit确认；后续repair把reinforcement纳入同一durable mutation/audit identity。
