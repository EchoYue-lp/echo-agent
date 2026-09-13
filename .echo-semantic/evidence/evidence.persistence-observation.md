---
schema_version: 1
id: evidence.persistence-observation
kind: evidence
observed_at: source:8d6ff0470d17f79ed8a03d9a7582f36e94d1a96bb02cbafa853d97ba05cee64e
source_refs:
  - echo-core/src/agent/event_envelope.rs
  - echo-core/src/memory/store.rs
  - echo-core/src/memory/conversation.rs
  - echo-state/src/memory/conversation.rs
  - echo-state/src/memory/file_conversation.rs
  - echo-state/src/memory/sqlite_conversation.rs
  - echo-state/src/memory/store.rs
  - echo-state/src/memory/sqlite_store.rs
  - echo-state/src/memory/embedding_store.rs
  - echo-state/src/memory/typed_store.rs
  - echo-state/src/journal/mod.rs
  - echo-state/src/journal/file.rs
  - echo-state/src/journal/segmented.rs
  - echo-state/src/delivery.rs
  - src/state/mod.rs
  - src/state/file.rs
  - src/state/sqlite.rs
  - src/agent/snapshot.rs
  - src/agent/react/run/phases/compact.rs
  - src/agent/react/run/phases/tools.rs
  - src/agent/react/run/phases/finalize.rs
  - src/trace/mod.rs
  - src/agent/react/mod.rs
  - src/eval/runner.rs
  - src/agent/react/run/pipeline.rs
  - echo-state/src/audit/memory.rs
  - echo-state/src/audit/mod.rs
  - echo-state/src/audit/file.rs
  - src/agent/subagent/events.rs
  - echo-orchestration/src/tasks/events.rs
  - echo-orchestration/src/workflow/graph.rs
  - docs/en/41-persistence-concepts.md
  - docs/en/41-delivery-ledger.md
  - docs/adr/0004-async-file-store-ownership.md
  - docs/adr/0007-atomic-journal-batch-commits.md
  - docs/adr/0019-typed-delivery-ledger-api.md
  - docs/adr/0030-versioned-subagent-event-envelope.md
  - docs/adr/0038-eval-trace-correlation-identity.md
supports: [behavior.observation-persistence, rule.fact-projection-separation]
limitations:
  - 所有事件 family 的完整 durable/live/lossy/diagnostic 分类仍需下一阶段 audit 逐项反证
---

# Persistence 与 Observation 证据

## 支持的结论

`EventJournal`保存有序事实，checkpoint加速恢复，`RuntimeStateStore`保存ReAct runtime state，`ConversationStore`是transcript投影，`Store`保存长期知识，`RunStore`以producer-owned Run ID保存诊断trace并保留parent/turn/execution correlation，`DeliveryLedger`通过journal与reducer拥有交付生命周期。Eval只把唯一精确匹配且可load的真实trace ID投影到结果。

## 来源与范围

来源覆盖事件信封、Conversation/long-term/runtime Store traits与内建File/SQLite/Embedding/typed backends、snapshot producers、Journal/checkpoint、Delivery Ledger、Trace producer/Eval consumer correlation、Task/Subagent/Workflow事件和持久化ADR。

## 已知缺口

通过只证明各已列 authority 的实现与合同存在，不证明每个事件消费者在 lag、gap、retention 和 generation 切换下都正确；trace/audit 的原始输入 retention 已由 Finding 路由。
