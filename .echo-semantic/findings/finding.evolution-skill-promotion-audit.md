---
schema_version: 1
id: finding.evolution-skill-promotion-audit
kind: finding
type: authority_conflict
status: open
severity: high
primary_focus: permission_external
focus: [data_durability, state_authority, contract_evidence]
boundary_ref: boundary.eval-evolution
behavior_refs: [behavior.eval-evolution]
rule_refs: [rule.quality-observation-boundary]
evidence_refs: [evidence.provider-protocol-quality]
audit_refs: [audit.eval-evolution.data-durability, audit.eval-evolution.permission-external]
decision_refs: []
repair_evidence_refs: []
verification_evidence_refs: []
rereview_audit_refs: []
discovered_at: f1e9027246760661144786e9e35615cd46d580c6
---

# Skill Curator promotion 缺可验证批准与 audit authority

## 外部 Issue

GitHub Issue: https://github.com/EchoYue-lp/echo-agent/issues/54

## 问题

Curator promotion/touch可直接Active且无approval/ChangeLog/security；SkillMerger可合入allowed_tools，SkillPatcher可直接写SKILL.md，security check未接生产；这与全mutation可审计、高风险变更需review的合同冲突。

## 触发条件与影响

调用方直接提升 candidate/draft 时，状态可进入 Active 而不留下统一审计记录或批准依据，后续无法可靠解释、回滚或区分自动与人工 promotion。

## 证据

`src/evolution/curator.rs` 展示直接 mutation；`src/evolution/mod.rs`、`draft.rs`、`merge.rs`、`patch.rs` 展示声明与其它 mutation 的 ChangeLog/review 合同。

## 处理记录

Discovery 记录；后续 audit 必须明确 Skill lifecycle 唯一 owner、approval artifact 与 ChangeLog/rollback 顺序。
