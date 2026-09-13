---
schema_version: 1
id: rule.fact-projection-separation
kind: rule
status: needs_review
expectation: human_confirmed
risk: high
primary_focus: state_authority
focus: [data_durability, contract_evidence, failure_concurrency]
observed_at: source:8b3972e1d2bc92f4ad59f511b6674eaaf243c1760f21973caf9e96558f71db90
behavior_refs: [behavior.observation-persistence]
code_refs: [echo-core/src/agent/event_envelope.rs, echo-state/src/journal/mod.rs, echo-state/src/delivery.rs, src/trace/mod.rs, docs/en/41-persistence-concepts.md]
evidence_refs: [evidence.persistence-observation]
finding_refs: [finding.trace-effect-event-producers, finding.eval-trace-identity, finding.trace-audit-secret-boundary]
---

# Fact、Projection 与 Trace 分离

## 不变量或唯一权威

在明确 journal-backed 的领域中，Journal 保存 ordered fact，projection/checkpoint 可重建；Trace 解释执行但默认不决定业务 commit。其它 live/lossy event family 不自动拥有 Journal authority。

## 适用行为

已验证范围是 EventJournal/CheckpointedReducer/DeliveryLedger 与其 projection/replay；Agent/Task/Subagent/Workflow/Hook events、transcript、trace 和 UI/协议投影需逐 family 分类。

## 当前实现

EventEnvelope 提供 versioned identity/sequence，CheckpointedReducer 从 Journal 恢复，DeliveryLedger 使用 Journal+reducer，RunStore 独立保存 trace。

## 期望行为

丢失投影可重建；trace 写失败时业务路径可继续则 trace 不能决定提交；gap/retention floor 必须显式可观察。

## 证据

Persistence 文档、ADR 0007/0019/0030 与 journal/delivery/event tests 提供证据。

## 裁决记录

用户要求明确 Journal、Projection、Trace、Delivery Ledger 谁是事实源；完整 event family 分类尚未完成，故本规则保持 needs_review。
