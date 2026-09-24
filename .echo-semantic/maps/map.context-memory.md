---
schema_version: 1
id: map.context-memory
kind: capability_map
title: Context、Memory、Compression 与 Checkpoint
risk: high
observed_at: source:87b717676a7b51e213630677989947777b4bed441acd8c7d655fb6c96dca77ad
boundary_refs: [boundary.context-memory]
behavior_refs: [behavior.context-memory-lifecycle]
rule_refs: [rule.context-persistence-separation]
evidence_refs: [evidence.agent-context-execution, evidence.persistence-observation, evidence.high-risk-audit-frontier, evidence.framework-concept-navigation, evidence.transcript-projection-settlement-repair, evidence.transcript-projection-settlement-verification, evidence.checkpoint-plan-authority-repair, evidence.checkpoint-plan-authority-verification, evidence.memory-provenance-authority-repair, evidence.memory-provenance-authority-verification]
finding_refs: [finding.transcript-projection-settlement, finding.transcript-generation-runtime-identity, finding.checkpoint-current-plan-orphan-authority, finding.pre-compaction-memory-trust-provenance]
audit_refs: [audit.context-memory.data-durability, audit.transcript-generation-runtime-identity-rereview, audit.transcript-projection-settlement-rereview, audit.checkpoint-plan-authority-rereview, audit.memory-provenance-authority-rereview]
related_map_refs: [map.agent-session-turn, map.observation-persistence-delivery, map.eval-evolution]
scenarios:
  active-context-and-compression:
    status: mapped
    source_refs: [echo-state/src/compression/mod.rs, src/agent/react/run/context.rs]
    behavior_refs: [behavior.context-memory-lifecycle]
    rule_refs: [rule.context-persistence-separation]
  checkpoint-transcript-memory-separation:
    status: mapped
    source_refs: [src/agent/snapshot.rs, src/agent/react/run/phases/compact.rs, src/agent/react/run/phases/tools.rs, src/agent/react/run/phases/finalize.rs, src/state/mod.rs, echo-core/src/memory/conversation.rs, echo-core/src/memory/store.rs]
    rule_refs: [rule.context-persistence-separation]
    evidence_refs: [evidence.persistence-observation]
  transcript-projection-settlement:
    status: mapped
    source_refs: [echo-core/src/memory/conversation.rs, echo-state/src/memory/file_conversation.rs, echo-state/src/memory/sqlite_conversation.rs, src/state/mod.rs, src/state/file.rs, src/state/sqlite.rs, src/agent/snapshot.rs, src/agent/react/run/stream_channel.rs, src/agent/react/run/phases/compact.rs, src/agent/react/run/phases/tools.rs, src/agent/react/run/phases/finalize.rs, docs/adr/0056-durable-transcript-projection-settlement.md]
    behavior_refs: [behavior.context-memory-lifecycle, behavior.observation-persistence]
    rule_refs: [rule.context-persistence-separation, rule.fact-projection-separation]
    evidence_refs: [evidence.transcript-projection-settlement-repair, evidence.transcript-projection-settlement-verification]
    finding_refs: [finding.transcript-projection-settlement]
  runtime-incarnation-clear:
    status: mapped
    source_refs: [src/state/mod.rs, src/state/file.rs, src/state/sqlite.rs, docs/adr/0006-runtime-state-scope-lineage.md]
    behavior_refs: [behavior.context-memory-lifecycle]
    evidence_refs: [evidence.agent-context-execution]
  runtime-transcript-identity:
    status: mapped
    source_refs: [src/agent/snapshot.rs, src/state/mod.rs, src/agent/react/run/stream_channel.rs]
    behavior_refs: [behavior.context-memory-lifecycle]
    rule_refs: [rule.context-persistence-separation]
    evidence_refs: [evidence.transcript-generation-runtime-identity-repair]
    finding_refs: [finding.transcript-generation-runtime-identity]
  assembler-manager-alignment:
    status: needs_review
    source_refs: [src/context/mod.rs, echo-state/src/compression/mod.rs]
    unknown: ContextAssembler 只服务自定义 loop，与默认 ContextManager 的 source ordering/budget/projection 不变量未声明完全对等或明确不同
    next_step: 在 context audit 中比较相同输入并决定共享 contract 还是文档化差异
  checkpoint-current-plan:
    status: mapped
    source_refs: [src/agent/snapshot.rs, src/agent/react/mod.rs, src/agent/react/run/context.rs, src/agent/react/run/stream_channel.rs, src/agent/react/tests.rs, src/state/mod.rs, src/state/file.rs, src/state/sqlite.rs, docs/adr/0008-canonical-runtime-task-authority.md]
    finding_refs: [finding.checkpoint-current-plan-orphan-authority]
    behavior_refs: [behavior.context-memory-lifecycle, behavior.task-subagent-execution]
    rule_refs: [rule.context-persistence-separation, rule.task-subagent-authority]
    evidence_refs: [evidence.checkpoint-plan-authority-repair, evidence.checkpoint-plan-authority-verification]
    audit_refs: [audit.checkpoint-plan-authority-rereview]
  reviewed-long-term-memory:
    status: mapped
    source_refs: [echo-core/src/memory/types.rs, echo-state/src/compression/mod.rs, src/agent/config.rs, src/agent/react/builder.rs, src/agent/react/mod.rs, src/agent/react/run/context.rs, src/evolution/layer.rs, src/evolution/recall.rs, src/evolution/triggers.rs, src/evolution/review.rs, src/tools/builtin/memory.rs, src/memory_promoter.rs, echo-agent-learning/tests/example_contracts/demo31_memory_tools.rs, echo-agent-learning/examples/demo18_semantic_memory.rs, echo-agent-learning/examples/demo27_sqlite_memory.rs, echo-agent-learning/examples/demo45_customer_service.rs, docs/adr/0070-memory-provenance-and-recall-authority.md]
    finding_refs: [finding.pre-compaction-memory-trust-provenance]
    behavior_refs: [behavior.context-memory-lifecycle, behavior.eval-evolution]
    rule_refs: [rule.context-persistence-separation, rule.quality-observation-boundary]
    evidence_refs: [evidence.memory-provenance-authority-repair, evidence.memory-provenance-authority-verification]
    audit_refs: [audit.memory-provenance-authority-rereview]
---

# Context、Memory、Compression 与 Checkpoint

## 能力范围

覆盖活跃模型 context、token window、压缩、runtime checkpoint、transcript、长期 memory 和 clear/delete。

## 入口与输出

Agent 构造/Invocation/LLM prepare、turn finalize、resume/reset/clear/delete 和 memory tools 触发；输出 bounded messages 与持久记录。

## 行为关系

ContextManager、RuntimeStateStore、ConversationStore、Store 各自拥有不同语义，压缩不重写 transcript。

## 状态与数据流

Stable conversation scope 可以拥有多个 runtime incarnation；checkpoint 保存 ReAct state 和 transcript cursor，不拥有 Task DAG。

## 策略来源与优先级

Invocation runtime ID 优先于 product conversation/legacy config；tokenizer/model budget 与 compressor 决定 prepare。

## 生命周期与失败路径

Hydrate/reset/prepare/compact/save/restore/clear/delete；损坏或 partial write 必须显式错误或保守重建。

## 权限与敏感信息

Memory/trace 内容必须做 secret 边界；删除 scope 需区分 runtime incarnation 与 stable transcript。

## 用户侧投影

ConversationStore 供 history UI；active context 与 checkpoint 默认不直接作为用户历史展示。

## 场景处置清单

四层 authority、transcript settlement、clear/delete 与旧 current_plan 退役路径已映射；
assembler alignment 仍保持 needs_review。#42 已合入framework main并关闭；
长期 typed memory 的 Draft、approval 与 recall 由 #76 单独治理。

## 未展开项

Evolution memory mutation 由 eval/evolution map 展开。
