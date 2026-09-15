---
schema_version: 1
id: behavior.observation-persistence
kind: behavior
status: needs_review
expectation: inferred
risk: high
primary_focus: data_durability
focus: [state_authority, time_lifecycle, failure_concurrency, contract_evidence]
boundary: boundary.observation-persistence-delivery
observed_at: source:66a74859cd586d80d2ad791b3a2369b31e7bcff60f6afb6a4edf3575a29a7778
code_refs: [echo-core/src/agent/event_envelope.rs, echo-state/src/journal/mod.rs, echo-state/src/journal/file.rs, echo-state/src/journal/segmented.rs, echo-state/src/delivery.rs, src/trace/mod.rs, src/eval/runner.rs, src/agent/react/mod.rs, src/state/mod.rs, echo-core/src/memory/conversation.rs, echo-sdk-host/src/core_profile/persistence.rs, echo-sdk-host/src/core_profile/state.rs, docs/adr/0038-eval-trace-correlation-identity.md, docs/adr/0046-turn-execution-delivery-settlement.md, docs/adr/0055-checkpoint-journal-identity.md]
rule_refs: [rule.fact-projection-separation]
evidence_refs: [evidence.persistence-observation, evidence.checkpoint-journal-binding-repair, evidence.checkpoint-journal-binding-verification, evidence.checkpoint-journal-sdk-inventory, evidence.eval-trace-correlation-repair, evidence.eval-trace-correlation-verification, evidence.turn-terminal-delivery-settlement-repair, evidence.turn-terminal-delivery-settlement-verification]
finding_refs: [finding.trace-effect-event-producers, finding.eval-trace-identity, finding.trace-audit-secret-boundary, finding.turn-terminal-commit-projection-order, finding.checkpoint-journal-binding]
---

# Observation、Persistence 与 Delivery

## 重要承诺

Durable fact、checkpoint、live stream、bounded replay、projection 和 diagnostic trace 必须保持不同语义角色。

## 当前行为

`EventEnvelope`提供identity/sequence/hash/parent；Journal保存有序事实并以generation identity限定sequence，Journal派生checkpoint携带相同identity；Delivery Ledger归约投递生命周期；RuntimeStateStore、ConversationStore、Store与RunStore保存不同数据域。TurnReceipt把execution与delivery作为两个不可互相覆盖的结果；SDK恢复要求Journal、index和receipt watermark一致。Trace producer分配真实Run ID，调用方identity只作为parent/turn/execution correlation。

## 期望行为

只有该领域指定的 authority 驱动恢复与状态决策；UI/feed/trace 不从 EOF 或渲染结果发明终态。

## 触发、结果与副作用

Agent/Task/Subagent/Workflow/Hook/Delivery 事件进入不同 stream 或 store，消费者可查询、replay、ack、投影和保留。

## 失败、重试与恢复

Gap、lag、torn tail、unknown batch outcome、checkpoint mismatch、retention floor、generation drift与trace correlation歧义/存储不一致必须显式处理。

## 证据

EventEnvelope、Journal/checkpoint、Delivery Ledger、Store/Trace 实现与 ADR 0007/0019/0030/0055 提供基础证据。

## 裁决记录

所有 event family 的 durable/live/lossy/diagnostic 分类仍需高风险 audit 逐项反证。
