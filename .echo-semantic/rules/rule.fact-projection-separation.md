---
schema_version: 1
id: rule.fact-projection-separation
kind: rule
status: needs_review
expectation: human_confirmed
risk: high
primary_focus: state_authority
focus: [data_durability, contract_evidence, failure_concurrency]
observed_at: source:96384cfb5e0bbc3384493805d026d65c6b096b3d0c3a26fde0cc1c007378e2c5
behavior_refs: [behavior.observation-persistence]
code_refs: [echo-core/src/agent/event_envelope.rs, echo-state/src/journal/mod.rs, echo-state/src/delivery.rs, echo-state/src/audit/mod.rs, echo-state/src/audit/file.rs, src/trace/mod.rs, src/eval/runner.rs, echo-sdk-host/src/core_profile/persistence.rs, docs/en/41-persistence-concepts.md, docs/adr/0038-eval-trace-correlation-identity.md, docs/adr/0046-turn-execution-delivery-settlement.md, docs/adr/0053-trace-audit-persistence-visibility.md]
evidence_refs: [evidence.persistence-observation, evidence.eval-trace-correlation-repair, evidence.eval-trace-correlation-verification, evidence.turn-terminal-delivery-settlement-repair, evidence.turn-terminal-delivery-settlement-verification, evidence.diagnostic-persistence-failure-visibility-repair]
finding_refs: [finding.trace-effect-event-producers, finding.eval-trace-identity, finding.trace-audit-secret-boundary, finding.turn-terminal-commit-projection-order, finding.diagnostic-persistence-failure-visibility, finding.in-memory-audit-successful-drop]
---

# Fact、Projection 与 Trace 分离

## 不变量或唯一权威

在明确 journal-backed 的领域中，Journal 保存 ordered fact，projection/checkpoint 可重建；Trace 解释执行但默认不决定业务 commit。其它 live/lossy event family 不自动拥有 Journal authority。

## 适用行为

已验证范围是 EventJournal/CheckpointedReducer/DeliveryLedger 与其 projection/replay；Agent/Task/Subagent/Workflow/Hook events、transcript、trace 和 UI/协议投影需逐 family 分类。

## 当前实现

EventEnvelope提供versioned identity/sequence，CheckpointedReducer从Journal恢复，DeliveryLedger使用Journal+reducer，RunStore独立保存producer-owned trace。Turn execution由producer terminal决定，sink projection只决定delivery；SDK恢复用真实Journal反查receipt watermark，不能从索引投影发明已交付事实。Trace/Audit persistence failure通过独立有界observer delivery报告，不改变producer terminal。

## 期望行为

丢失投影可重建；trace写失败时业务路径可继续则trace不能决定提交；需要trace的质量投影遇到歧义或存储不一致必须失败可见；gap/retention floor必须显式可观察。

## 证据

Persistence 文档、ADR 0007/0019/0030/0053 与 journal/delivery/event tests 提供证据。

## 裁决记录

用户要求明确 Journal、Projection、Trace、Delivery Ledger 谁是事实源；diagnostic persistence repair仍待工程验证，完整event family分类尚未完成，故本规则保持needs_review。
