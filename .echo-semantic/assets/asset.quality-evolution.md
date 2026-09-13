---
schema_version: 1
id: asset.quality-evolution
kind: asset
title: Trace、Eval、Improve 与 Evolution
asset_type: state_authority
status: needs_review
risk: high
observed_at: source:c205eb521ef63e2d37d921693a1a0703b253144b575642baa3ec94c3ba2d75b3
boundary_refs: [boundary.eval-evolution]
code_refs: [src/trace/mod.rs, src/eval/runner.rs, src/eval/comparator.rs, src/improve/mod.rs, src/improve/loop.rs, src/evolution/mod.rs, src/evolution/background_review.rs, src/evolution/dreaming.rs, src/evolution/layer.rs, src/evolution/curator.rs, src/evolution/draft.rs, src/evolution/merge.rs, src/evolution/patch.rs, src/evolution/review.rs, src/evolution/security.rs, echo-state/src/skill_telemetry.rs, docs/adr/0036-eval-workspace-generation-lifecycle.md]
consumer_refs: [echo-agent-learning/tests/example_contracts/demo50_eval.rs, echo-agent-learning/tests/example_contracts/demo51_self_improvement.rs]
behavior_refs: [behavior.eval-evolution]
rule_refs: [rule.quality-observation-boundary]
evidence_refs: [evidence.provider-protocol-quality, evidence.persistence-observation, evidence.improve-singleton-split-repair, evidence.improve-singleton-split-verification, evidence.improve-iteration-config-repair, evidence.improve-iteration-config-verification, evidence.eval-workspace-generation-repair, evidence.eval-workspace-generation-verification]
finding_refs: [finding.eval-trace-identity, finding.eval-timeout-settlement, finding.eval-workspace-generation-isolation, finding.improve-iteration-config, finding.improve-single-case-panic, finding.evolution-audit-atomicity, finding.evolution-skill-promotion-audit, finding.evolution-doc-namespace]
candidate_refs: []
---

# Trace、Eval、Improve 与 Evolution

## 资产身份

执行观察、评估、离线改进与持久语义演化的 quality pipeline。

## 来源与消费者

ReactAgent trace producer、Eval/Improve API、Evolution runtime integration 和 examples 消费。

## 生命周期

Record/finalize trace、create/retain/close workspace generation、run/grade/report eval、suggest/export improve、detect/review/apply/audit evolution。

## 候选关系

Quality observation 不替代业务 commit；Evolution 持久 mutation 需要独立审计。

## 未知与限制

Trace identity、timeout settlement、memory audit atomicity、Skill promotion audit与文档namespace仍有Finding；workspace generation已有本轮修复候选，singleton split与iteration config已闭合。
