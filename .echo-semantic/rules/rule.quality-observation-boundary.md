---
schema_version: 1
id: rule.quality-observation-boundary
kind: rule
status: needs_review
expectation: inferred
risk: medium
primary_focus: result_side_effect
focus: [contract_evidence, data_durability, permission_external]
observed_at: source:8b3972e1d2bc92f4ad59f511b6674eaaf243c1760f21973caf9e96558f71db90
behavior_refs: [behavior.eval-evolution]
code_refs: [src/trace/mod.rs, src/eval/runner.rs, src/improve/mod.rs, src/improve/loop.rs, src/evolution/mod.rs, src/evolution/background_review.rs, src/evolution/dreaming.rs, src/evolution/layer.rs, src/evolution/curator.rs, src/evolution/draft.rs, src/evolution/merge.rs, src/evolution/patch.rs, src/evolution/review.rs, src/evolution/security.rs]
evidence_refs: [evidence.provider-protocol-quality, evidence.persistence-observation]
finding_refs: [finding.eval-trace-identity, finding.eval-timeout-settlement, finding.improve-iteration-config, finding.improve-single-case-panic, finding.evolution-audit-atomicity, finding.evolution-skill-promotion-audit, finding.evolution-doc-namespace]
---

# Quality Observation 不替代业务权威

## 不变量或唯一权威

Trace/Eval/Improve 观察和评价执行；Evolution 的持久 mutation 仍需来源、审计和显式授权，任何评分都不自动成为业务 commit 或权限变更。

## 适用行为

适用于 RunStore、EvalRunner、grader/constraints、ImprovementLoop、trajectory、memory/skill/rule candidate 与 change audit。

## 当前实现

Eval/Improve 显式消费 Agent/trace；Evolution runtime integration 可生成或写入 memory/skill artifacts，并记录 change log。

## 期望行为

Trace identity、timeout settlement、iteration configuration 与 mutation/audit 原子边界必须一致；人工裁决仍控制语义推广。

## 证据

Trace/Eval/Improve/Evolution 源码、tests/examples 和正式文档提供部分证据。

## 裁决记录

Eval trace/timeout、Improve max_iterations、Evolution audit 原子性与文档 namespace 已形成待审 Finding。
