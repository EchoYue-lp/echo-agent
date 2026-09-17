---
schema_version: 1
id: map.context-memory
kind: capability_map
title: Context、Memory、Compression 与 Checkpoint
risk: high
observed_at: source:eff0290e1aff3c3a56f0ba94f57460e220d08f8eb04d3023efde058982972f03
boundary_refs: [boundary.context-memory]
behavior_refs: [behavior.context-memory-lifecycle]
rule_refs: [rule.context-persistence-separation]
evidence_refs: [evidence.agent-context-execution, evidence.persistence-observation, evidence.high-risk-audit-frontier, evidence.framework-concept-navigation, evidence.transcript-projection-settlement-repair, evidence.transcript-projection-settlement-verification]
finding_refs: [finding.transcript-projection-settlement, finding.transcript-generation-runtime-identity, finding.checkpoint-current-plan-orphan-authority]
audit_refs: [audit.context-memory.data-durability, audit.transcript-generation-runtime-identity-rereview, audit.transcript-projection-settlement-rereview]
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
    status: needs_review
    source_refs: [src/agent/snapshot.rs, src/agent/react/mod.rs, src/state/mod.rs]
    finding_refs: [finding.checkpoint-current-plan-orphan-authority]
    unknown: AgentCheckpoint.current_plan 可保存恢复，但未发现 production writer 建立 canonical Task plan state
    next_step: consolidation/decision 选择接通 canonical Task artifact 或退役该 checkpoint 字段
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

四层 authority、transcript settlement 与 clear/delete 已映射；assembler alignment 和 current_plan writer
保持 needs_review。

## 未展开项

Evolution memory mutation 由 eval/evolution map 展开。
