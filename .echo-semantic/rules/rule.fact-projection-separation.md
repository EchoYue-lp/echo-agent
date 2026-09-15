---
schema_version: 1
id: rule.fact-projection-separation
kind: rule
status: needs_review
expectation: human_confirmed
risk: high
primary_focus: state_authority
focus: [data_durability, contract_evidence, failure_concurrency]
observed_at: source:66a74859cd586d80d2ad791b3a2369b31e7bcff60f6afb6a4edf3575a29a7778
behavior_refs: [behavior.observation-persistence]
code_refs: [echo-core/src/agent/event_envelope.rs, echo-state/src/journal/mod.rs, echo-state/src/journal/file.rs, echo-state/src/journal/segmented.rs, echo-state/src/delivery.rs, src/trace/mod.rs, src/eval/runner.rs, echo-sdk-host/src/core_profile/persistence.rs, docs/en/41-persistence-concepts.md, docs/zh/41-persistence-concepts.md, docs/adr/0038-eval-trace-correlation-identity.md, docs/adr/0046-turn-execution-delivery-settlement.md, docs/adr/0055-checkpoint-journal-identity.md]
evidence_refs: [evidence.persistence-observation, evidence.checkpoint-journal-binding-repair, evidence.checkpoint-journal-binding-verification, evidence.checkpoint-journal-sdk-inventory, evidence.eval-trace-correlation-repair, evidence.eval-trace-correlation-verification, evidence.turn-terminal-delivery-settlement-repair, evidence.turn-terminal-delivery-settlement-verification]
finding_refs: [finding.trace-effect-event-producers, finding.eval-trace-identity, finding.trace-audit-secret-boundary, finding.turn-terminal-commit-projection-order, finding.checkpoint-journal-binding]
---

# Fact、Projection 与 Trace 分离

## 不变量或唯一权威

在明确 journal-backed 的领域中，Journal 保存 ordered fact，projection/checkpoint 可重建；Trace 解释执行但默认不决定业务 commit。其它 live/lossy event family 不自动拥有 Journal authority。

## 适用行为

已验证范围是 EventJournal/CheckpointedReducer/DeliveryLedger 与其 projection/replay；Agent/Task/Subagent/Workflow/Hook events、transcript、trace 和 UI/协议投影需逐 family 分类。

## 当前实现

EventEnvelope提供versioned identity/sequence；Journal generation identity与sequence共同限定事实位置，CheckpointedReducer只接受同generation checkpoint，DeliveryLedger继续使用同一Journal+reducer；RunStore独立保存producer-owned trace。Turn execution由producer terminal决定，sink projection只决定delivery；SDK恢复用真实Journal反查receipt watermark，不能从索引投影发明已交付事实。

## 期望行为

丢失投影可重建；trace写失败时业务路径可继续则trace不能决定提交；需要trace的质量投影遇到歧义或存储不一致必须失败可见；gap/retention floor必须显式可观察。

## 证据

Persistence 文档、ADR 0007/0019/0030/0055 与 journal/delivery/event tests 提供证据；本次identity反例的focused验证、独立复审和本地integration gate均已通过，远端平台CI等待MR执行。

## 裁决记录

用户要求明确 Journal、Projection、Trace、Delivery Ledger 谁是事实源；完整 event family 分类尚未完成，故本规则保持 needs_review。
