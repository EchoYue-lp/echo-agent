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
audit_refs: []
decision_refs: []
repair_evidence_refs: []
verification_evidence_refs: []
rereview_audit_refs: []
discovered_at: source:8b3972e1d2bc92f4ad59f511b6674eaaf243c1760f21973caf9e96558f71db90
---

# Skill Curator promotion 缺可验证批准与 audit authority

## 问题

`Curator::promote_to_active` 是公开持久 mutation，可直接改变 Skill lifecycle 状态，但没有 `ChangeLog` 参数或可验证 human approval input；这与 Evolution 全 mutation 可审计、高风险变更需 review 的合同冲突。

## 触发条件与影响

调用方直接提升 candidate/draft 时，状态可进入 Active 而不留下统一审计记录或批准依据，后续无法可靠解释、回滚或区分自动与人工 promotion。

## 证据

`src/evolution/curator.rs` 展示直接 mutation；`src/evolution/mod.rs`、`draft.rs`、`merge.rs`、`patch.rs` 展示声明与其它 mutation 的 ChangeLog/review 合同。

## 处理记录

Discovery 记录；后续 audit 必须明确 Skill lifecycle 唯一 owner、approval artifact 与 ChangeLog/rollback 顺序。
