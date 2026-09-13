---
schema_version: 1
id: asset.trace-store
kind: asset
title: Trace Run 与 RunStore
asset_type: state_authority
status: active
risk: medium
observed_at: source:8b3972e1d2bc92f4ad59f511b6674eaaf243c1760f21973caf9e96558f71db90
boundary_refs: [boundary.observation-persistence-delivery, boundary.eval-evolution]
code_refs: [src/trace/mod.rs, src/trace/analyzer.rs]
consumer_refs: [src/eval/runner.rs, src/improve/mod.rs, docs/en/27-tracing.md]
behavior_refs: [behavior.observation-persistence, behavior.eval-evolution]
rule_refs: [rule.fact-projection-separation, rule.quality-observation-boundary]
evidence_refs: [evidence.persistence-observation, evidence.provider-protocol-quality]
finding_refs: [finding.eval-trace-identity, finding.trace-effect-event-producers, finding.trace-audit-secret-boundary]
candidate_refs: []
---

# Trace Run 与 RunStore

## 资产身份

执行诊断、usage、tool/phase/error 轨迹及其查询存储。

## 来源与消费者

ReactAgent 可选记录，Analyzer/Eval/Improve 消费。

## 生命周期

Start trace、append events、finalize status、query/replay/analyze。

## 候选关系

Trace Run ID 与 product run/Turn/TaskRun 是不同身份。

## 未知与限制

Eval trace identity、缺失 event producers 与原始输入 retention 已形成 Findings。
