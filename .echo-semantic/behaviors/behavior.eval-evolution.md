---
schema_version: 1
id: behavior.eval-evolution
kind: behavior
status: needs_review
expectation: inferred
risk: medium
primary_focus: contract_evidence
focus: [result_side_effect, data_durability, permission_external, failure_concurrency]
boundary: boundary.eval-evolution
observed_at: source:16e4824bc89e28e36a6c329505451b8ca5c86d6e4f4d1144ea01616535f3ac09
code_refs: [src/trace/mod.rs, src/eval/runner.rs, src/eval/replay.rs, echo-orchestration/src/runtime/turn_driver.rs, src/agent/react/run/stream_channel.rs, src/improve/mod.rs, src/improve/loop.rs, src/evolution/mod.rs, src/evolution/background_review.rs, src/evolution/dreaming.rs, src/evolution/layer.rs, src/evolution/mutation.rs, src/evolution/runtime_integration.rs, src/evolution/candidate.rs, src/evolution/curator.rs, src/evolution/skill_mutation.rs, src/evolution/draft.rs, src/evolution/merge.rs, src/evolution/patch.rs, src/evolution/review.rs, src/evolution/security.rs, src/agent/snapshot.rs, echo-state/src/skill_telemetry.rs, docs/adr/0037-eval-timeout-turn-settlement.md, docs/adr/0038-eval-trace-correlation-identity.md, docs/adr/0065-evolution-memory-audit-reconciliation.md, docs/adr/0068-skill-candidate-mutation-audit-reconciliation.md, docs/adr/0069-skill-lifecycle-mutation-authority.md]
rule_refs: [rule.quality-observation-boundary]
evidence_refs: [evidence.provider-protocol-quality, evidence.persistence-observation, evidence.improve-singleton-split-repair, evidence.improve-singleton-split-verification, evidence.improve-iteration-config-repair, evidence.improve-iteration-config-verification, evidence.eval-workspace-generation-repair, evidence.eval-workspace-generation-verification, evidence.eval-timeout-turn-settlement-repair, evidence.eval-timeout-turn-settlement-verification, evidence.eval-trace-correlation-repair, evidence.eval-trace-correlation-verification, evidence.evolution-memory-audit-repair, evidence.evolution-memory-audit-verification, evidence.skill-candidate-audit-repair, evidence.skill-candidate-audit-verification, evidence.skill-lifecycle-authority-repair, evidence.skill-lifecycle-authority-verification]
finding_refs: [finding.eval-trace-identity, finding.eval-timeout-settlement, finding.eval-workspace-generation-isolation, finding.improve-iteration-config, finding.improve-single-case-panic, finding.evolution-audit-atomicity, finding.evolution-changelog-rollback-authority, finding.evolution-skill-promotion-audit, finding.skill-candidate-reinforcement-audit-gap, finding.evolution-doc-namespace]
---

# Eval、Improve 与 Evolution

## 重要承诺

Quality pipeline 消费 trace、test 和人工裁决证据；它可以提出或执行已授权维护，但不能把诊断评分变成业务 commit 或自动放宽权限。

## 当前行为

EvalRunner为每次case创建唯一workspace generation和run/turn/execution correlation，并通过AgentTurnDriver取得唯一TurnReceipt。Settled后按parent/turn/execution从RunStore解析唯一真实trace，EvalResult、criteria、constraints和metrics共用该Run；零trace合法，歧义或存储不一致失败。Deadline后先请求cancel，再对同一drive future等待共享的6秒settlement grace；未settled timeout跳过RunStore与trace criteria并保留generation，caller-drop同样保留。Replay/Analyzer读取RunStore，Improve复用同一runner generation生成离线建议/轨迹。Criteria单例采用train-only disposition，EvalDrivenImprovement把public max_iterations直接传入唯一ImprovementLoop。Evolution分为Background Review/Dreaming、memory mutation、Skill candidate/draft/review/promote/merge/patch与仅有安全检查的rule-promotion surface。Memory rollback已由PR #140交付；Skill candidate payload继续由ADR 0068 Store CAS owner负责，ADR 0069候选让后续lifecycle、exact SKILL.md、approval、audit/reconcile和later inverse收敛到SkillMutationAuthority。Rule mutation/rollback明确HostOwned。

## 期望行为

训练/holdout、grader、memory promotion、skill patch/merge 和 rule promotion 保持来源、审批、回滚与 secret/injection 检查。

## 触发、结果与副作用

显式 API、测试任务或应用调度可触发评估和维护；结果包括报告、建议、memory/skill/rule 候选与 change audit。

## 失败、重试与恢复

需要trace但缺失、trace correlation歧义或存储不一致、grader failure、timeout、未结算Turn、命令失败、partial mutation、stale candidate和rollback failure不得被记为质量提升成功；cancel request本身不等于settlement。分层记忆prepare失败不得修改投影；prepare后失败报告unknown并保留可恢复债务，observer只在实时业务审计结算后触发。Skill candidate create/reinforce以private operation identity和Store CAS结算；后续Skill lifecycle先校验security与digest-bound approval，再prepare exact file/Curator projection、幂等audit和settle，restart补齐但不重放observer。non-tip/ABA、external edit、approval mismatch/replay失败关闭，unknown usage不创建Active。

## 证据

Trace/Eval/Improve/Evolution 实现、对应文档与 tests 提供当前存在性和部分行为证据。

## 裁决记录

应用调度与人工 review 属于产品适配；完整运行时覆盖和自动化边界仍需 audit。
