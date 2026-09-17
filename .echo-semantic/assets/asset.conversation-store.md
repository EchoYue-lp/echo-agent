---
schema_version: 1
id: asset.conversation-store
kind: asset
title: Conversation Transcript Store
asset_type: state_authority
status: active
risk: high
observed_at: source:bfa5b4590c617d8286f2b80571d5c450622d47c7425d6f1ecb1978bf85743352
boundary_refs: [boundary.context-memory, boundary.observation-persistence-delivery]
code_refs: [echo-core/src/memory/conversation.rs, echo-state/src/memory/conversation.rs, echo-state/src/memory/file_conversation.rs, echo-state/src/memory/sqlite_conversation.rs, src/agent/snapshot.rs]
consumer_refs: [src/agent/react/run/phases/compact.rs, src/agent/react/run/phases/finalize.rs, docs/en/03-memory.md]
behavior_refs: [behavior.context-memory-lifecycle, behavior.observation-persistence]
rule_refs: [rule.context-persistence-separation, rule.fact-projection-separation]
evidence_refs: [evidence.agent-context-execution, evidence.persistence-observation, evidence.transcript-projection-settlement-repair, evidence.transcript-projection-settlement-verification]
finding_refs: [finding.transcript-projection-settlement]
candidate_refs: []
---

# Conversation Transcript Store

## 资产身份

`ConversationStore` 是用户 transcript projection 的独立权威；FileConversationStore 与可选 SqliteConversationStore 是同一 trait 的 framework backends，`echo-state/src/memory/conversation.rs` 提供 projection helper。

## 来源与消费者

ReactAgent safe-point/finalization 写入，history consumers 查询；不与长期 `Store` 合并。

## 生命周期

Ensure epoch、atomic apply/AlreadyApplied、managed import/metadata/delete、query 与 retention receipt。
每个 managed call 携带 absolute deadline；File/SQLite 在实际 authority lock/transaction 后复查。

## 候选关系

不替代 RuntimeStateStore、长期 Store、Journal 或 Trace。

## 未知与限制

外部 adapter 必须显式声明 AtomicV1 与 AbsoluteDeadlineV1；独立 SDK 尚未映射的新 public contract
继续由 Issue #106 后续 outcome 跟踪。EKO 不采用 SQLite 不影响 framework option。
