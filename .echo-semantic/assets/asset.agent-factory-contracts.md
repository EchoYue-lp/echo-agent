---
schema_version: 1
id: asset.agent-factory-contracts
kind: asset
title: 两类 AgentFactory 合同
asset_type: symbol
status: needs_review
risk: medium
observed_at: source:8b3972e1d2bc92f4ad59f511b6674eaaf243c1760f21973caf9e96558f71db90
boundary_refs: [boundary.agent-session-turn, boundary.workspace-architecture]
code_refs: [echo-core/src/agent/factory.rs, src/agent/subagent/registry.rs]
consumer_refs: [src/agent/default_factory.rs, src/agent/subagent/executor.rs]
behavior_refs: [behavior.agent-turn-lifecycle, behavior.task-subagent-execution]
rule_refs: [rule.framework-layer-ownership, rule.task-subagent-authority]
evidence_refs: [evidence.agent-context-execution, evidence.task-subagent-workflow]
finding_refs: []
candidate_refs: []
---

# 两类 AgentFactory 合同

## 资产身份

Core config-based Agent factory 与 Subagent registry lazy factory 使用同名 trait 但接受不同输入。

## 来源与消费者

DefaultAgentFactory 消费前者，SubagentRegistry/Executor 消费后者。

## 生命周期

分别在直接构造或 lazy Subagent resolve 时创建 Agent。

## 候选关系

需审查是否只需限定命名，不能因同名直接合并不同生命周期。

## 未知与限制

当前未形成 consolidation 决策。
