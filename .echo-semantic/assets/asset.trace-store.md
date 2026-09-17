---
schema_version: 1
id: asset.trace-store
kind: asset
title: Trace Run 与 RunStore
asset_type: state_authority
status: active
risk: medium
observed_at: ab3ed7d23f0a3fbe2bb859a7537df2546531239e
boundary_refs: [boundary.observation-persistence-delivery, boundary.eval-evolution]
code_refs: [src/trace/mod.rs, src/trace/analyzer.rs, src/eval/runner.rs, src/agent/react/mod.rs, echo-state/src/audit/mod.rs, echo-state/src/audit/file.rs, docs/adr/0038-eval-trace-correlation-identity.md, docs/adr/0053-trace-audit-persistence-visibility.md]
consumer_refs: [src/eval/runner.rs, src/improve/mod.rs, docs/en/27-tracing.md]
behavior_refs: [behavior.observation-persistence, behavior.eval-evolution]
rule_refs: [rule.fact-projection-separation, rule.quality-observation-boundary]
evidence_refs: [evidence.persistence-observation, evidence.provider-protocol-quality, evidence.eval-trace-correlation-repair, evidence.eval-trace-correlation-verification, evidence.diagnostic-persistence-failure-visibility-repair]
finding_refs: [finding.eval-trace-identity, finding.trace-effect-event-producers, finding.trace-audit-secret-boundary, finding.diagnostic-persistence-failure-visibility]
candidate_refs: []
---

# Trace Run 与 RunStore

## 资产身份

执行诊断、usage、tool/phase/error 轨迹及其查询存储。

## 来源与消费者

ReactAgent 可选记录，Analyzer/Eval/Improve 消费。

## 生命周期

Start trace、append events、finalize status、correlate/query/replay/analyze。

## 候选关系

Trace Run ID与product run/Turn/TaskRun是不同身份；parent/turn/execution只用于correlation，不替代RunStore primary key。

## 未知与限制

Eval trace correlation已由repair、verification和独立rereview闭合；持久化失败可见性已有repair候选但等待工程验证，缺失event producers、InMemory audit成功丢写与原始输入retention继续由独立Findings追踪。
