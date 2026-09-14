---
schema_version: 1
id: asset.conversation-store
kind: asset
title: Conversation Transcript Store
asset_type: state_authority
status: needs_review
risk: high
observed_at: f1e9027246760661144786e9e35615cd46d580c6
boundary_refs: [boundary.context-memory, boundary.observation-persistence-delivery]
code_refs: [echo-core/src/memory/conversation.rs, echo-state/src/memory/conversation.rs, echo-state/src/memory/file_conversation.rs, echo-state/src/memory/sqlite_conversation.rs, src/agent/snapshot.rs]
consumer_refs: [src/agent/react/run/phases/compact.rs, src/agent/react/run/phases/finalize.rs, docs/en/03-memory.md]
behavior_refs: [behavior.context-memory-lifecycle, behavior.observation-persistence]
rule_refs: [rule.context-persistence-separation, rule.fact-projection-separation]
evidence_refs: [evidence.agent-context-execution, evidence.persistence-observation]
finding_refs: [finding.transcript-projection-settlement]
candidate_refs: []
---

# Conversation Transcript Store

## 资产身份

`ConversationStore` 是用户 transcript projection 的独立权威；FileConversationStore 与可选 SqliteConversationStore 是同一 trait 的 framework backends，`echo-state/src/memory/conversation.rs` 提供 projection helper。

## 来源与消费者

ReactAgent safe-point/finalization 写入，history consumers 查询；不与长期 `Store` 合并。

## 生命周期

Ensure conversation、append/save projection、query、clear/delete；失败结算当前由 Finding 跟踪。

## 候选关系

不替代 RuntimeStateStore、长期 Store、Journal 或 Trace。

## 未知与限制

投影写失败仅告警且没有 retry/debt，故保持 needs_review；EKO 不采用 SQLite 不影响 framework option。
