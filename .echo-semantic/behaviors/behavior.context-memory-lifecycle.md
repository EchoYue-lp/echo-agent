---
schema_version: 1
id: behavior.context-memory-lifecycle
kind: behavior
status: needs_review
expectation: inferred
risk: high
primary_focus: data_durability
focus: [state_authority, time_lifecycle, failure_concurrency, contract_evidence]
boundary: boundary.context-memory
observed_at: f1e9027246760661144786e9e35615cd46d580c6
code_refs: [echo-state/src/compression/mod.rs, src/context/mod.rs, src/agent/snapshot.rs, src/agent/react/run/phases/compact.rs, src/agent/react/run/phases/tools.rs, src/agent/react/run/phases/finalize.rs, src/state/mod.rs, src/state/file.rs, src/state/sqlite.rs, echo-core/src/memory/conversation.rs, echo-core/src/memory/store.rs]
rule_refs: [rule.context-persistence-separation]
evidence_refs: [evidence.agent-context-execution, evidence.persistence-observation, evidence.transcript-generation-runtime-identity-repair, evidence.transcript-generation-runtime-identity-verification]
finding_refs: [finding.transcript-projection-settlement, finding.transcript-generation-runtime-identity]
---

# Context、Memory 与 Checkpoint 生命周期

## 重要承诺

活跃模型 context、runtime checkpoint、用户 transcript 和长期知识使用不同 authority；压缩只改变模型可见窗口，不重写事实历史。

## 当前行为

默认 ReAct 使用 `ContextManager`；`ContextAssembler` 是自定义 loop building block。`RuntimeStateStore` 保存 `AgentCheckpoint`，`ConversationStore` 保存 transcript，`Store` 保存长期知识。

## 期望行为

Prepare/compact/save/restore/reset/clear/delete 必须按 scope 与 generation 工作，assistant tool call/result 配对和 transcript cursor 不得跨 incarnation 混写。

## 触发、结果与副作用

LLM 调用前执行预算准备与压缩，turn safe point 保存 checkpoint/transcript，显式 reset/clear/delete 回收相应范围。

## 失败、重试与恢复

损坏 checkpoint、部分 transcript 写入、切换 runtime ID、重复 safe point 和取消中的 hydration 必须保守失败或重建，不重放已完成 effect。

## 证据

ContextManager、RuntimeStateStore、file backends、ConversationStore、Store、ADR 0004/0006 与恢复测试提供已知证据。

## 裁决记录

当前仍需复核 transcript projection 失败结算、ContextAssembler 与默认路径的策略关系，以及 `AgentCheckpoint.current_plan` 的生产写入来源。
