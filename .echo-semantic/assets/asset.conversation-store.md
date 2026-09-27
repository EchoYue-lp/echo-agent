---
schema_version: 1
id: asset.conversation-store
kind: asset
title: Conversation Transcript Store
asset_type: state_authority
status: active
risk: high
observed_at: source:6fdcc1782b7aa2c97a148f7c377d60b4ed26b9d470f02c05a651de1e8e2eac74
boundary_refs: [boundary.context-memory, boundary.observation-persistence-delivery]
code_refs: [echo-core/src/memory/conversation.rs, echo-state/src/memory/conversation.rs, echo-state/src/memory/file_conversation.rs, echo-state/src/memory/sqlite_conversation.rs, src/agent/snapshot.rs]
consumer_refs: [src/agent/react/run/phases/compact.rs, src/agent/react/run/phases/finalize.rs, docs/en/03-memory.md]
behavior_refs: [behavior.context-memory-lifecycle, behavior.observation-persistence]
rule_refs: [rule.context-persistence-separation, rule.fact-projection-separation]
evidence_refs: [evidence.agent-context-execution, evidence.persistence-observation, evidence.transcript-projection-settlement-repair, evidence.transcript-projection-settlement-verification, evidence.managed-import-generation-repair, evidence.transcript-observer-current-repair, evidence.transcript-observer-current-verification, evidence.framework-only-finding-closure-verification]
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
Generation-bound import 在替换消息和推进 epoch 的同一事务内写入新 frontier，并在提交前验证消息可恢复。
每个 managed call 携带 absolute deadline；File/SQLite 在实际 authority lock/transaction 后复查。

## 候选关系

不替代 RuntimeStateStore、长期 Store、Journal 或 Trace。

## 未知与限制

外部 adapter 必须显式声明 AtomicV1 与 AbsoluteDeadlineV1；consumer 对新 public contract 的
映射由其所属仓库追踪，不阻塞 Issue #106。EKO 不采用 SQLite 不影响 framework option。
