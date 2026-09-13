---
schema_version: 1
id: asset.long-term-memory-store
kind: asset
title: Long-term Memory Store
asset_type: state_authority
status: active
risk: high
observed_at: source:8b3972e1d2bc92f4ad59f511b6674eaaf243c1760f21973caf9e96558f71db90
boundary_refs: [boundary.context-memory, boundary.observation-persistence-delivery]
code_refs: [echo-core/src/memory/store.rs, echo-state/src/memory/store.rs, echo-state/src/memory/sqlite_store.rs, echo-state/src/memory/embedding_store.rs, echo-state/src/memory/typed_store.rs]
consumer_refs: [src/agent/react/subsystems/memory.rs, src/agent/react/mod.rs, src/evolution/layer.rs, docs/en/03-memory.md]
behavior_refs: [behavior.context-memory-lifecycle, behavior.observation-persistence]
rule_refs: [rule.context-persistence-separation, rule.fact-projection-separation]
evidence_refs: [evidence.agent-context-execution, evidence.persistence-observation]
finding_refs: []
candidate_refs: []
---

# Long-term Memory Store

## 资产身份

`Store` 是 namespaced 长期知识权威；in-memory/file、可选 SQLite、Embedding wrapper 和 TypedMemoryStore 是 framework capability/backends。

## 来源与消费者

ReactAgent memory tools、retrieval 与 Evolution consumers 使用，不由 transcript 或 runtime checkpoint 替代。

## 生命周期

Put/get/search/delete/prune、embedding index rebuild/persist 与 typed memory mutation 分别由具体 backend 和上层策略结算。

## 候选关系

不与 `ConversationStore` 合并；SQLite/Embedding 是合理 framework 选项，不能因应用未采用而判死。

## 未知与限制

Evolution mutation/audit 由 eval/evolution 边界另行审计。
