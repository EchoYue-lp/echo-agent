---
schema_version: 1
id: map.eval-evolution
kind: capability_map
title: Trace、Eval、Improve 与 Evolution
risk: high
observed_at: source:87b717676a7b51e213630677989947777b4bed441acd8c7d655fb6c96dca77ad
boundary_refs: [boundary.eval-evolution]
behavior_refs: [behavior.eval-evolution]
rule_refs: [rule.quality-observation-boundary, rule.fact-projection-separation]
evidence_refs: [evidence.provider-protocol-quality, evidence.persistence-observation, evidence.high-risk-audit-frontier, evidence.improve-singleton-split-repair, evidence.improve-singleton-split-verification, evidence.improve-iteration-config-repair, evidence.improve-iteration-config-verification, evidence.eval-workspace-generation-repair, evidence.eval-workspace-generation-verification, evidence.eval-timeout-turn-settlement-repair, evidence.eval-timeout-turn-settlement-verification, evidence.eval-trace-correlation-repair, evidence.eval-trace-correlation-verification, evidence.evolution-memory-audit-repair, evidence.evolution-memory-audit-verification, evidence.memory-provenance-authority-repair, evidence.memory-provenance-authority-verification, evidence.skill-candidate-audit-repair, evidence.skill-candidate-audit-verification, evidence.skill-lifecycle-authority-repair, evidence.skill-lifecycle-authority-verification]
finding_refs: [finding.eval-trace-identity, finding.eval-timeout-settlement, finding.improve-iteration-config, finding.improve-single-case-panic, finding.eval-workspace-generation-isolation, finding.background-review-detached-persistence-settlement, finding.evolution-audit-atomicity, finding.evolution-changelog-rollback-authority, finding.evolution-skill-promotion-audit, finding.skill-candidate-reinforcement-audit-gap, finding.evolution-doc-namespace, finding.pre-compaction-memory-trust-provenance]
audit_refs: [audit.eval-evolution.data-durability, audit.eval-evolution.failure-concurrency, audit.eval-evolution.permission-external, audit.improve-singleton-split-rereview, audit.improve-iteration-config-rereview, audit.eval-workspace-generation-rereview, audit.eval-timeout-turn-settlement-rereview, audit.eval-trace-correlation-rereview, audit.evolution-memory-audit-atomicity-rereview, audit.evolution-memory-rollback-rereview, audit.memory-provenance-authority-rereview, audit.skill-candidate-audit-rereview, audit.skill-lifecycle-authority-rereview]
related_map_refs: [map.observation-persistence-delivery, map.agent-session-turn, map.llm-provider-runtime, map.extension-lifecycle]
scenarios:
  trace-record-and-analysis:
    status: mapped
    source_refs: [src/trace/mod.rs, src/trace/analyzer.rs, src/eval/runner.rs, docs/adr/0038-eval-trace-correlation-identity.md]
    finding_refs: [finding.eval-trace-identity]
    rule_refs: [rule.fact-projection-separation]
    evidence_refs: [evidence.eval-trace-correlation-repair, evidence.eval-trace-correlation-verification]
    audit_refs: [audit.eval-trace-correlation-rereview]
  eval-run-grade-report:
    status: mapped
    source_refs: [src/eval/runner.rs, src/eval/comparator.rs, src/eval/mod.rs, src/eval/replay.rs, echo-orchestration/src/runtime/turn_driver.rs, src/agent/react/run/stream_channel.rs, docs/adr/0037-eval-timeout-turn-settlement.md, docs/adr/0038-eval-trace-correlation-identity.md]
    finding_refs: [finding.eval-trace-identity, finding.eval-timeout-settlement, finding.eval-workspace-generation-isolation]
    behavior_refs: [behavior.eval-evolution]
    evidence_refs: [evidence.eval-workspace-generation-repair, evidence.eval-workspace-generation-verification, evidence.eval-timeout-turn-settlement-repair, evidence.eval-timeout-turn-settlement-verification, evidence.eval-trace-correlation-repair, evidence.eval-trace-correlation-verification]
    audit_refs: [audit.eval-workspace-generation-rereview, audit.eval-timeout-turn-settlement-rereview, audit.eval-trace-correlation-rereview]
  improve-loop-and-trajectory:
    status: mapped
    source_refs: [src/improve/loop.rs, src/improve/eval_improvement.rs, src/improve/trajectory.rs]
    finding_refs: [finding.improve-iteration-config, finding.improve-single-case-panic, finding.eval-workspace-generation-isolation]
    evidence_refs: [evidence.provider-protocol-quality, evidence.improve-singleton-split-repair, evidence.improve-singleton-split-verification, evidence.improve-iteration-config-repair, evidence.improve-iteration-config-verification, evidence.eval-workspace-generation-repair, evidence.eval-workspace-generation-verification]
    audit_refs: [audit.improve-singleton-split-rereview, audit.improve-iteration-config-rereview, audit.eval-workspace-generation-rereview]
  evolution-background-review-and-dreaming:
    status: needs_review
    source_refs: [src/evolution/background_review.rs, src/evolution/dreaming.rs, src/evolution/runtime_integration.rs, src/evolution/review.rs]
    rule_refs: [rule.quality-observation-boundary]
    unknown: proposal-only review、可选 auto-persistence、deterministic dreaming 与 application scheduling 的协调 owner 未闭合
    next_step: 区分 observation/proposal 与 mutation，并审计 join/取消/失败结算
  evolution-memory-mutation:
    status: needs_review
    source_refs: [src/evolution/layer.rs, src/evolution/mutation.rs, src/evolution/audit.rs, src/evolution/review.rs, src/evolution/runtime_integration.rs, src/tools/builtin/memory.rs, src/memory_promoter.rs, src/agent/react/run/context.rs, src/evolution/security.rs, docs/adr/0065-evolution-memory-audit-reconciliation.md]
    finding_refs: [finding.evolution-audit-atomicity, finding.evolution-changelog-rollback-authority, finding.evolution-doc-namespace, finding.pre-compaction-memory-trust-provenance]
    rule_refs: [rule.quality-observation-boundary]
    evidence_refs: [evidence.evolution-memory-audit-repair, evidence.evolution-memory-audit-verification, evidence.evolution-memory-rollback-repair, evidence.evolution-memory-rollback-verification]
    audit_refs: [audit.evolution-memory-audit-atomicity-rereview, audit.evolution-memory-rollback-rereview]
    unknown: durable prepare/reconcile、memory later rollback与Skill lifecycle authority均已在主线交付并通过独立复审；raw Store读者仍可暂见prepared中间态，Rule rollback保持host-owned，旧namespace仍属独立范围
    next_step: Rule owner在application boundary验收，旧namespace依其Finding处置
  memory-provenance-and-recall:
    status: mapped
    source_refs: [echo-core/src/memory/types.rs, echo-state/src/compression/mod.rs, echo-state/src/memory/typed_store.rs, src/agent/config.rs, src/agent/react/builder.rs, src/agent/react/mod.rs, src/agent/react/run/context.rs, src/agent/react/run/phases/compact.rs, src/memory_promoter.rs, src/evolution/layer.rs, src/evolution/recall.rs, src/evolution/triggers.rs, src/evolution/review.rs, src/evolution/dreaming.rs, src/evolution/background_review.rs, src/tools/builtin/memory.rs, echo-agent-learning/tests/example_contracts/demo31_memory_tools.rs, echo-agent-learning/examples/demo18_semantic_memory.rs, echo-agent-learning/examples/demo27_sqlite_memory.rs, echo-agent-learning/examples/demo45_customer_service.rs, docs/adr/0070-memory-provenance-and-recall-authority.md]
    behavior_refs: [behavior.eval-evolution, behavior.context-memory-lifecycle]
    rule_refs: [rule.quality-observation-boundary, rule.context-persistence-separation]
    finding_refs: [finding.pre-compaction-memory-trust-provenance]
    evidence_refs: [evidence.memory-provenance-authority-repair, evidence.memory-provenance-authority-verification]
    audit_refs: [audit.memory-provenance-authority-rereview]
  evolution-skill-lifecycle:
    status: mapped
    source_refs: [src/evolution/candidate.rs, src/evolution/curator.rs, src/evolution/skill_mutation.rs, src/evolution/draft.rs, src/evolution/merge.rs, src/evolution/patch.rs, src/evolution/review.rs, src/evolution/security.rs, src/agent/snapshot.rs, docs/adr/0068-skill-candidate-mutation-audit-reconciliation.md, docs/adr/0069-skill-lifecycle-mutation-authority.md]
    finding_refs: [finding.evolution-skill-promotion-audit, finding.skill-candidate-reinforcement-audit-gap]
    rule_refs: [rule.quality-observation-boundary]
    evidence_refs: [evidence.skill-candidate-audit-repair, evidence.skill-candidate-audit-verification, evidence.skill-lifecycle-authority-repair, evidence.skill-lifecycle-authority-verification]
    audit_refs: [audit.skill-candidate-audit-rereview, audit.skill-lifecycle-authority-rereview]
  evolution-rule-promotion-surface:
    status: mapped
    source_refs: [src/evolution/security.rs, src/evolution/mod.rs]
    rule_refs: [rule.quality-observation-boundary]
  runtime-trigger-and-human-review:
    status: needs_review
    source_refs: [src/agent/react/run/context.rs, src/evolution/runtime_integration.rs, src/evolution/review.rs]
    finding_refs: [finding.background-review-detached-persistence-settlement, finding.pre-compaction-memory-trust-provenance]
    unknown: Background Review task settlement和应用调度仍由 #38 及产品owner追踪；记忆Draft/approval由MemoryLayerManager拥有
    next_step: "#38 处理detached review的join、取消、deadline和持久终态"
---

# Trace、Eval、Improve 与 Evolution

## 能力范围

覆盖执行 trace、case/constraint/grader eval、离线 improvement、trajectory export、typed memory/skill/rule evolution 与 change audit。

## 入口与输出

ReactAgent 可选记录 trace；显式 Eval/Improve API 和 runtime/app trigger 消费；输出 Run/report/suggestion/trajectory/candidate/mutation audit。

## 行为关系

Trace 是 observation，Eval/Improve 消费但不驱动业务 commit；Evolution 可写持久状态，分层记忆与Skill lifecycle分别用资源owner的journal恢复和owner-applied inverse；ChangeLog保持append-only，Rule mutation/rollback归host。

## 状态与数据流

RunStore保存producer-owned trace；EvalRunner拥有每次invocation唯一run/turn/execution correlation并只把已load的真实trace ID写入EvalResult；AgentTurnDriver/TurnReceipt是Eval invocation终态权威；EvalWorkspaceGeneration持有每次run的临时目录与cleanup disposition；EvalResult/Report保存评分；ImprovementLoop保存迭代结果；MemoryLayer的operation journal保存可恢复写入事实，ChangeLog保存业务审计，Curator保存独立技能状态。

## 策略来源与优先级

Eval cases/constraints、grader、explicit config、memory source/provenance/risk/status 和 host review decision 决定行为。

## 生命周期与失败路径

Trace start/finalize；Eval run/deadline/cancel/bounded settlement/correlate trace/grade/report，未settled timeout跳过RunStore与评分并保留generation；trace缺失保持可选，歧义或存储不一致失败；Improve iterate/stop/export；Evolution detect/review/prepare/project/audit/settle/reconcile；memory与Skill later rollback由各自resource owner以generation fencing、inverse lineage和request-id幂等结算，Rule rollback保持host-owned。

## 权限与敏感信息

Tool command、fixture、memory/skill/rule 写入是外部 effect；自动抽取只形成Draft，来源证据与显式批准分开，untrusted source不能自动提升，secret/injection检查必须保留。

## 用户侧投影

Report/dashboard/suggestions 是质量投影，不等同产品成功或允许自动改变权限。

## 场景处置清单

Trace/Eval/Improve、Background Review/Dreaming、Memory mutation、Skill lifecycle与Rule promotion已分别路由；memory与Skill rollback authority已交付，Rule明确为HostOwned；其它开放Finding继续按各自边界处理。

## 未展开项

Application review dashboard、schedule 和 UI 不进入 framework baseline。
