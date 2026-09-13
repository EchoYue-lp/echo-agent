---
schema_version: 1
id: map.eval-evolution
kind: capability_map
title: Trace、Eval、Improve 与 Evolution
risk: high
observed_at: 81e2756cee9127fa23a9bb1023bd56aa8f954964
boundary_refs: [boundary.eval-evolution]
behavior_refs: [behavior.eval-evolution]
rule_refs: [rule.quality-observation-boundary, rule.fact-projection-separation]
evidence_refs: [evidence.provider-protocol-quality, evidence.persistence-observation, evidence.high-risk-audit-frontier, evidence.improve-singleton-split-repair, evidence.improve-singleton-split-verification, evidence.improve-iteration-config-repair, evidence.improve-iteration-config-verification, evidence.eval-workspace-generation-repair, evidence.eval-workspace-generation-verification, evidence.eval-timeout-turn-settlement-repair, evidence.eval-timeout-turn-settlement-verification, evidence.eval-trace-correlation-repair, evidence.eval-trace-correlation-verification]
finding_refs: [finding.eval-trace-identity, finding.eval-timeout-settlement, finding.improve-iteration-config, finding.improve-single-case-panic, finding.eval-workspace-generation-isolation, finding.background-review-detached-persistence-settlement, finding.evolution-audit-atomicity, finding.evolution-changelog-rollback-authority, finding.evolution-skill-promotion-audit, finding.skill-candidate-reinforcement-audit-gap, finding.evolution-doc-namespace, finding.pre-compaction-memory-trust-provenance]
audit_refs: [audit.eval-evolution.data-durability, audit.eval-evolution.failure-concurrency, audit.eval-evolution.permission-external, audit.improve-singleton-split-rereview, audit.improve-iteration-config-rereview, audit.eval-workspace-generation-rereview, audit.eval-timeout-turn-settlement-rereview, audit.eval-trace-correlation-rereview]
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
    source_refs: [src/evolution/layer.rs, src/evolution/audit.rs, src/evolution/security.rs]
    finding_refs: [finding.evolution-audit-atomicity, finding.evolution-changelog-rollback-authority, finding.evolution-doc-namespace, finding.pre-compaction-memory-trust-provenance]
    rule_refs: [rule.quality-observation-boundary]
    unknown: memory mutation 与 audit 非原子，正式 namespace 文档漂移
    next_step: audit commit/audit/rollback 顺序与当前 namespace contract
  evolution-skill-lifecycle:
    status: needs_review
    source_refs: [src/evolution/curator.rs, src/evolution/draft.rs, src/evolution/merge.rs, src/evolution/patch.rs, src/evolution/review.rs, src/evolution/security.rs]
    finding_refs: [finding.evolution-skill-promotion-audit, finding.skill-candidate-reinforcement-audit-gap]
    rule_refs: [rule.quality-observation-boundary]
    unknown: Curator public promotion 可直接持久化 active 状态，未携带可验证 human approval 或 ChangeLog；其它 draft/merge/patch 各自有审计合同
    next_step: audit skill candidate/draft/review/promote/merge/patch 的唯一 lifecycle 与授权证据
  evolution-rule-promotion-surface:
    status: needs_review
    source_refs: [src/evolution/security.rs, src/evolution/mod.rs]
    unknown: 仅发现 rule-promotion 安全检查，未发现 framework 内规则 mutation/持久 authority，不能声明 rule evolution 已 mapped
    next_step: 在 application boundary audit 中确认 consumer；若无实现则收窄公开文档与 capability 名称
    rule_refs: [rule.quality-observation-boundary]
  runtime-trigger-and-human-review:
    status: needs_review
    source_refs: [src/agent/react/run/context.rs, src/evolution/runtime_integration.rs, src/evolution/review.rs]
    finding_refs: [finding.background-review-detached-persistence-settlement, finding.pre-compaction-memory-trust-provenance]
    unknown: 自动维护、应用调度、detached review settlement与人工批准的完整production coordination未形成一个owner
    next_step: semantic-decide trusted-host/ApprovalArtifact边界，并repair后台任务与pre-compaction provenance
---

# Trace、Eval、Improve 与 Evolution

## 能力范围

覆盖执行 trace、case/constraint/grader eval、离线 improvement、trajectory export、typed memory/skill/rule evolution 与 change audit。

## 入口与输出

ReactAgent 可选记录 trace；显式 Eval/Improve API 和 runtime/app trigger 消费；输出 Run/report/suggestion/trajectory/candidate/mutation audit。

## 行为关系

Trace 是 observation，Eval/Improve 消费但不驱动业务 commit；Evolution 可写持久状态，需独立 authorization/audit/rollback。

## 状态与数据流

RunStore保存producer-owned trace；EvalRunner拥有每次invocation唯一run/turn/execution correlation并只把已load的真实trace ID写入EvalResult；AgentTurnDriver/TurnReceipt是Eval invocation终态权威；EvalWorkspaceGeneration持有每次run的临时目录与cleanup disposition；EvalResult/Report保存评分；ImprovementLoop保存迭代结果；MemoryLayer/Curator/ChangeLog保存演化状态。

## 策略来源与优先级

Eval cases/constraints、grader、explicit config、memory source/risk/status 和 human decision 决定行为。

## 生命周期与失败路径

Trace start/finalize；Eval run/deadline/cancel/bounded settlement/correlate trace/grade/report，未settled timeout跳过RunStore与评分并保留generation；trace缺失保持可选，歧义或存储不一致失败；Improve iterate/stop/export；Evolution detect/review/apply/audit/rollback。

## 权限与敏感信息

Tool command、fixture、memory/skill/rule 写入是外部 effect；untrusted source 不能自动提升，secret/injection 检查必须保留。

## 用户侧投影

Report/dashboard/suggestions 是质量投影，不等同产品成功或允许自动改变权限。

## 场景处置清单

Trace/Eval/Improve、Background Review/Dreaming、Memory mutation、Skill lifecycle与Rule promotion已分别路由；trace correlation、singleton panic、iteration config、workspace generation与timeout settlement已关闭，其它Finding保持open。

## 未展开项

Application review dashboard、schedule 和 UI 不进入 framework baseline。
