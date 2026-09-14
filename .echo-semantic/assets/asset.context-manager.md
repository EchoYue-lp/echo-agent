---
schema_version: 1
id: asset.context-manager
kind: asset
title: 默认 ReAct ContextManager
asset_type: state_authority
status: active
risk: high
observed_at: f1e9027246760661144786e9e35615cd46d580c6
boundary_refs: [boundary.context-memory]
code_refs: [echo-state/src/compression/mod.rs, src/agent/react/run/context.rs]
consumer_refs: [src/agent/react/mod.rs, src/agent/react/run/phases/think.rs]
behavior_refs: [behavior.context-memory-lifecycle]
rule_refs: [rule.context-persistence-separation]
evidence_refs: [evidence.agent-context-execution]
finding_refs: []
candidate_refs: [asset.context-assembler]
---

# 默认 ReAct ContextManager

## 资产身份

默认 ReactAgent 活跃消息、token budget、compression 与 canonical context 的状态权威。

## 来源与消费者

ReAct prepare/think/finalize 消费，compressors/tokenizer/memory promoter 提供策略。

## 生命周期

Agent 构造后持有，按 invocation reset/restore，LLM 前 prepare，safe point 保存。

## 候选关系

与独立 ContextAssembler 相邻但不是同一生产路径。

## 未知与限制

两者的排序与预算不变量是否需完全对齐仍待审计。
