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
observed_at: source:e5661d8044dbe3eba9bc3ce5fd34a408b5fc89558472af0a66cbc6566a39c0ce
code_refs: [src/trace/mod.rs, src/eval/runner.rs, src/eval/replay.rs, src/improve/mod.rs, src/improve/loop.rs, src/evolution/mod.rs, src/evolution/background_review.rs, src/evolution/dreaming.rs, src/evolution/layer.rs, src/evolution/curator.rs, src/evolution/draft.rs, src/evolution/merge.rs, src/evolution/patch.rs, src/evolution/review.rs, src/evolution/security.rs, echo-state/src/skill_telemetry.rs]
rule_refs: [rule.quality-observation-boundary]
evidence_refs: [evidence.provider-protocol-quality, evidence.persistence-observation, evidence.improve-singleton-split-repair, evidence.improve-singleton-split-verification, evidence.improve-iteration-config-repair, evidence.improve-iteration-config-verification]
finding_refs: [finding.eval-trace-identity, finding.eval-timeout-settlement, finding.improve-iteration-config, finding.improve-single-case-panic, finding.evolution-audit-atomicity, finding.evolution-skill-promotion-audit, finding.evolution-doc-namespace]
---

# Eval、Improve 与 Evolution

## 重要承诺

Quality pipeline 消费 trace、test 和人工裁决证据；它可以提出或执行已授权维护，但不能把诊断评分变成业务 commit 或自动放宽权限。

## 当前行为

EvalRunner执行cases并评分，Replay/Analyzer读取RunStore，Improve生成离线建议/轨迹；criteria单例采用train-only disposition，EvalDrivenImprovement把public max_iterations直接传入唯一ImprovementLoop。Evolution分为Background Review/Dreaming、memory mutation、Skill candidate/draft/review/promote/merge/patch与仅有安全检查的rule-promotion surface。

## 期望行为

训练/holdout、grader、memory promotion、skill patch/merge 和 rule promotion 保持来源、审批、回滚与 secret/injection 检查。

## 触发、结果与副作用

显式 API、测试任务或应用调度可触发评估和维护；结果包括报告、建议、memory/skill/rule 候选与 change audit。

## 失败、重试与恢复

缺 trace、grader failure、命令失败、partial mutation、stale candidate 和 rollback failure 不得被记为质量提升成功。

## 证据

Trace/Eval/Improve/Evolution 实现、对应文档与 tests 提供当前存在性和部分行为证据。

## 裁决记录

应用调度与人工 review 属于产品适配；完整运行时覆盖和自动化边界仍需 audit。
