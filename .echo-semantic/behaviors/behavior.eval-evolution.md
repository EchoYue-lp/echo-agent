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
observed_at: source:0d241bfdeeb69882ac21d5647af13f50f25fd491a34dcc6a7dd0f70daf2ac08a
code_refs: [src/trace/mod.rs, src/eval/runner.rs, src/eval/replay.rs, echo-orchestration/src/runtime/turn_driver.rs, src/agent/react/run/stream_channel.rs, src/improve/mod.rs, src/improve/loop.rs, src/evolution/mod.rs, src/evolution/background_review.rs, src/evolution/dreaming.rs, src/evolution/layer.rs, src/evolution/mutation.rs, src/evolution/runtime_integration.rs, src/evolution/curator.rs, src/evolution/draft.rs, src/evolution/merge.rs, src/evolution/patch.rs, src/evolution/review.rs, src/evolution/security.rs, echo-state/src/skill_telemetry.rs, docs/adr/0037-eval-timeout-turn-settlement.md, docs/adr/0038-eval-trace-correlation-identity.md, docs/adr/0065-evolution-memory-audit-reconciliation.md]
rule_refs: [rule.quality-observation-boundary]
evidence_refs: [evidence.provider-protocol-quality, evidence.persistence-observation, evidence.improve-singleton-split-repair, evidence.improve-singleton-split-verification, evidence.improve-iteration-config-repair, evidence.improve-iteration-config-verification, evidence.eval-workspace-generation-repair, evidence.eval-workspace-generation-verification, evidence.eval-timeout-turn-settlement-repair, evidence.eval-timeout-turn-settlement-verification, evidence.eval-trace-correlation-repair, evidence.eval-trace-correlation-verification, evidence.evolution-memory-audit-repair, evidence.evolution-memory-audit-verification]
finding_refs: [finding.eval-trace-identity, finding.eval-timeout-settlement, finding.eval-workspace-generation-isolation, finding.improve-iteration-config, finding.improve-single-case-panic, finding.evolution-audit-atomicity, finding.evolution-skill-promotion-audit, finding.evolution-doc-namespace]
---

# Eval、Improve 与 Evolution

## 重要承诺

Quality pipeline 消费 trace、test 和人工裁决证据；它可以提出或执行已授权维护，但不能把诊断评分变成业务 commit 或自动放宽权限。

## 当前行为

EvalRunner为每次case创建唯一workspace generation和run/turn/execution correlation，并通过AgentTurnDriver取得唯一TurnReceipt。Settled后按parent/turn/execution从RunStore解析唯一真实trace，EvalResult、criteria、constraints和metrics共用该Run；零trace合法，歧义或存储不一致失败。Deadline后先请求cancel，再对同一drive future等待共享的6秒settlement grace；未settled timeout跳过RunStore与trace criteria并保留generation，caller-drop同样保留。Replay/Analyzer读取RunStore，Improve复用同一runner generation生成离线建议/轨迹。Criteria单例采用train-only disposition，EvalDrivenImprovement把public max_iterations直接传入唯一ImprovementLoop。Evolution分为Background Review/Dreaming、memory mutation、Skill candidate/draft/review/promote/merge/patch与仅有安全检查的rule-promotion surface。主线 `cb4ee9ed` 的分层记忆以operation journal记录prepare与settled，`ChangeLog`按固定ID幂等提交；已结算投影回退和未结算组都在启动恢复，原始Store读者仍可能暂见中间态。#52 候选增加以ChangeId/BatchId定位的preview与later rollback：merge整批、generation CAS防ABA、inverse lineage与request-id幂等；Skill/Rule/host rollback仍独立。

## 期望行为

训练/holdout、grader、memory promotion、skill patch/merge 和 rule promotion 保持来源、审批、回滚与 secret/injection 检查。

## 触发、结果与副作用

显式 API、测试任务或应用调度可触发评估和维护；结果包括报告、建议、memory/skill/rule 候选与 change audit。

## 失败、重试与恢复

需要trace但缺失、trace correlation歧义或存储不一致、grader failure、timeout、未结算Turn、命令失败、partial mutation、stale candidate和rollback failure不得被记为质量提升成功；cancel request本身不等于settlement。分层记忆prepare失败不得修改投影；prepare后失败报告unknown并保留可恢复债务，observer只在实时业务审计结算后触发。晋升/降级旧值在串行prepare区复核；热层非规范单行内容使用带标记的JSON字符串无损存放。

## 证据

Trace/Eval/Improve/Evolution 实现、对应文档与 tests 提供当前存在性和部分行为证据。

## 裁决记录

应用调度与人工 review 属于产品适配；完整运行时覆盖和自动化边界仍需 audit。
