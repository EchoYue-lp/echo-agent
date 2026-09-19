---
schema_version: 1
id: asset.quality-evolution
kind: asset
title: Trace、Eval、Improve 与 Evolution
asset_type: state_authority
status: needs_review
risk: high
observed_at: source:e2f708d5ddfba82cdb921b3559e440532246d4f9b679a4fa104b7f8f173b3a6b
boundary_refs: [boundary.eval-evolution]
code_refs: [src/trace/mod.rs, src/eval/runner.rs, src/eval/comparator.rs, echo-orchestration/src/runtime/turn_driver.rs, src/agent/react/run/stream_channel.rs, src/improve/mod.rs, src/improve/loop.rs, src/evolution/mod.rs, src/evolution/background_review.rs, src/evolution/dreaming.rs, src/evolution/layer.rs, src/evolution/mutation.rs, src/evolution/runtime_integration.rs, src/evolution/candidate.rs, src/evolution/curator.rs, src/evolution/draft.rs, src/evolution/merge.rs, src/evolution/patch.rs, src/evolution/review.rs, src/evolution/security.rs, echo-state/src/skill_telemetry.rs, docs/adr/0036-eval-workspace-generation-lifecycle.md, docs/adr/0037-eval-timeout-turn-settlement.md, docs/adr/0038-eval-trace-correlation-identity.md, docs/adr/0065-evolution-memory-audit-reconciliation.md, docs/adr/0068-skill-candidate-mutation-audit-reconciliation.md]
consumer_refs: [echo-agent-learning/tests/example_contracts/demo50_eval.rs, echo-agent-learning/tests/example_contracts/demo51_self_improvement.rs]
behavior_refs: [behavior.eval-evolution]
rule_refs: [rule.quality-observation-boundary]
evidence_refs: [evidence.provider-protocol-quality, evidence.persistence-observation, evidence.improve-singleton-split-repair, evidence.improve-singleton-split-verification, evidence.improve-iteration-config-repair, evidence.improve-iteration-config-verification, evidence.eval-workspace-generation-repair, evidence.eval-workspace-generation-verification, evidence.eval-timeout-turn-settlement-repair, evidence.eval-timeout-turn-settlement-verification, evidence.eval-trace-correlation-repair, evidence.eval-trace-correlation-verification, evidence.evolution-memory-audit-repair, evidence.evolution-memory-audit-verification, evidence.skill-candidate-audit-repair, evidence.skill-candidate-audit-verification]
finding_refs: [finding.eval-trace-identity, finding.eval-timeout-settlement, finding.eval-workspace-generation-isolation, finding.improve-iteration-config, finding.improve-single-case-panic, finding.evolution-audit-atomicity, finding.evolution-skill-promotion-audit, finding.skill-candidate-reinforcement-audit-gap, finding.evolution-doc-namespace]
candidate_refs: []
---

# Trace、Eval、Improve 与 Evolution

## 资产身份

执行观察、评估、离线改进与持久语义演化的 quality pipeline。

## 来源与消费者

ReactAgent trace producer、Eval/Improve API、Evolution runtime integration 和 examples 消费。

## 生命周期

Record/finalize/query trace、create/retain/close workspace generation、drive/deadline/cancel/settle/correlate/grade/report eval、suggest/export improve、detect/review/prepare/project/audit/reconcile evolution。

## 候选关系

Quality observation 不替代业务 commit；Evolution 持久 mutation 需要独立审计。

## 未知与限制

Trace correlation、timeout settlement、workspace generation、singleton split与iteration config已闭合；memory audit durable reconciliation已在主线 `cb4ee9ed` 交付并通过独立复审，raw Store直接读取的中间态、later rollback、Skill promotion audit与文档namespace仍需明确边界。
