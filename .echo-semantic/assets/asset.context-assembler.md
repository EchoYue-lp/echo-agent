---
schema_version: 1
id: asset.context-assembler
kind: asset
title: 自定义 Loop ContextAssembler
asset_type: symbol
status: candidate
risk: medium
observed_at: source:8b3972e1d2bc92f4ad59f511b6674eaaf243c1760f21973caf9e96558f71db90
boundary_refs: [boundary.context-memory]
code_refs: [src/context/mod.rs, src/context/selector.rs]
consumer_refs: [echo-agent-learning/tests/example_contracts/demo65_context_assembler.rs, echo-agent-learning/tests/example_contracts/demo66_context_selector.rs]
behavior_refs: [behavior.context-memory-lifecycle]
rule_refs: [rule.context-persistence-separation]
evidence_refs: [evidence.agent-context-execution]
finding_refs: []
candidate_refs: [asset.context-manager]
---

# 自定义 Loop ContextAssembler

## 资产身份

为自定义 Agent loop 提供 source ordering、预算分配和文件选择 building blocks。

## 来源与消费者

由 public API 与 executable examples 消费，默认 ReactAgent 不调用。

## 生命周期

调用时纯装配输入，不持有默认 ReAct 的跨轮 state。

## 候选关系

与 ContextManager 构成语义对齐候选，不表示应删除任一公开能力。

## 未知与限制

完整 contract parity 尚未验证。
