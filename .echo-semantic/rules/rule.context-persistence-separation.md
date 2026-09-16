---
schema_version: 1
id: rule.context-persistence-separation
kind: rule
status: verified
expectation: human_confirmed
risk: high
primary_focus: data_durability
focus: [state_authority, time_lifecycle, failure_concurrency]
observed_at: f1e9027246760661144786e9e35615cd46d580c6
behavior_refs: [behavior.agent-turn-lifecycle, behavior.context-memory-lifecycle]
code_refs: [echo-state/src/compression/mod.rs, src/agent/snapshot.rs, src/agent/react/run/phases/compact.rs, src/agent/react/run/phases/tools.rs, src/agent/react/run/phases/finalize.rs, src/state/mod.rs, echo-core/src/memory/conversation.rs, echo-core/src/memory/store.rs, docs/en/41-persistence-concepts.md]
evidence_refs: [evidence.agent-context-execution, evidence.persistence-observation, evidence.transcript-generation-runtime-identity-repair, evidence.transcript-generation-runtime-identity-verification]
finding_refs: [finding.transcript-projection-settlement, finding.transcript-generation-runtime-identity]
---

# Context 与持久化职责分离

## 不变量或唯一权威

`ContextManager` 拥有活跃模型窗口；`RuntimeStateStore` 拥有 ReAct checkpoint；`ConversationStore` 是 transcript；`Store` 是长期知识。

## 适用行为

适用于 context prepare/compact、turn finalize、resume、reset、runtime incarnation clear、conversation delete 和 memory tools。

## 当前实现

各 trait 使用独立数据模型与 key/scope；runtime lineage 区分稳定 conversation scope 和可轮换 runtime state ID。

## 期望行为

压缩不删除 transcript，trace 不恢复业务状态，checkpoint 不拥有 Task DAG；跨 incarnation 写入必须由 generation/cursor 防止。

## 证据

Persistence 文档、ADR 0004/0006、Store/Conversation/RuntimeState implementations 与 crash-cut tests 提供证据。

## 裁决记录

这些名称表示不同语义角色，不引入一个 universal Store trait 或统一 checkpoint 状态机。
