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
observed_at: f1e9027246760661144786e9e35615cd46d580c6
code_refs: [echo-core/src/agent/event_envelope.rs, echo-state/src/journal/mod.rs, echo-state/src/delivery.rs, src/trace/mod.rs, src/state/mod.rs, echo-core/src/memory/conversation.rs]
rule_refs: [rule.fact-projection-separation]
evidence_refs: [evidence.persistence-observation]
finding_refs: [finding.trace-effect-event-producers, finding.eval-trace-identity, finding.trace-audit-secret-boundary]
---

# Observation、Persistence 与 Delivery

## 重要承诺

Durable fact、checkpoint、live stream、bounded replay、projection 和 diagnostic trace 必须保持不同语义角色。

## 当前行为

`EventEnvelope` 提供 identity/sequence/hash/parent；Journal 保存有序事实；Delivery Ledger 归约投递生命周期；RuntimeStateStore、ConversationStore、Store 与 RunStore 保存不同数据域。

## 期望行为

只有该领域指定的 authority 驱动恢复与状态决策；UI/feed/trace 不从 EOF 或渲染结果发明终态。

## 触发、结果与副作用

Agent/Task/Subagent/Workflow/Hook/Delivery 事件进入不同 stream 或 store，消费者可查询、replay、ack、投影和保留。

## 失败、重试与恢复

Gap、lag、torn tail、unknown batch outcome、checkpoint mismatch、retention floor 与 generation drift 必须显式处理。

## 证据

EventEnvelope、Journal/checkpoint、Delivery Ledger、Store/Trace 实现与 ADR 0007/0019/0030 提供基础证据。

## 裁决记录

所有 event family 的 durable/live/lossy/diagnostic 分类仍需高风险 audit 逐项反证。
