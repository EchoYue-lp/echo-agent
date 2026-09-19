---
schema_version: 1
id: finding.skill-candidate-reinforcement-audit-gap
kind: finding
type: implementation_bug
status: resolved
severity: medium
primary_focus: data_durability
focus: [contract_evidence, state_authority]
boundary_ref: boundary.eval-evolution
behavior_refs: [behavior.eval-evolution]
rule_refs: [rule.quality-observation-boundary]
evidence_refs: [evidence.skill-candidate-audit-repair, evidence.skill-candidate-audit-verification]
audit_refs: [audit.eval-evolution.data-durability]
decision_refs: []
repair_evidence_refs: [evidence.skill-candidate-audit-repair]
verification_evidence_refs: [evidence.skill-candidate-audit-verification]
rereview_audit_refs: [audit.skill-candidate-audit-rereview]
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

Data-durability Audit确认；repair 已把 create/reinforcement 纳入同一 durable
mutation/audit identity，并通过完整分支门禁、17-feature matrix、四轮独立实现复审和
PR #143 的七项 CI。修复以 GitHub verified squash commit `d0d1e975` 进入远端 main，
post-merge closure rereview 未发现 Critical、Important 或 Minor 问题，本 Finding resolved。
#54 Skill promotion/approval/later rollback 与 #52 umbrella rollback 继续保持独立。
